//! Tests for the managed ZIP engine and the streaming hasher.  Kept in their own
//! file so a failure here is not tangled with the tar/pathguard suite.

use qarc_core::engine::{Engine, EngineError, EngineResult, Op, Registry};
use qarc_core::formats::Format;
use qarc_core::models::{ArchiveInfo, CompressOptions, EngineKind, ExtractRequest};
use qarc_core::tar::TarEngine;
use qarc_core::zip_engine::ZipEngine;
use std::io::Write;
use std::path::{Path, PathBuf};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let mut root = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        root.push(format!("qarc-test-{tag}-{stamp}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        TempDir(root)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tree(dir: &Path) -> PathBuf {
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("readme.md"), "hello 速壓".as_bytes()).unwrap();
    std::fs::write(src.join("docs").join("a.txt"), vec![b'x'; 1000]).unwrap();
    src
}

fn zip_opts(archive: &Path, src: &Path) -> CompressOptions {
    CompressOptions {
        archive: archive.to_string_lossy().into_owned(),
        sources: vec![src.to_string_lossy().into_owned()],
        format: Format::Zip,
        level: 6,
        ..Default::default()
    }
}

#[test]
fn zip_round_trip_keeps_bytes_and_dir_flags() {
    let dir = TempDir::new("zip");
    let src = tree(dir.path());
    let archive = dir.path().join("bundle.zip");
    ZipEngine.create(&zip_opts(&archive, &src), &mut |_| {}).unwrap();

    let info = ZipEngine.list(&archive, None).unwrap();
    assert_eq!(info.entries.len(), 3);
    assert!(info.entries.iter().any(|e| e.is_dir));
    assert!(
        info.total_packed < info.total_size,
        "deflate must actually shrink the repeated block ({} -> {})",
        info.total_size,
        info.total_packed
    );

    let out = dir.path().join("out");
    let req = ExtractRequest {
        archive: archive.to_string_lossy().into_owned(),
        dest: out.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let done = ZipEngine.extract(&req, &mut |_| {}).unwrap();
    assert!(done.ok && done.blocked.is_empty());
    assert_eq!(
        std::fs::read(out.join("src").join("docs").join("a.txt")).unwrap(),
        std::fs::read(src.join("docs").join("a.txt")).unwrap()
    );
    assert_eq!(
        std::fs::read(out.join("src").join("readme.md")).unwrap(),
        "hello 速壓".as_bytes()
    );
}

#[test]
fn a_zip_member_cannot_escape_the_destination() {
    let dir = TempDir::new("zipslip");
    let archive = dir.path().join("evil.zip");
    {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        // The writer stores whatever name it is handed; only the reader is
        // expected to guard, which is exactly the assumption zip-slip attacks.
        writer
            .start_file("../pwned.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"evil").unwrap();
        writer
            .start_file("good.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"ok").unwrap();
        writer.finish().unwrap();
    }
    let out = dir.path().join("out");
    let req = ExtractRequest {
        archive: archive.to_string_lossy().into_owned(),
        dest: out.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let done = ZipEngine.extract(&req, &mut |_| {}).unwrap();
    assert_eq!(done.blocked.len(), 1, "one escaping member, reported not written");
    assert!(!dir.path().join("pwned.txt").exists());
    assert!(out.join("good.txt").exists());

    let info = ZipEngine.list(&archive, None).unwrap();
    assert!(info.entries.iter().any(|e| e.unsafe_path), "listing flags it before extracting");
}

struct NoSevenZip;

impl Engine for NoSevenZip {
    fn id(&self) -> &'static str {
        "no7z"
    }
    fn kind(&self) -> EngineKind {
        EngineKind::CommandLine
    }
    fn supports(&self, _: Format, _: Op) -> bool {
        true
    }
    fn list(&self, _: &Path, _: Option<&str>) -> EngineResult<ArchiveInfo> {
        Err(EngineError::EngineUnavailable("7-Zip 未安裝".into()))
    }
}

#[test]
fn encrypted_zip_falls_through_instead_of_guessing() {
    let dir = TempDir::new("zippw");
    let archive = dir.path().join("locked.zip");
    std::fs::write(&archive, b"PK\x03\x04 placeholder").unwrap();
    let ops = qarc_core::Ops::with_registry(Registry::new(vec![Box::new(ZipEngine), Box::new(NoSevenZip)]));
    let err = ops.list(&archive, Some("pw")).unwrap_err();
    assert!(err.is_capability_fault(), "{err:?} must be a capability fault");
    assert!(format!("{err}").contains("7-Zip"), "the message names the missing engine");
}

#[test]
fn zip_listing_reports_sizes_that_are_not_the_file_length() {
    let dir = TempDir::new("zipmeta");
    let src = dir.path().join("one.bin");
    std::fs::write(&src, vec![0u8; 100_000]).unwrap();
    let archive = dir.path().join("zeros.zip");
    ZipEngine.create(&zip_opts(&archive, &src), &mut |_| {}).unwrap();
    let info = ZipEngine.list(&archive, None).unwrap();
    let entry = &info.entries[0];
    assert_eq!(entry.size, 100_000);
    assert!(entry.packed.unwrap() < 2_000, "100 KB of zeros must store tiny");
}

#[test]
fn streaming_hash_matches_known_vectors() {
    let dir = TempDir::new("hash");
    let path = dir.path().join("abc.bin");
    std::fs::write(&path, b"abc").unwrap();
    assert_eq!(
        qarc_core::digest_file(&path, qarc_core::Algo::Sha256).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        qarc_core::digest_file(&path, qarc_core::Algo::Md5).unwrap(),
        "900150983cd24fb0d6963f7d28e17f72"
    );
    // Past the 64 KiB buffer so the streaming path is really exercised.
    let big = dir.path().join("big.bin");
    std::fs::write(&big, vec![7u8; 200_000]).unwrap();
    let hex = qarc_core::digest_file(&big, qarc_core::Algo::Sha256).unwrap();
    assert_eq!(hex.len(), 64, "sha256 hex is 64 characters, not 40");
    assert_eq!(qarc_core::Algo::Sha256.label(), "SHA-256");
}

/// The lifecycle the Tauri command runs: register, stream progress, finish.
/// A job that cannot be observed cannot be cancelled, so this is the contract
/// the UI depends on.
#[test]
fn a_job_lifecycle_streams_progress_and_can_be_cancelled_midway() {
    use qarc_core::{JobState, Jobs};

    let dir = TempDir::new("lifecycle");
    let src = tree(dir.path());
    let archive = dir.path().join("many.tar");
    // Enough members that a cancel actually has something to stop.
    let big = dir.path().join("many");
    std::fs::create_dir_all(&big).unwrap();
    for i in 0..400 {
        std::fs::write(big.join(format!("f{i:03}.dat")), vec![b'a' + (i % 26) as u8; 512]).unwrap();
    }

    let ops = qarc_core::Ops::with_registry(Registry::new(vec![Box::new(TarEngine)]));
    let jobs = Jobs::default();
    let (id, token) = jobs.begin("extract", "many.tar".into(), "解壓全部".into()).unwrap();

    let dest = dir.path().join("out");
    let req = qarc_core::ExtractRequest {
        archive: archive.to_string_lossy().into_owned(),
        dest: dest.to_string_lossy().into_owned(),
        ..Default::default()
    };
    // Create first with the engine directly so the job under test is the extract.
    TarEngine
        .create(
            &qarc_core::CompressOptions {
                archive: archive.to_string_lossy().into_owned(),
                sources: vec![big.to_string_lossy().into_owned()],
                format: qarc_core::Format::Tar,
                ..Default::default()
            },
            &mut |_| {},
        )
        .unwrap();

    let seen = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    {
        let seen = seen.clone();
        let done = ops
            .extract(&req, &mut |p| {
                *seen.lock().unwrap() += 1;
                jobs.report(id, &p);
            }, None)
            .unwrap();
        assert!(done.ok);
        jobs.finish(id, JobState::Done, done.items, done.bytes_out, done.blocked.len(), None);
    }
    let snap = jobs.snapshot();
    assert_eq!(snap[0].state, JobState::Done);
    assert_eq!(snap[0].percent, 100.0);
    assert_eq!(snap[0].items, 400, "every member must be counted, not just the last");
    assert!(*seen.lock().unwrap() > 0, "progress reached the registry");

    // Now the same job with the token pre-cancelled: partial work, not failure.
    let dest2 = dir.path().join("out2");
    let req2 = qarc_core::ExtractRequest { dest: dest2.to_string_lossy().into_owned(), ..req.clone() };
    let (id2, token2) = jobs.begin("extract", "many.tar".into(), String::new()).unwrap();
    jobs.finish(id, JobState::Done, 0, 0, 0, None); // free the slot
    token2.cancel();
    let partial = ops.extract(&req2, &mut |_| {}, Some(&token2)).unwrap();
    assert!(partial.cancelled && !partial.ok);
    jobs.finish(id2, JobState::Cancelled, partial.items, partial.bytes_out, 0, Some("已取消".into()));
    let after: Vec<_> = jobs.snapshot().iter().map(|j| j.state).collect();
    assert!(after.contains(&JobState::Cancelled), "{after:?}");
    assert!(!jobs.cancel(id2), "a finished job has nothing left to cancel");
    let _ = src;
}
