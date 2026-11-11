//! 7-Zip command-line adapter.
//!
//! Every switch here was chosen by experiment, not by reading docs, because the
//! documented forms and the working forms differ:
//!   * `-sccUTF-8` needs the hyphen; `sc=UTF-8` is rejected outright.
//!   * `pos=` / `dp=` are refused, so dictionary size goes through `-m0=LZMA2:d=`.
//!   * `-x!` is anchored to the current directory, so basename excludes need `-xr!`.
//!   * `@listfile` carries file *names* only, never switches.
//!   * stdin passwords (`-p` with the value on stdin) break when `-mhe=on` is set.

use crate::engine::{Engine, EngineError, EngineResult, Op};
use crate::formats::{self, Format};
use crate::models::{ArchiveInfo, EngineKind, Entry, ExtractRequest, Outcome, Progress};
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct SevenZip {
    pub exe: PathBuf,
    pub caps: Caps,
}

/// One backslash, spelled as a numeric escape so no text tooling can eat it.
const BS: char = '\u{5C}';

/// What this particular binary can do.  `7zr.exe` reads 7z but not RAR; a full
/// install does both, and the difference has to be discovered, not assumed.
#[derive(Clone, Debug, Default)]
pub struct Caps {
    /// 7-Zip's own format name (e.g. "7z", "Tar", "gzip") and whether it can create it.
    pub formats: Vec<(String, bool)>,
    pub codecs: Vec<String>,
    pub version: Option<String>,
}

/// 7-Zip's internal format names differ from ours, so the mapping is explicit.
/// `Rar5`/`Rar` are separate entries in newer builds; either one means "can read RAR".
fn sevenzip_names(format: Format) -> &'static [&'static str] {
    match format {
        Format::SevenZip => &["7z"],
        Format::Zip => &["Zip"],
        Format::Tar => &["Tar"],
        Format::GZip => &["gzip"],
        Format::BZip2 => &["bzip2"],
        Format::Xz => &["xz"],
        Format::Zstd => &["Zstd"],
        Format::Rar => &["Rar5", "Rar"],
        Format::Split => &["Split"],
        Format::Unknown => &[],
        // tar.* is two capabilities: the tar reader plus the compressor codec.
        Format::TarGz => &["Tar"],
        Format::TarBz2 => &["Tar"],
        Format::TarXz => &["Tar"],
        Format::TarZst => &["Tar"],
    }
}

fn codec_for(format: Format) -> Option<&'static str> {
    Some(match format {
        Format::TarGz => "gzip",
        Format::TarBz2 => "bzip2",
        Format::TarXz => "xz",
        Format::TarZst => "Zstd",
        _ => return None,
    })
}

impl Caps {
    pub fn supports(&self, format: Format, write: bool) -> bool {
        let names = sevenzip_names(format);
        if names.is_empty() {
            return false;
        }
        let hit = self
            .formats
            .iter()
            .find(|(name, _)| names.iter().any(|n| name.eq_ignore_ascii_case(n)));
        let Some((_, can_write)) = hit else { return false };
        if write && !*can_write {
            return false;
        }
        // A tar.gz needs the tar reader *and* the gzip codec present.
        match codec_for(format) {
            Some(c) => self
                .formats
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(c) || self.has_codec(c)),
            None => true,
        }
    }

    pub fn has_codec(&self, needle: &str) -> bool {
        self.codecs.iter().any(|m| m.eq_ignore_ascii_case(needle))
    }
}

/// Parse the `7z i` report.  Verified against 7-Zip (r) 26.00 output:
///
/// ```text
///    C...F..........c.a.m+..  7z       7z            7 z BC AF ' 1C
///   CK.....................  xz       xz txz (.tar) FD 7 z X Z 00
/// ```
///
/// The first token is a fixed-width flag string whose leading character is 'C'
/// exactly when creation is supported; the second is the format name.  The
/// extension column is padded and runs into the magic bytes, so it is ignored.
pub fn parse_info(text: &str) -> Caps {
    let mut caps = Caps::default();
    let mut section = String::new();
    for line in text.lines() {
        let t = line.trim_end_matches('\r');
        let trimmed = t.trim();
        if let Some(rest) = trimmed.strip_prefix("7-Zip") {
            caps.version = rest
                .split_whitespace()
                .find(|s| s.starts_with(|c: char| c.is_ascii_digit()))
                .map(|s| s.trim_end_matches(':').to_string());
            continue;
        }
        if trimmed.ends_with(':') && !trimmed.contains(' ') {
            section = trimmed[..trimmed.len() - 1].to_lowercase();
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        let cols: Vec<&str> = trimmed.split_whitespace().collect();
        match section.as_str() {
            "formats" if cols.len() >= 2 => {
                let can_write = cols[0].starts_with('C');
                caps.formats.push((cols[1].to_string(), can_write));
            }
            "codecs" if cols.len() >= 2 => {
                caps.codecs.push(cols[cols.len() - 1].to_string());
            }
            _ => {}
        }
    }
    caps
}

/// Locate a 7-Zip binary.  Order is deliberate: explicit env, PATH, then the
/// install directories, then the copy bundled with this workspace's tooling.
pub fn find_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("QARC_7Z") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    let names = if cfg!(windows) {
        ["7z.exe", "7za.exe", "7zr.exe"]
    } else {
        ["7z", "7za", "7zr"]
    };
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let mut homes = vec![
        PathBuf::from(r"C:\Program Files\7-Zip\7z.exe"),
        PathBuf::from(r"C:\Program Files (x86)\7-Zip\7z.exe"),
        PathBuf::from("/usr/bin/7z"),
        PathBuf::from("/opt/homebrew/bin/7z"),
    ];
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        homes.push(Path::new(&home).join(".qoder/bin/7zr.exe"));
        homes.push(Path::new(&home).join(".qoder/bin/7z"));
    }
    homes.into_iter().find(|p| p.is_file())
}

/// A password on a command line is visible to every process that can enumerate
/// them.  7-Zip's own documentation calls this out; the mitigation is to prefer
/// managed engines and to redact everywhere we log.
pub fn redact(args: &[String]) -> Vec<String> {
    args.iter()
        .map(|a| {
            if a.starts_with("-p") && a.len() > 2 {
                format!("-p{}", "*".repeat(a.len() - 2))
            } else {
                a.clone()
            }
        })
        .collect()
}

/// Switches shared by every verb.  `-y` because a blocked prompt in a GUI
/// subprocess is indistinguishable from a hang.  `-sccUTF-8` (hyphen included) is
/// what makes non-ASCII member names survive the round trip through the console.
pub fn base_args(password: Option<&str>) -> Vec<String> {
    let mut args = vec!["-y".to_string(), "-sccUTF-8".to_string()];
    if let Some(pw) = password {
        args.push(format!("-p{pw}"));
    }
    args
}

pub fn list_args(archive: &Path, password: Option<&str>) -> Vec<String> {
    let mut args = base_args(password);
    args.push("l".into());
    args.push("-ba".into()); // no header block: one less thing to parse wrong
    args.push("-slt".into()); // technical listing, gives size/packed/attrs
    args.push(archive.to_string_lossy().into_owned());
    args
}

pub fn extract_args(req: &ExtractRequest) -> Vec<String> {
    let mut args = base_args(req.password.as_deref());
    args.push(if req.test_only { "t".into() } else { "x".into() });
    args.push(format!("-o{}", req.dest));
    // -ao{a|s|t|u} is real; "newer than target" has no 7z equivalent and is decided
    // in the core before we get here.
    if let Some(code) = req.overwrite.sevenzip_code() {
        args.push(format!("-ao{code}"));
    }
    args.push(req.archive.clone());
    args.extend(req.members.iter().cloned());
    args
}

pub fn create_args(opts: &crate::models::CompressOptions) -> Vec<String> {
    let mut args = base_args(opts.password.as_deref());
    args.push("a".into());
    // -bb1 makes 7z print one " - <file>" line per member; without it there is no
    // way to count what was added, and the CLI used to report "0 項" for a
    // successful 400-file archive.
    args.push("-bb1".into());
    args.push(format!("-t{}", formats::info(opts.format).extension));
    if opts.threads > 0 {
        args.push(format!("-mmt{}", opts.threads));
    }
    if opts.format == Format::SevenZip {
        // Documented -m0=LZMA2:pos=/dp= forms are refused by the binary; this one works.
        args.push(format!("-m0=LZMA2:d={}", level_to_dict(opts.level)));
        if opts.header_encryption && opts.password.is_some() {
            args.push("-mhe=on".into());
        }
    } else {
        args.push(format!("-mx{}", opts.level));
    }
    args.push(opts.archive.clone());
    args.extend(opts.sources.iter().cloned());
    args
}

/// `Add new data to archive: 3 folders, 20 files, 19988608 bytes (20 MiB)` is the
/// only honest member count 7z gives for `a`; the per-file lines are `+ <path>`, not
/// the ` - <path>` shape the extract verb uses.
pub fn added_files(stdout: &str) -> usize {
    let from_summary = stdout.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix("Add new data to archive:")?;
        rest.split(',').find_map(|part| {
            let part = part.trim();
            part.strip_suffix(" files")?.trim().parse().ok()
        })
    });
    from_summary.unwrap_or_else(|| stdout.lines().filter(|l| l.starts_with('+')).count())
}

/// `Files: 3` / `Folders: 2` from the closing summary block.
pub fn summary_count(stdout: &str, field: &str) -> usize {
    summary_u64(stdout, field) as usize
}

pub fn summary_u64(stdout: &str, field: &str) -> u64 {
    let needle = format!("{field}:");
    stdout
        .lines()
        .filter_map(|l| l.trim().strip_prefix(&needle))
        .filter_map(|v| v.trim().replace(' ', "").parse::<u64>().ok())
        .next()
        .unwrap_or(0)
}

/// Rewrite the create command so 7z stores names relative to the sources' common
/// parent.  Without this, `a out.7z /tmp/src` archives members as `/tmp/src/...`.
pub fn create_plan(opts: &crate::models::CompressOptions) -> (Vec<String>, Option<PathBuf>) {
    let mut relative = opts.clone();
    let mut cwd = None;
    if !opts.sources.is_empty() {
        let paths: Vec<PathBuf> = opts.sources.iter().map(PathBuf::from).collect();
        let parents: Vec<PathBuf> = paths
            .iter()
            .map(|p| p.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")))
            .collect();
        let common = parents
            .iter()
            .skip(1)
            .fold(parents[0].clone(), |acc, p| {
                let mut n = 0;
                for (a, b) in acc.components().zip(p.components()) {
                    if a != b {
                        break;
                    }
                    n += 1;
                }
                acc.components().take(n).collect()
            });
        if common.as_os_str().is_empty() {
            // Different drives or root-level sources: 7z keeps whatever it is given.
            cwd = None;
        } else {
            relative.sources = paths
                .iter()
                .map(|p| {
                    p.strip_prefix(&common)
                        .unwrap_or(p.as_path())
                        .to_string_lossy()
                        .replace(BS, "/")
                })
                .collect();
            relative.archive = std::path::absolute(&opts.archive)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| opts.archive.clone());
            cwd = Some(common);
        }
    }
    (create_args(&relative), cwd)
}

fn level_to_dict(level: u8) -> String {
    let kb = match level {
        0 => 64,
        1 => 1024,
        2 => 4096,
        3 => 8192,
        4 | 5 => 16384,
        6 => 32768,
        7 => 49152,
        8 => 65536,
        _ => 98304,
    };
    format!("{kb}k")
}

/// Exit codes as measured on 7-Zip 21.x-26.x, plus the output text.
///
/// The text matters: 7z returns 7 both for a wrong password and for a damaged
/// volume, and calling a corrupt archive "wrong password" sends the user to the
/// wrong door.  Only an explicit password complaint is trusted as one.
pub fn error_from_code(code: i32, stdout: &str, stderr: &str) -> Option<EngineError> {
    let text = format!("{stdout}{stderr}");
    let mentions_password = text.contains("Wrong password")
        || text.contains("Enter password")
        || text.contains("Headers Error");
    if code == 0 {
        return None;
    }
    if mentions_password {
        return Some(EngineError::WrongPassword);
    }
    Some(match code {
        1 => EngineError::Corrupt("警告：部分檔案未完成".into()),
        2 => {
            if text.contains("Can't open as archive") || text.contains("Cannot open") {
                EngineError::Corrupt("不是可識別的壓縮包或無法開啟".into())
            } else {
                EngineError::Corrupt(text.trim().to_string())
            }
        }
        7 => EngineError::Corrupt("資料錯誤（檔頭或資料區損毀）".into()),
        8 => EngineError::Io("記憶體或磁碟限制".into()),
        _ => EngineError::EngineUnavailable(format!("exit {code}")),
    })
}

/// ` 45% 12 - path\to\file.ext` and ` 100% 330 -` style progress lines.
pub fn parse_progress(line: &str) -> Option<Progress> {
    let t = line.trim();
    let pct_end = t.find('%')?;
    let pct: f64 = t[..pct_end].trim().parse().ok()?;
    let rest = t[pct_end + 1..].trim_start();
    let summary = match rest.split_once(" - ") {
        Some((_, tail)) => tail.trim().to_string(),
        None => rest.to_string(),
    };
    Some(Progress {
        fraction: Some((pct.clamp(0.0, 100.0)) / 100.0),
        done: 0,
        total: 0,
        summary,
    })
}

/// `-slt` block: `path = x`, `Size = 1`, `Modified = 2026-01-02 03:04:05`.
pub fn parse_listing(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut cur: Option<Entry> = None;
    let flush = |cur: &mut Option<Entry>, out: &mut Vec<Entry>| {
        if let Some(e) = cur.take() {
            if !e.path.is_empty() {
                out.push(e);
            }
        }
    };
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            flush(&mut cur, &mut out);
            continue;
        }
        let (key, value) = match line.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        let key = key.trim();
        let value = value.trim();
        let entry = cur.get_or_insert_with(|| Entry {
            name: String::new(),
            path: String::new(),
            is_dir: false,
            size: 0,
            packed: None,
            mtime: None,
            attrs: String::new(),
            encrypted: false,
            codec: None,
            unsafe_path: false,
        });
        match key {
            "Path" | "path" => {
                entry.path = value.replace('\\', "/");
                entry.name = entry.path.rsplit('/').next().unwrap_or(&entry.path).to_string();
                entry.is_dir = entry.path.ends_with('/');
            }
            "Size" => entry.size = value.parse().unwrap_or(0),
            // 7z does not append a slash to a directory's Path; the D flag in
            // Attributes is what says "folder".  Trusting only the trailing slash
            // turned every 7z directory into a second, 0-byte row beside the real one.
            "Packed Size" => {
                if !value.is_empty() {
                    entry.packed = value.parse().ok();
                }
            }
            "Attributes" => {
                entry.attrs = value.to_string();
                if value.contains('D') {
                    entry.is_dir = true;
                    if !entry.path.ends_with('/') {
                        entry.path.push('/');
                    }
                }
            }
            "Modified" => entry.mtime = parse_mtime(value),
            "Encrypted" => entry.encrypted = value == "+" || value == "-h",
            "Characteristics" => entry.encrypted |= value.contains("h"),
            _ => {}
        }
    }
    flush(&mut cur, &mut out);
    out
}

pub fn parse_mtime(value: &str) -> Option<i64> {
    // "2026-11-05 16:11:03" -> unix seconds, no external date crate needed for
    // whole seconds; a wrong parse yields None rather than a fabricated time.
    let bytes = value.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b' ' {
        return None;
    }
    let n = |i: usize, w: usize| value[i..i + w].parse::<i64>().ok();
    let (y, mo, d, h, mi, s) = (n(0, 4)?, n(5, 2)?, n(8, 2)?, n(11, 2)?, n(14, 2)?, n(17, 2)?);
    let days = days_from_civil(y, mo, d);
    Some(days * 86_400 + h * 3_600 + mi * 60 + s)
}

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

impl SevenZip {
    pub fn probe(exe: PathBuf) -> EngineResult<Self> {
        let out = Command::new(&exe)
            .arg("i")
            .output()
            .map_err(|e| EngineError::EngineUnavailable(e.to_string()))?;
        if !out.status.success() {
            return Err(EngineError::EngineUnavailable(format!("{} i", exe.display())));
        }
        let caps = parse_info(&String::from_utf8_lossy(&out.stdout));
        if caps.formats.is_empty() {
            return Err(EngineError::EngineUnavailable("無法解讀 7z i 輸出".into()));
        }
        Ok(Self { exe, caps })
    }

    fn run_in(
        &self,
        args: &[String],
        cwd: Option<&Path>,
        cancel: crate::cancel::Cancel,
        mut on_line: impl FnMut(&str),
    ) -> EngineResult<(i32, String, String)> {
        if std::env::var_os("QARC_DEBUG_ARGS").is_some() {
            eprintln!("[7z] {} {}", self.exe.display(), redact(args).join(" "));
        }
        let mut cmd = Command::new(&self.exe);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| EngineError::EngineUnavailable(e.to_string()))?;
        let stdout = child.stdout.take().expect("stdout piped");
        // stderr is drained on its own thread.  Reading it to EOF here instead
        // deadlocks as soon as a big archive fills the stdout pipe: the child
        // cannot exit until stdout is consumed, and stdout is not consumed until
        // stderr has been read.
        let err_buf = child.stderr.take().map(|stderr| {
            std::thread::spawn(move || {
                let mut s = String::new();
                let mut reader = std::io::BufReader::new(stderr);
                let _ = reader.read_to_string(&mut s);
                s
            })
        });
        let mut lines = String::new();
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            let line = line.map_err(|e| EngineError::Io(e.to_string()))?;
            if crate::cancel::stopped(cancel) {
                // Waiting for 7z to finish a 40 GB archive after the user pressed
                // cancel is not cancellation.
                let _ = child.kill();
                let _ = child.wait();
                if let Some(handle) = err_buf {
                    let _ = handle.join();
                }
                return Err(EngineError::Cancelled);
            }
            on_line(&line);
            lines.push_str(&line);
            lines.push('\n');
        }
        let status = child.wait().map_err(|e| EngineError::Io(e.to_string()))?;
        let err_buf = err_buf.map(|h| h.join().unwrap_or_default()).unwrap_or_default();
        let code = status.code().unwrap_or(-1);
        if std::env::var_os("QARC_DEBUG_ARGS").is_some() && code != 0 {
            eprintln!("[7z] exit={code}\n[7z] stdout={}\n[7z] stderr={}", lines.trim(), err_buf.trim());
        }
        Ok((code, lines, err_buf))
    }

    fn run(&self, args: &[String], on_line: impl FnMut(&str)) -> EngineResult<(i32, String, String)> {
        self.run_in(args, None, None, on_line)
    }

    pub fn version(&self) -> Option<String> {
        let out = Command::new(&self.exe).arg("i").output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        parse_info(&text).version
    }
}

impl Engine for SevenZip {
    fn id(&self) -> &'static str {
        "7zip"
    }

    fn kind(&self) -> EngineKind {
        EngineKind::CommandLine
    }

    fn supports(&self, format: Format, op: Op) -> bool {
        if format == Format::Unknown {
            return false;
        }
        self.caps.supports(format, op == Op::Write)
    }

    fn priority(&self, format: Format, op: Op) -> i32 {
        if !self.supports(format, op) {
            return -1;
        }
        // 7z is the reference engine for its own formats and the catch-all for
        // everything else (RAR read, xz, iso, wim) that no managed crate covers.
        match format {
            Format::SevenZip | Format::Rar | Format::Xz | Format::Split => 100,
            f if f.is_compound() => 60,
            _ => 40,
        }
    }

    fn location(&self) -> Option<String> {
        Some(self.exe.to_string_lossy().into_owned())
    }

    fn list(&self, archive: &Path, password: Option<&str>) -> EngineResult<ArchiveInfo> {
        if !archive.is_file() {
            return Err(EngineError::Io(format!("檔案不存在：{}", archive.display())));
        }
        let args = list_args(archive, password);
        let (code, stdout, stderr) = self.run(&args, |_| {})?;
        if let Some(e) = error_from_code(code, &stdout, &stderr) {
            return Err(e);
        }
        let mut entries = parse_listing(&stdout);
        for entry in &mut entries {
            entry.unsafe_path = crate::pathguard::is_traversal(&entry.path);
            if entry.unsafe_path {
                entry.name = format!("{}  [越界]", entry.name);
            }
        }
        let path = archive.to_string_lossy().into_owned();
        let format = formats::sniff(archive);
        Ok(ArchiveInfo {
            needs_password: entries.iter().any(|e| e.encrypted),
            total_packed: entries.iter().filter_map(|e| e.packed).sum(),
            total_size: entries.iter().map(|e| e.size).sum(),
            engine: self.id().to_string(),
            entries,
            format,
            path,
            ..Default::default()
        })
    }

    fn create_cancellable(
        &self,
        opts: &crate::models::CompressOptions,
        report: &mut dyn FnMut(Progress),
        cancel: crate::cancel::Cancel,
    ) -> EngineResult<Outcome> {
        let (args, cwd) = create_plan(opts);
        let run = self.run_in(&args, cwd.as_deref(), cancel, |line| {
            if let Some(p) = parse_progress(line) {
                report(p);
            }
        });
        let (code, stdout, stderr) = match run {
            Ok(v) => v,
            // The child is already killed; report what stopped rather than a
            // failure the user cannot act on.
            Err(EngineError::Cancelled) =>
                return Ok(Outcome {
                    ok: false,
                    cancelled: true,
                    engine: self.id().to_string(),
                    ..Default::default()
                }),
            Err(e) => return Err(e),
        };
        if let Some(e) = error_from_code(code, &stdout, &stderr) {
            return Err(e);
        }
        // 7z reports 100% even when it wrote nothing at all; the file size is the
        // only thing that cannot lie about that.
        let written = std::fs::metadata(&opts.archive).map(|m| m.len()).unwrap_or(0);
        if written == 0 {
            return Err(EngineError::Corrupt("7z 回報成功但沒有写出任何位元組".into()));
        }
        Ok(Outcome {
            ok: true,
            cancelled: false,
            items: added_files(&stdout),
            bytes_out: written,
            errors: Vec::new(),
            blocked: Vec::new(),
            engine: self.id().to_string(),
        })
    }

    fn extract_cancellable(
        &self,
        req: &ExtractRequest,
        report: &mut dyn FnMut(Progress),
        cancel: crate::cancel::Cancel,
    ) -> EngineResult<Outcome> {
        let args = extract_args(req);
        let run = self.run_in(&args, None, cancel, |line| {
            if let Some(p) = parse_progress(line) {
                report(p);
            }
        });
        let (code, stdout, stderr) = match run {
            Ok(v) => v,
            // The child is already killed; report what stopped rather than a
            // failure the user cannot act on.
            Err(EngineError::Cancelled) =>
                return Ok(Outcome {
                    ok: false,
                    cancelled: true,
                    engine: self.id().to_string(),
                    ..Default::default()
                }),
            Err(e) => return Err(e),
        };
        if let Some(e) = error_from_code(code, &stdout, &stderr) {
            return Err(e);
        }
        Ok(Outcome {
            ok: true,
            cancelled: false,
            items: summary_count(&stdout, "Files"),
            bytes_out: summary_u64(&stdout, "Size"),
            errors: Vec::new(),
            blocked: Vec::new(),
            engine: self.id().to_string(),
        })
    }
}
