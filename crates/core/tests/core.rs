//! Behavioural parity tests for the Rust port.  These mirror the cases that the
//! .NET suite proved the hard way: a fallback that swallows a wrong password, an
//! extraction that reports success while writing nothing, and a member name that
//! escapes the destination.

use qarc_core::engine::{Engine, EngineError, EngineResult, Op, Registry, with_fallback};
use qarc_core::formats::{self, Format};
use qarc_core::models::{ArchiveInfo, CompressOptions, Entry, ExtractRequest, Outcome, Overwrite, Progress};
use qarc_core::pathguard;
use qarc_core::repair;
use qarc_core::sevenzip;
use qarc_core::tar::TarEngine;
use qarc_core::zip_engine::ZipEngine;
use std::path::{Path, PathBuf};

/// Self-contained temp directory.  `tempfile` was not an option: it pulls
/// `getrandom` -> `windows-sys`, and windows-sys needs dlltool, which this
/// machine does not have.  Staying pure-Rust is what keeps the suite runnable.
struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let mut root = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        root.push(format!("qarc-test-{tag}-{stamp}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        TempDir { root }
    }
    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn tempdir(tag: &str) -> TempDir {
    TempDir::new(tag)
}

fn fixture_7zr_info() -> &'static str {
    // Captured from `7zr.exe i` on this machine, 7-Zip (r) 26.00 (x86).
    "
7-Zip (r) 26.00 (x86) : Igor Pavlov : Public domain : 2026-02-12


Formats:
   C...F..........c.a.m+..  7z       7z            7 z BC AF ' 1C
    ......................  Split    001
    K.....O...............  lzma     lzma
    K.....................  lzma86   lzma86
   CK.....................  xz       xz txz (.tar) FD 7 z X Z 00
   CK.....O.....XC........  Hash     sha256 sha512 md5 crc32 crc64

Codecs:
   4ED   303011B BCJ2
    EDF  3030103 BCJ
    ED         0 Copy
    ED        21 LZMA2
    EDF  6F10701 7zAES

Hashers:
      4        1 CRC32
"
}

// ---------------------------------------------------------------- formats

#[test]
fn compound_suffix_beats_single_layer() {
    assert_eq!(formats::from_extension("a/b/報告.tar.gz"), Format::TarGz);
    assert_eq!(formats::from_extension("plain.gz"), Format::GZip);
    assert_eq!(formats::from_extension("x.TAR.ZST"), Format::TarZst);
    assert_eq!(formats::from_extension("notes.txt"), Format::Unknown);
}

#[test]
fn magic_overrides_a_lying_extension() {
    let mut head = vec![0u8; 512];
    head[257..265].copy_from_slice(b"ustar\x0000");
    assert_eq!(formats::from_magic(&head), Some(Format::Tar));
    assert_eq!(formats::from_magic(b"PK\x03\x04junk"), Some(Format::Zip));
    assert_eq!(formats::from_magic(b"\xfd7zXZ\x00"), Some(Format::Xz));
    assert_eq!(formats::from_magic(b"not an archive"), None);
}

#[test]
fn tar_gz_is_not_mistaken_for_bare_gzip() {
    let dir = tempdir("case");
    let path = dir.path().join("bundle.tar.gz");
    std::fs::write(&path, b"\x1f\x8b\x08\x00rest of a gzip stream").unwrap();
    assert_eq!(formats::sniff(&path), Format::TarGz);
    let lone = dir.path().join("data.gz");
    std::fs::write(&lone, b"\x1f\x8b\x08\x00rest").unwrap();
    assert_eq!(formats::sniff(&lone), Format::GZip);
}

// -------------------------------------------------------------- pathguard

#[test]
fn traversal_is_blocked_before_it_reaches_the_disk() {
    for name in ["../evil.txt", "a/../../evil.txt", "/etc/passwd", "C:/windows/x", "..\\..\\evil"] {
        assert!(pathguard::is_traversal(name), "{name} should be blocked");
    }
    for name in ["ok.txt", "dir/sub/ok.txt", "a/../b.txt", "./x"] {
        assert!(!pathguard::is_traversal(name), "{name} should be allowed");
    }
}

#[test]
fn safe_join_resolves_inside_and_rejects_outside() {
    let dir = tempdir("case");
    let root = dir.path();
    let inside = pathguard::safe_join(root, "docs/report.txt").unwrap();
    assert!(inside.starts_with(root));
    assert_eq!(inside, root.join("docs").join("report.txt"));
    for name in ["../escape.txt", "a/../../escape.txt", "/absolute"] {
        assert!(pathguard::safe_join(root, name).is_err(), "{name} must not resolve");
    }
}

#[test]
fn unique_name_never_overwrites() {
    let dir = tempdir("case");
    let target = dir.path().join("report.txt");
    std::fs::write(&target, b"keep").unwrap();
    let alt = pathguard::unique_name(&target);
    assert_eq!(alt, dir.path().join("report (2).txt"));
    assert!(!alt.exists());
}

// ----------------------------------------------------------------- repair

#[test]
fn gb18030_names_survive_the_cp437_round_trip() {
    let real = "報告表2026/季報.xlsx";
    let raw = encoding_rs::GB18030.encode(real).0.into_owned();
    let mojibake = repair::from_cp437(&raw);
    assert_ne!(mojibake, real, "the fixture must actually be mojibake");
    let (fixed, label) = repair::repair(&mojibake);
    assert_eq!(fixed, real);
    assert_eq!(label.as_deref(), Some("GB18030"));
}

#[test]
fn big5_names_are_repaired_and_labelled() {
    let real = "季報.xlsx";
    let raw = encoding_rs::BIG5.encode(real).0.into_owned();
    let (fixed, label) = repair::repair_bytes(&raw);
    assert_eq!(fixed, real);
    assert_eq!(label.as_deref(), Some("Big5"));
}

#[test]
fn repair_is_fail_safe_on_correct_input() {
    // Already-correct UTF-8, pure ASCII, and un-round-trippable text must all come
    // back untouched: a repair that guesses is worse than one that does nothing.
    let utf8 = "報告.docx";
    assert_eq!(repair::repair(utf8).0, utf8);
    assert_eq!(repair::repair(utf8).1, None);
    assert_eq!(repair::repair("plain.txt").1, None);
    let odd = "日本語のファイル.docx";
    assert_eq!(repair::repair(odd).0, odd);
}

#[test]
fn loose_codepage_labels_from_7z_resolve() {
    assert!(repair::resolve("GBK").is_some());
    assert!(repair::resolve("cp936").is_some());
    assert!(repair::resolve("Big5").is_some());
    assert!(repair::resolve("cp950").is_some());
    assert!(repair::resolve("not-a-codepage").is_none());
}

// --------------------------------------------------------------- sevenzip

#[test]
fn capabilities_are_read_from_the_binary_not_assumed() {
    let caps = sevenzip::parse_info(fixture_7zr_info());
    assert_eq!(caps.version.as_deref(), Some("26.00"));
    assert!(caps.supports(Format::SevenZip, true), "7zr can create 7z");
    assert!(caps.supports(Format::Xz, false));
    assert!(!caps.supports(Format::Zip, false), "7zr has no zip codec");
    assert!(!caps.supports(Format::Rar, false), "7zr has no rar codec");
    assert!(caps.has_codec("LZMA2"));
    assert!(!caps.has_codec("PPC"));
}

#[test]
fn passwords_are_redacted_in_every_logged_form() {
    let args = vec!["-y".into(), "-pS3cr3t!".into(), "a.7z".into()];
    let red = sevenzip::redact(&args);
    assert_eq!(red[1], "-p*******");
    assert!(!red.iter().any(|a| a.contains("S3cr3t")));
}

#[test]
fn switches_use_the_forms_the_binary_accepts() {
    let args = sevenzip::base_args(Some("pw"));
    assert!(args.iter().any(|a| a == "-sccUTF-8"), "hyphen form is mandatory");
    assert!(args.iter().any(|a| a == "-ppw"), "inline -p<pw> is all 7z accepts");
    assert!(!args.iter().any(|a| a == "sc=UTF-8"));

    let req = ExtractRequest {
        archive: "a.7z".into(),
        dest: "out".into(),
        members: vec!["dir/".into()],
        password: Some("pw".into()),
        overwrite: Overwrite::Skip,
        ..Default::default()
    };
    let ex = sevenzip::extract_args(&req);
    assert!(ex.contains(&"x".to_string()));
    assert!(ex.iter().any(|a| a == "-oout"));
    assert!(ex.iter().any(|a| a == "-aot"), "Skip maps to -aot");

    let ask = ExtractRequest { overwrite: Overwrite::Ask, ..req };
    assert!(!sevenzip::extract_args(&ask).iter().any(|a| a.starts_with("-ao")));

    let opts = CompressOptions {
        archive: "out.7z".into(),
        sources: vec!["a".into()],
        format: Format::SevenZip,
        password: Some("pw".into()),
        header_encryption: true,
        level: 5,
        threads: 4,
    };
    let cr = sevenzip::create_args(&opts);
    assert!(cr.iter().any(|a| a == "-mmt4"), "thread switch is -mmt");
    assert!(cr.iter().any(|a| a.starts_with("-m0=LZMA2:d=")), "pos=/dp= are refused");
    assert!(cr.iter().any(|a| a == "-mhe=on"));
}

#[test]
fn exit_codes_map_to_specific_faults() {
    assert!(sevenzip::error_from_code(0, "", "").is_none());
    // 7 alone is a data error; only an explicit password complaint is a password error.
    assert!(matches!(sevenzip::error_from_code(7, "", ""), Some(EngineError::Corrupt(_))));
    assert_eq!(
        sevenzip::error_from_code(7, "", "Wrong password"),
        Some(EngineError::WrongPassword)
    );
    assert!(matches!(sevenzip::error_from_code(2, "Can't open as archive", ""), Some(EngineError::Corrupt(_))));
    assert!(matches!(sevenzip::error_from_code(99, "", ""), Some(EngineError::EngineUnavailable(_))));
}

#[test]
fn progress_and_listing_are_parsed_not_guessed() {
    let p = sevenzip::parse_progress(" 45% 3 - docs\\report.txt").unwrap();
    assert_eq!(p.fraction, Some(0.45));
    assert!(p.summary.ends_with("report.txt"), "{}", p.summary);
    assert!(sevenzip::parse_progress("no percent here").is_none());

    let listing = "\
Path = 報告表2026/季報.xlsx
Size = 1000
Packed Size = 400
Modified = 2026-11-05 16:11:03
Attributes = A
Encrypted = -

Path = 報告表2026/
Attributes = D
";
    let entries = sevenzip::parse_listing(listing);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].size, 1000);
    assert_eq!(entries[0].packed, Some(400));
    assert_eq!(entries[0].mtime, Some(1_793_895_063));
    assert!(entries[1].is_dir);

    let evil = sevenzip::parse_listing("Path = ..\\..\\evil.txt\nSize = 1\n");
    assert_eq!(evil.len(), 1);
    assert!(evil[0].path.contains(".."));
}

#[test]
fn mtime_parser_rejects_junk_instead_of_inventing_time() {
    assert!(sevenzip::parse_mtime("2026-11-05 16:11:03").is_some());
    assert_eq!(sevenzip::parse_mtime("2026-11-05 16:11:03"), Some(1_793_895_063));
    assert!(sevenzip::parse_mtime("").is_none());
    assert!(sevenzip::parse_mtime("not a date").is_none());
}

// ---------------------------------------------------------------- engine

struct AlwaysFails;
impl Engine for AlwaysFails {
    fn id(&self) -> &'static str { "fails" }
    fn kind(&self) -> qarc_core::models::EngineKind { qarc_core::models::EngineKind::Managed }
    fn supports(&self, _: Format, _: Op) -> bool { true }
    fn list(&self, _: &Path, _: Option<&str>) -> EngineResult<ArchiveInfo> {
        Err(EngineError::UnsupportedFormat("nope".into()))
    }
}

struct WrongPassword;
impl Engine for WrongPassword {
    fn id(&self) -> &'static str { "pw" }
    fn kind(&self) -> qarc_core::models::EngineKind { qarc_core::models::EngineKind::Managed }
    fn supports(&self, _: Format, _: Op) -> bool { true }
    fn list(&self, _: &Path, _: Option<&str>) -> EngineResult<ArchiveInfo> {
        Err(EngineError::WrongPassword)
    }
}

struct Works;
impl Engine for Works {
    fn id(&self) -> &'static str { "works" }
    fn kind(&self) -> qarc_core::models::EngineKind { qarc_core::models::EngineKind::Managed }
    fn supports(&self, _: Format, _: Op) -> bool { true }
    fn list(&self, archive: &Path, _: Option<&str>) -> EngineResult<ArchiveInfo> {
        Ok(ArchiveInfo { path: archive.to_string_lossy().into_owned(), engine: "works".into(), ..Default::default() })
    }
}

#[test]
fn fallback_replaces_only_engines_that_cannot_do_the_job() {
    let cands: Vec<&dyn Engine> = vec![&AlwaysFails, &Works];
    let got = with_fallback(&cands, |e| e.list(Path::new("x.7z"), None)).unwrap();
    assert_eq!(got.engine, "works");

    // A wrong password must surface, not trigger a retry somewhere else.
    let cands: Vec<&dyn Engine> = vec![&WrongPassword, &Works];
    let err = with_fallback(&cands, |e| e.list(Path::new("x.7z"), None)).unwrap_err();
    assert_eq!(err, EngineError::WrongPassword);
}

#[test]
fn registry_orders_by_priority_and_drops_unsupported() {
    let reg = Registry::new(vec![Box::new(TarEngine)]);
    assert_eq!(reg.candidates(Format::Tar, Op::Read).len(), 1);
    assert!(reg.candidates(Format::Rar, Op::Read).is_empty());
}

// -------------------------------------------------------------------- tar

fn write_tree(dir: &Path) -> PathBuf {
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("readme.md"), "hello 速壓".as_bytes()).unwrap();
    std::fs::write(src.join("docs").join("a.txt"), b"0123456789".repeat(10)).unwrap();
    src
}

#[test]
fn tar_gz_round_trip_is_byte_exact() {
    let dir = tempdir("case");
    let src = write_tree(dir.path());
    let archive = dir.path().join("bundle.tar.gz");

    TarEngine
        .create(
            &CompressOptions { archive: archive.to_string_lossy().into_owned(), sources: vec![src.to_string_lossy().into_owned()], format: Format::TarGz, ..Default::default() },
            &mut |_| {},
        )
        .unwrap();
    assert!(archive.exists());

    let info = TarEngine.list(&archive, None).unwrap();
    assert_eq!(info.entries.len(), 3, "two files and one directory");
    assert!(info.entries.iter().any(|e| e.is_dir));

    let out = dir.path().join("out");
    let req = ExtractRequest {
        archive: archive.to_string_lossy().into_owned(),
        dest: out.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let mut seen = 0usize;
    let done = TarEngine.extract(&req, &mut |_| seen += 1).unwrap();
    assert!(done.ok);
    assert!(done.bytes_out > 0);
    assert_eq!(std::fs::read(out.join("src").join("readme.md")).unwrap(), std::fs::read(src.join("readme.md")).unwrap());
    assert_eq!(
        std::fs::read(out.join("src").join("docs").join("a.txt")).unwrap(),
        std::fs::read(src.join("docs").join("a.txt")).unwrap()
    );
}

#[test]
fn a_tar_member_cannot_escape_the_destination() {
    let dir = tempdir("slip");
    let archive = dir.path().join("evil.tar");
    // Hand-built ustar blocks: the tar writer refuses "../" outright, so a real
    // attack archive could only come from another tool.  The reader must not
    // inherit that courtesy.
    std::fs::write(&archive, ustar(&[("../pwned.txt", &b"evil"[..]), ("good.txt", &b"ok"[..])])).unwrap();
    let out = dir.path().join("out");
    let req = ExtractRequest {
        archive: archive.to_string_lossy().into_owned(),
        dest: out.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let done = TarEngine.extract(&req, &mut |_| {}).unwrap();
    assert_eq!(done.blocked.len(), 1, "the escaping member is reported, not written");
    assert!(!dir.path().join("pwned.txt").exists());
    assert!(out.join("good.txt").exists(), "the harmless member still extracts");
}

/// One 512-byte ustar header per member plus zero-padded payload.
fn ustar(members: &[(&str, &[u8])]) -> Vec<u8> {
    fn octal(value: u64, width: usize) -> Vec<u8> {
        let mut v = format!("{value:0>width$o}").into_bytes();
        v.push(0x00);
        v
    }
    let mut out = Vec::new();
    for (name, data) in members {
        let mut block = vec![0u8; 512];
        block[0..name.len()].copy_from_slice(name.as_bytes());
        block[100..108].copy_from_slice(&octal(0o644, 7)); // mode
        block[108..116].copy_from_slice(&octal(0, 7)); // uid
        block[116..124].copy_from_slice(&octal(0, 7)); // gid
        block[124..136].copy_from_slice(&octal(data.len() as u64, 11)); // size
        block[136..148].copy_from_slice(&octal(1_700_000_000, 11)); // mtime
        block[156] = b'0'; // typeflag: regular file
        block[257..262].copy_from_slice(b"ustar");
        block[263..265].copy_from_slice(b"00");
        // The checksum is taken with its own field treated as spaces, which is the
        // part every hand-rolled tar writer gets wrong at least once.
        for byte in &mut block[148..156] {
            *byte = b' ';
        }
        let sum: u64 = block.iter().map(|b| *b as u64).sum();
        block[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
        block[154] = 0x00;
        block[155] = b' ';
        out.extend_from_slice(&block);
        out.extend_from_slice(data);
        let pad = (512 - data.len() % 512) % 512;
        out.extend(std::iter::repeat(0u8).take(pad));
    }
    out.extend(std::iter::repeat(0u8).take(1024)); // two zero blocks end the archive
    out
}

#[test]
fn selecting_a_member_that_is_not_there_is_an_error_not_a_silent_success() {
    let dir = tempdir("case");
    let src = write_tree(dir.path());
    let archive = dir.path().join("b.tar");
    TarEngine
        .create(
            &CompressOptions { archive: archive.to_string_lossy().into_owned(), sources: vec![src.to_string_lossy().into_owned()], format: Format::Tar, ..Default::default() },
            &mut |_| {},
        )
        .unwrap();
    let ops = qarc_core::Ops::with_registry(Registry::new(vec![Box::new(TarEngine)]));
    let out = dir.path().join("out");
    let req = ExtractRequest {
        archive: archive.to_string_lossy().into_owned(),
        dest: out.to_string_lossy().into_owned(),
        members: vec!["no-such-file.txt".into()],
        ..Default::default()
    };
    let done = ops.extract(&req, &mut |_| {}, None).unwrap();
    assert!(!done.ok, "0 items with an explicit filter must fail loudly");
    assert!(done.errors[0].contains("找不到"));
}

#[test]
fn skip_policy_leaves_existing_files_alone() {
    let dir = tempdir("case");
    let src = write_tree(dir.path());
    let archive = dir.path().join("b.tar.gz");
    TarEngine
        .create(
            &CompressOptions { archive: archive.to_string_lossy().into_owned(), sources: vec![src.to_string_lossy().into_owned()], format: Format::TarGz, ..Default::default() },
            &mut |_| {},
        )
        .unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir_all(out.join("src")).unwrap();
    std::fs::write(out.join("src").join("readme.md"), b"DO NOT TOUCH").unwrap();
    let req = ExtractRequest {
        archive: archive.to_string_lossy().into_owned(),
        dest: out.to_string_lossy().into_owned(),
        overwrite: Overwrite::Skip,
        ..Default::default()
    };
    TarEngine.extract(&req, &mut |_| {}).unwrap();
    assert_eq!(std::fs::read(out.join("src").join("readme.md")).unwrap(), b"DO NOT TOUCH");
}

#[test]
fn test_only_reads_without_writing() {
    let dir = tempdir("case");
    let src = write_tree(dir.path());
    let archive = dir.path().join("b.tar");
    TarEngine
        .create(
            &CompressOptions { archive: archive.to_string_lossy().into_owned(), sources: vec![src.to_string_lossy().into_owned()], format: Format::Tar, ..Default::default() },
            &mut |_| {},
        )
        .unwrap();
    let out = dir.path().join("never-created");
    let req = ExtractRequest {
        archive: archive.to_string_lossy().into_owned(),
        dest: out.to_string_lossy().into_owned(),
        test_only: true,
        ..Default::default()
    };
    let done = TarEngine.test(&req, &mut |_| {}).unwrap();
    assert!(done.ok);
    assert!(done.bytes_out > 0, "integrity test must actually read the bytes");
    assert!(!out.join("src").join("readme.md").exists(), "and must not write them out");
}

#[test]
fn entry_ratio_is_reported_only_when_it_is_real() {
    let packed = Entry { size: 1000, packed: Some(400), ..plain() };
    assert_eq!(packed.ratio(), Some(0.6));
    let stored = Entry { size: 0, packed: Some(0), ..plain() };
    assert_eq!(stored.ratio(), None);
    let unknown = Entry { size: 10, packed: None, ..plain() };
    assert_eq!(unknown.ratio(), None);
}

fn plain() -> Entry {
    Entry {
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
    }
}

#[test]
fn progress_struct_defaults_are_sane() {
    let p = Progress::default();
    assert_eq!(p.fraction, None);
    assert_eq!(p.done, 0);
    let o = Outcome::default();
    assert!(!o.ok);
    assert!(o.blocked.is_empty());
}

#[test]
fn sevenzip_stores_names_relative_to_the_common_parent() {
    // Without this rewrite `a out.7z /tmp/work/src` archives members as
    // /tmp/work/src/..., which is how an archive ends up with its whole source
    // path baked into it.
    let opts = CompressOptions {
        archive: "out.7z".into(),
        sources: vec!["/tmp/work/src".into()],
        format: Format::SevenZip,
        level: 5,
        threads: 0,
        password: None,
        header_encryption: false,
    };
    let (args, cwd) = sevenzip::create_plan(&opts);
    assert!(cwd.is_some(), "the child process must run from the parent of src");
    assert!(args.iter().any(|a| a == "src"), "{args:?}");
    assert!(!args.iter().any(|a| a.contains("work/src")), "{args:?}");
    assert!(args.iter().any(|a| a == "-t7z"));
}

#[test]
fn sevenzip_password_switches_stay_redacted_in_the_debug_echo() {
    let opts = CompressOptions {
        archive: "out.7z".into(),
        sources: vec!["a".into()],
        format: Format::SevenZip,
        password: Some("hunter2".into()),
        header_encryption: true,
        ..Default::default()
    };
    let (args, _) = sevenzip::create_plan(&opts);
    assert!(args.iter().any(|a| a == "-mhe=on"));
    // The password really is in argv: 7-Zip only accepts it inline, and stdin is
    // refused when -mhe=on is set.  That is 7-Zip's documented limitation, so the
    // guarantee we can make is about *our* output, not about the process list.
    assert!(args.iter().any(|a| a == "-phunter2"));
    assert!(!sevenzip::redact(&args).iter().any(|a| a.contains("hunter2")));
    assert!(sevenzip::redact(&args).iter().any(|a| a == "-p*******"));
}

#[test]
fn sevenzip_reports_the_member_count_it_actually_prints() {
    // Captured shape from `7zr a -bb1`, which prints "+ <path>" per file and one
    // summary line; the old code looked for "Adding", so a successful 20-file
    // archive was reported as 0 items.
    let stdout = "
Scanning the drive:
3 folders, 20 files, 19988608 bytes (20 MiB)

Creating archive: .bench/t2.7z

Add new data to archive: 3 folders, 20 files, 19988608 bytes (20 MiB)

+ .bench\\src\\mixed\\blob00.bin
+ .bench\\src\\mixed\\blob01.bin
";
    assert_eq!(sevenzip::added_files(stdout), 20);
    // Without the summary line, fall back to counting the + entries.
    assert_eq!(sevenzip::added_files("+ a
+ b
+ c
"), 3);
    assert_eq!(sevenzip::added_files(""), 0);
}

// ---------------------------------------------------------------- browse

fn entry(path: &str, size: u64, packed: u64) -> qarc_core::Entry {
    qarc_core::Entry {
        name: path.rsplit('/').next().unwrap_or(path).to_string(),
        path: path.to_string(),
        is_dir: path.ends_with('/'),
        size,
        packed: (packed > 0).then_some(packed),
        mtime: None,
        attrs: String::new(),
        encrypted: false,
        codec: None,
        unsafe_path: false,
    }
}

#[test]
fn children_never_lists_the_folder_you_are_already_in() {
    // The .NET build indexed past the end on exactly this row.
    let entries = vec![entry("報告/", 0, 0), entry("報告/a.txt", 10, 4), entry("b.txt", 5, 5)];
    let root = qarc_core::children(&entries, "");
    assert_eq!(root.len(), 2, "{:?}", root.iter().map(|e| &e.path).collect::<Vec<_>>());
    assert!(root.iter().all(|e| e.path != ""), "no self row at root");

    let inside = qarc_core::children(&entries, "報告/");
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0].name, "a.txt");
    assert_eq!(inside[0].path, "報告/a.txt", "the child keeps its full internal path");
}

#[test]
fn same_named_folders_are_not_merged_and_are_not_duplicated() {
    let entries = vec![
        entry("a/報告/x.bin", 100, 40),
        entry("a/報告/y.bin", 50, 10),
        entry("b/報告/z.bin", 7, 7),
        entry("a/報告/", 0, 0),
    ];
    let root = qarc_core::children(&entries, "");
    let dirs: Vec<&str> = root.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(dirs, vec!["a/", "b/"], "{dirs:?}");
    assert_eq!(root[0].name, "a", "one segment deep, so the row is a/");

    // Two different folders both named 報告 must stay two rows, each with its own
    // aggregated size -- keying by basename would have merged them into one.
    let under_a = qarc_core::children(&entries, "a/");
    let under_b = qarc_core::children(&entries, "b/");
    assert_eq!(under_a.len(), 1);
    assert_eq!(under_a[0].path, "a/報告/");
    assert_eq!(under_a[0].name, "報告");
    assert_eq!(under_a[0].size, 150, "folder size is the sum of its descendants");
    assert_eq!(under_a[0].packed, Some(50));
    assert_eq!(under_b[0].path, "b/報告/");
    assert_eq!(under_b[0].size, 7);
}

#[test]
fn folders_sort_before_files_regardless_of_name() {
    let entries = vec![entry("zzz.txt", 1, 1), entry("aaa/", 0, 0), entry("aaa/in.txt", 2, 2), entry("mmm.txt", 3, 3)];
    let root = qarc_core::children(&entries, "");
    let names: Vec<&str> = root.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["aaa", "mmm.txt", "zzz.txt"], "{names:?}");
}

#[test]
fn a_zip_inside_a_zip_can_be_opened_and_listed() {
    let dir = tempdir("nested");
    let inner = dir.path().join("inner.zip");
    {
        let mut w = zip::ZipWriter::new(std::fs::File::create(&inner).unwrap());
        w.start_file("deep/hello.txt", zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut w, b"nested payload").unwrap();
        w.finish().unwrap();
    }
    let outer = dir.path().join("outer.zip");
    {
        let mut w = zip::ZipWriter::new(std::fs::File::create(&outer).unwrap());
        w.start_file("bundle/inner.zip", zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut w, &std::fs::read(&inner).unwrap()).unwrap();
        w.start_file("readme.txt", zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut w, b"top").unwrap();
        w.finish().unwrap();
    }

    let ops = qarc_core::Ops::with_registry(Registry::new(vec![Box::new(ZipEngine)]));
    let (path, info) = qarc_core::open_nested(&ops, &outer, "bundle/inner.zip", None).unwrap();
    assert!(path.is_file(), "the inner archive is materialised in the scratch folder");
    assert_eq!(info.entries.len(), 1);
    assert_eq!(info.entries[0].path, "deep/hello.txt");

    let missing = qarc_core::open_nested(&ops, &outer, "nope.zip", None).unwrap_err();
    assert!(matches!(missing, EngineError::Corrupt(_)), "{missing:?}");
}

#[test]
fn an_escaping_member_is_shown_in_full_and_not_grouped_as_dotdot() {
    let entries = vec![
        entry("../../evil/", 0, 0),
        entry("../../evil/pwned.txt", 12, 12),
        entry("ok/readme.md", 30, 10),
    ];
    let root = qarc_core::children(&entries, "");
    let names: Vec<&str> = root.iter().map(|e| e.name.as_str()).collect();
    assert!(!names.iter().any(|n| *n == ".."), "{names:?} must not offer fake navigation");
    let escaping: Vec<&str> = root
        .iter()
        .filter(|e| e.unsafe_path)
        .map(|e| e.path.as_str())
        .collect();
    assert_eq!(escaping, vec!["../../evil/", "../../evil/pwned.txt"], "{escaping:?}");
    assert_eq!(root.iter().filter(|e| e.is_dir && !e.unsafe_path).count(), 1, "ok/ stays a normal folder");
}

#[test]
fn a_cancelled_run_reports_partial_work_not_failure() {
    let dir = tempdir("cancel");
    let src = write_tree(dir.path());
    let archive = dir.path().join("b.tar.gz");
    TarEngine
        .create_cancellable(
            &CompressOptions {
                archive: archive.to_string_lossy().into_owned(),
                sources: vec![src.to_string_lossy().into_owned()],
                format: Format::TarGz,
                ..Default::default()
            },
            &mut |_| {},
            None,
        )
        .unwrap();

    let token = qarc_core::CancelToken::new();
    token.cancel();
    let out = dir.path().join("out");
    let req = ExtractRequest {
        archive: archive.to_string_lossy().into_owned(),
        dest: out.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let done = TarEngine
        .extract_cancellable(&req, &mut |_| {}, Some(&token))
        .unwrap();
    assert!(done.cancelled, "the flag is how the UI shows 已取消 instead of 錯誤");
    assert!(!done.ok);
    assert!(done.errors.is_empty(), "{:?}", done.errors);
}

#[test]
fn ops_extract_and_create_take_a_cancel_token() {
    let dir = tempdir("canceltoken");
    let src = write_tree(dir.path());
    let archive = dir.path().join("b.tar");
    let ops = qarc_core::Ops::with_registry(Registry::new(vec![Box::new(TarEngine)]));
    let made = ops
        .create(
            &CompressOptions {
                archive: archive.to_string_lossy().into_owned(),
                sources: vec![src.to_string_lossy().into_owned()],
                format: Format::Tar,
                ..Default::default()
            },
            &mut |_| {},
            None,
        )
        .unwrap();
    assert!(made.ok);

    let out = dir.path().join("out");
    let req = ExtractRequest {
        archive: archive.to_string_lossy().into_owned(),
        dest: out.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let done = ops.extract(&req, &mut |_| {}, None).unwrap();
    assert!(done.ok && !done.cancelled);
}

#[test]
fn sevenzip_directory_entries_are_folders_even_without_a_trailing_slash() {
    // Measured from `7zr l -slt`: the Path line has no trailing separator and the
    // folder flag lives in Attributes.  Reading only the slash duplicated every
    // folder as a second 0-byte row beside the aggregated one.
    let listing = "\
Path = src
Size = 0
Attributes = D
Modified = 2026-11-05 18:42:10

Path = src/readme.md
Size = 5860
Attributes = A

Path = src/docs
Size = 0
Attributes = D

Path = src/docs/a.bin
Size = 200000
Attributes = A
";
    let entries = sevenzip::parse_listing(listing);
    assert_eq!(entries.len(), 4);
    assert!(entries[0].is_dir, "Attributes D must win over the missing slash");
    assert!(entries[0].path.ends_with('/'), "{}", entries[0].path);
    assert!(!entries[1].is_dir);
    assert!(entries[2].path.ends_with('/'), "{}", entries[2].path);

    let root = qarc_core::children(&entries, "");
    let names: Vec<&str> = root.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["src"], "{names:?} must not repeat the folder");
    assert_eq!(root[0].size, 205860, "aggregated across both descendants");

    let inside = qarc_core::children(&entries, "src/");
    let inside_names: Vec<&str> = inside.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(inside_names, vec!["docs", "readme.md"], "{inside_names:?}");
}
