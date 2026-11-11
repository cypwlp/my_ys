//! Legacy-encoding repair for archive member names.
//!
//! ZIP and old tar writers stored GBK/Big5 bytes with no codepage marker, and
//! readers default to cp437 — so `報告.docx` arrives as `σÉæ¡┐.docx`.  The fix is a
//! round-trip: re-encode the mojibake back to the bytes that produced it, then try
//! each candidate codepage and keep the reading that actually looks like text.

use encoding_rs::Encoding;

/// cp437 box-drawing/accents table, index = byte.  encoding_rs follows WHATWG and
/// deliberately omits cp437, so the table lives here.
static CP437: [char; 256] = [
    '\u{0000}', '\u{0001}', '\u{0002}', '\u{0003}', '\u{0004}', '\u{0005}', '\u{0006}', '\u{0007}', '\u{0008}', '\u{0009}', '\u{000A}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{000E}', '\u{000F}',
    '\u{0010}', '\u{0011}', '\u{0012}', '\u{0013}', '\u{0014}', '\u{0015}', '\u{0016}', '\u{0017}', '\u{0018}', '\u{0019}', '\u{001A}', '\u{001B}', '\u{001C}', '\u{001D}', '\u{001E}', '\u{001F}',
    '\u{0020}', '\u{0021}', '\u{0022}', '\u{0023}', '\u{0024}', '\u{0025}', '\u{0026}', '\u{0027}', '\u{0028}', '\u{0029}', '\u{002A}', '\u{002B}', '\u{002C}', '\u{002D}', '\u{002E}', '\u{002F}',
    '\u{0030}', '\u{0031}', '\u{0032}', '\u{0033}', '\u{0034}', '\u{0035}', '\u{0036}', '\u{0037}', '\u{0038}', '\u{0039}', '\u{003A}', '\u{003B}', '\u{003C}', '\u{003D}', '\u{003E}', '\u{003F}',
    '\u{0040}', '\u{0041}', '\u{0042}', '\u{0043}', '\u{0044}', '\u{0045}', '\u{0046}', '\u{0047}', '\u{0048}', '\u{0049}', '\u{004A}', '\u{004B}', '\u{004C}', '\u{004D}', '\u{004E}', '\u{004F}',
    '\u{0050}', '\u{0051}', '\u{0052}', '\u{0053}', '\u{0054}', '\u{0055}', '\u{0056}', '\u{0057}', '\u{0058}', '\u{0059}', '\u{005A}', '\u{005B}', '\u{005C}', '\u{005D}', '\u{005E}', '\u{005F}',
    '\u{0060}', '\u{0061}', '\u{0062}', '\u{0063}', '\u{0064}', '\u{0065}', '\u{0066}', '\u{0067}', '\u{0068}', '\u{0069}', '\u{006A}', '\u{006B}', '\u{006C}', '\u{006D}', '\u{006E}', '\u{006F}',
    '\u{0070}', '\u{0071}', '\u{0072}', '\u{0073}', '\u{0074}', '\u{0075}', '\u{0076}', '\u{0077}', '\u{0078}', '\u{0079}', '\u{007A}', '\u{007B}', '\u{007C}', '\u{007D}', '\u{007E}', '\u{007F}',
    '\u{00C7}', '\u{00FC}', '\u{00E9}', '\u{00E2}', '\u{00E4}', '\u{00E0}', '\u{00E5}', '\u{00E7}', '\u{00EA}', '\u{00EB}', '\u{00E8}', '\u{00EF}', '\u{00EE}', '\u{00EC}', '\u{00C4}', '\u{00C5}',
    '\u{00C9}', '\u{00E6}', '\u{00C6}', '\u{00F4}', '\u{00F6}', '\u{00F2}', '\u{00FB}', '\u{00F9}', '\u{00FF}', '\u{00D6}', '\u{00DC}', '\u{00A2}', '\u{00A3}', '\u{00A5}', '\u{20A7}', '\u{0192}',
    '\u{00E1}', '\u{00ED}', '\u{00F3}', '\u{00FA}', '\u{00F1}', '\u{00D1}', '\u{00AA}', '\u{00BA}', '\u{00BF}', '\u{2310}', '\u{00AC}', '\u{00BD}', '\u{00BC}', '\u{00A1}', '\u{00AB}', '\u{00BB}',
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}', '\u{2524}', '\u{2561}', '\u{2562}', '\u{2556}', '\u{2555}', '\u{2563}', '\u{2551}', '\u{2557}', '\u{255D}', '\u{255C}', '\u{255B}', '\u{2510}',
    '\u{2514}', '\u{2534}', '\u{252C}', '\u{251C}', '\u{2500}', '\u{253C}', '\u{255E}', '\u{255F}', '\u{255A}', '\u{2554}', '\u{2569}', '\u{2566}', '\u{2560}', '\u{2550}', '\u{256C}', '\u{2567}',
    '\u{2568}', '\u{2564}', '\u{2565}', '\u{2559}', '\u{2558}', '\u{2552}', '\u{2553}', '\u{256B}', '\u{256A}', '\u{2518}', '\u{250C}', '\u{2588}', '\u{2584}', '\u{258C}', '\u{2590}', '\u{2580}',
    '\u{03B1}', '\u{00DF}', '\u{0393}', '\u{03C0}', '\u{03A3}', '\u{03C3}', '\u{00B5}', '\u{03C4}', '\u{03A6}', '\u{0398}', '\u{03A9}', '\u{03B4}', '\u{221E}', '\u{03C6}', '\u{03B5}', '\u{2229}',
    '\u{2261}', '\u{00B1}', '\u{2265}', '\u{2264}', '\u{2320}', '\u{2321}', '\u{00F7}', '\u{2248}', '\u{00B0}', '\u{2219}', '\u{00B7}', '\u{221A}', '\u{207F}', '\u{00B2}', '\u{25A0}', '\u{00A0}',
];

/// Candidates in the order a Chinese-language user is most likely to want them.
static CANDIDATES: &[(&str, &Encoding)] = &[
    ("UTF-8", encoding_rs::UTF_8),
    ("GB18030", encoding_rs::GB18030),
    ("Big5", encoding_rs::BIG5),
    ("EUC-KR", encoding_rs::EUC_KR),
    ("Shift_JIS", encoding_rs::SHIFT_JIS),
    ("windows-1251", encoding_rs::WINDOWS_1251),
    ("windows-1252", encoding_rs::WINDOWS_1252),
];

fn to_cp437_bytes(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len());
    for ch in text.chars() {
        let byte = if ch == '\u{0020}' {
            Some(b' ')
        } else {
            CP437.iter().position(|c| *c == ch).map(|i| i as u8)
        };
        match byte {
            Some(b) => out.push(b),
            None => return None, // not cp437-round-trippable, so not our mojibake
        }
    }
    Some(out)
}

/// Decode header bytes the way a spec-following reader would have: ZIP stores no
/// codepage, so cp437 is the default and the mojibake we later repair comes from here.
pub fn from_cp437(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| CP437[*b as usize])
        .collect()
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF)
}

/// Garble signatures: the byte patterns that show up when one wrong codepage is
/// re-decoded by another.  A candidate containing these is almost never the truth.
const GARBLE: &[&str] = &["锟斤拷", "鎳", "銆", "锘", "Ã©", "â€"];

fn score(text: &str) -> i32 {
    let mut s = 0i32;
    let mut cjk = 0i32;
    let mut latin = 0i32;
    let mut bad = 0i32;
    for ch in text.chars() {
        if is_cjk(ch) {
            cjk += 1;
        } else if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '.' | '-' | '_' | '(' | ')' | '/') {
            latin += 1;
        } else if ch == '\u{FFFD}' || ch == '?' {
            bad += 1;
        } else if (ch as u32) < 0x20 || matches!(ch, '\u{2500}'..='\u{25FF}' | '\u{00C0}'..='\u{00FF}') {
            bad += 1;
        }
    }
    s += cjk * 6 + latin;
    s -= bad * 12;
    for g in GARBLE {
        if text.contains(g) {
            s -= 40;
        }
    }
    s
}

/// Repair a member name.  Returns the readable name plus the codepage label that
/// produced it; the label is what the UI shows in the 檔名編碼 column.
///
/// Fail-safe by construction: if no candidate beats the original, the original is
/// returned untouched and no label is claimed.
pub fn repair(mojibake: &str) -> (String, Option<String>) {
    if mojibake.is_empty() || !mojibake.chars().any(|c| !c.is_ascii()) {
        return (mojibake.to_string(), None);
    }
    let base = score(mojibake);
    let bytes = match to_cp437_bytes(mojibake) {
        Some(b) => b,
        None => return (mojibake.to_string(), None),
    };
    pick_best(&bytes, base)
}

/// The raw-bytes form, for formats that store names without any encoding marker
/// at all (tar) -- there is no cp437 round trip to undo, so the bytes go straight
/// to the candidates.
pub fn repair_bytes(bytes: &[u8]) -> (String, Option<String>) {
    if bytes.is_empty() || bytes.iter().all(|b| b.is_ascii()) {
        return (String::from_utf8_lossy(bytes).into_owned(), None);
    }
    let lossy = encoding_rs::UTF_8.decode(bytes).0.into_owned();
    let base = score(&lossy);
    pick_best(bytes, base)
}

fn pick_best(bytes: &[u8], base: i32) -> (String, Option<String>) {
    let mut best: Option<(i32, String, String)> = None;
    for (label, enc) in CANDIDATES {
        let (cow, _, had_errors) = enc.decode(bytes);
        let decoded = cow.into_owned();
        if decoded.is_empty() {
            continue;
        }
        let mut s = score(&decoded);
        if had_errors {
            s -= 8;
        }
        if s > base + 8 && s > best.as_ref().map(|(v, _, _)| *v).unwrap_or(i32::MIN) {
            best = Some((s, decoded, (*label).to_string()));
        }
    }
    match best {
        Some((_, name, label)) => (name, Some(label)),
        None => (String::from_utf8_lossy(bytes).into_owned(), None),
    }
}

/// The name engines hand back when they already know the codepage (7z prints
/// "GBK" / "Big5", not "cp936" / "cp950"); map the loose labels onto encodings.
pub fn resolve(label: &str) -> Option<&'static Encoding> {
    let key = label.trim().to_lowercase();
    let key = key.replace(['-', '_', ' '], "");
    Some(match key.as_str() {
        "utf8" | "u8" => encoding_rs::UTF_8,
        "gbk" | "gb2312" | "gb18030" | "cp936" | "ms936" | "936" => encoding_rs::GB18030,
        "big5" | "cp950" | "ms950" | "950" | "big5hkscs" => encoding_rs::BIG5,
        "euckr" | "cp949" | "949" => encoding_rs::EUC_KR,
        "shiftjis" | "sjis" | "cp932" | "932" => encoding_rs::SHIFT_JIS,
        "windows1251" | "cp1251" | "1251" | "koi8r" => encoding_rs::WINDOWS_1251,
        "windows1252" | "cp1252" | "1252" | "latin1" | "iso88591" => encoding_rs::WINDOWS_1252,
        _ => return None,
    })
}
