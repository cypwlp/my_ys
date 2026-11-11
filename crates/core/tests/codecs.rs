//! Tests for the bzip2 / xz / zstd layers.
//!
//! Fixtures are generated *in the same language* as the code under test: `bzip2`
//! and `lzma-rs` both ship pure-Rust encoders, and a zstd frame made of raw blocks
//! is legal RFC 8878 -- so no C tooling and no downloaded blobs are needed to prove
//! the decoders are actually wired up.

use qarc_core::codec;
use qarc_core::formats::{self, Format};
use qarc_core::tar::TarEngine;
use qarc_core::{Engine, ExtractRequest, Format as Fmt};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Temp {
        let mut root = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        root.push(format!("qarc-codec-{tag}-{stamp}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        Temp(root)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A zstd frame holding one raw block: valid per RFC 8878, and enough to prove the
/// decoder is actually invoked (a stub would return the header bytes).
fn zstd_raw_frame(payload: &[u8]) -> Vec<u8> {
    const MAX_BLOCK: usize = 128 * 1024; // RFC 8878 caps a block at 128 KiB
    let mut out = Vec::new();
    out.extend_from_slice(&[0x28, 0xb5, 0x2f, 0xfd]); // magic
    out.push(0x00); // frame header descriptor: no content size, no checksum
    // Window descriptor: exponent 10 -> 1 MiB, which comfortably covers a
    // 128 KiB raw block.  A smaller window makes the decoder reject the frame.
    out.push(0x50);
    let mut rest = payload;
    loop {
        let take = rest.len().min(MAX_BLOCK);
        let (chunk, tail) = rest.split_at(take);
        rest = tail;
        let last = rest.is_empty();
        let header = (take as u32) << 3 | u32::from(last); // type raw = 0
        out.extend_from_slice(&header.to_le_bytes()[..3]);
        out.extend_from_slice(chunk);
        if last {
            break;
        }
    }
    out
}

fn xz_compress(data: &[u8]) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(data.to_vec());
    let mut out = Vec::new();
    lzma_rs::xz_compress(&mut cursor, &mut out).expect("xz compress");
    out
}

fn bzip2_compress(data: &[u8], level: u32) -> Vec<u8> {
    let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::new(level));
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

#[test]
fn bzip2_round_trip_through_the_codec_layer() {
    let dir = Temp::new("bz2");
    let payload = "bzip2 me ".repeat(2000);
    let src = dir.path().join("a.bz2");
    std::fs::write(&src, bzip2_compress(payload.as_bytes(), 9)).unwrap();

    assert_eq!(formats::from_extension("a.bz2"), Format::BZip2);
    assert_eq!(formats::sniff(&src), Format::BZip2, "the BZh magic must be trusted");

    let out = dir.path().join("a");
    let written = codec::decode_to_file(&src, &out, Format::BZip2).unwrap();
    assert_eq!(written as usize, payload.len());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), payload);
}

#[test]
fn xz_round_trip_through_the_codec_layer() {
    let dir = Temp::new("xz");
    let payload: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
    let src = dir.path().join("a.xz");
    std::fs::write(&src, xz_compress(&payload)).unwrap();

    assert_eq!(formats::sniff(&src), Format::Xz, "the xz magic must be trusted");
    let out = dir.path().join("a");
    let written = codec::decode_to_file(&src, &out, Format::Xz).unwrap();
    assert_eq!(written as usize, payload.len(), "decoded size must match exactly");
    assert_eq!(std::fs::read(&out).unwrap(), payload);
}

#[test]
fn zstd_frame_decodes_through_the_codec_layer() {
    let dir = Temp::new("zst");
    let payload = b"ruzstd must read a raw block, not echo the header".to_vec();
    let src = dir.path().join("a.zst");
    std::fs::write(&src, zstd_raw_frame(&payload)).unwrap();

    assert_eq!(formats::sniff(&src), Format::Zstd);
    let out = dir.path().join("a");
    let written = codec::decode_to_file(&src, &out, Format::Zstd).unwrap();
    assert_eq!(written as usize, payload.len());
    assert_eq!(std::fs::read(&out).unwrap(), payload);
}

/// Builds a `.tar.<layer>` from one file, so the container path is exercised and
/// not just the raw codec.
fn make_tarball(dir: &Path, name: &str, layer: Format, payload: &[u8]) -> PathBuf {
    let mut raw = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut raw);
        let mut h = tar::Header::new_ustar();
        h.set_size(payload.len() as u64);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_mode(0o644);
        builder.append_data(&mut h, "inner/report.txt", &mut &payload[..]).unwrap();
        builder.finish().unwrap();
    }
    let path = dir.join(name);
    let bytes = match layer {
        Format::BZip2 => bzip2_compress(&raw, 9),
        Format::Xz => xz_compress(&raw),
        Format::Zstd => zstd_raw_frame(&raw),
        Format::GZip => {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(&raw).unwrap();
            e.finish().unwrap()
        }
        other => panic!("no such layer {other:?}"),
    };
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn every_tar_container_layer_lists_and_extracts_the_same_bytes() {
    let payload = "the same text, compressed four different ways\n".repeat(3000);
    for (layer, ext, fmt) in [
        (Format::BZip2, "tar.bz2", Format::TarBz2),
        (Format::Xz, "tar.xz", Format::TarXz),
        (Format::Zstd, "tar.zst", Format::TarZst),
        (Format::GZip, "tar.gz", Format::TarGz),
    ] {
        let dir = Temp::new(ext);
        let archive = make_tarball(dir.path(), &format!("a.{ext}"), layer, payload.as_bytes());
        assert_eq!(formats::from_extension(&format!("a.{ext}")), fmt, "{ext} detection");

        let info = TarEngine.list(&archive, None).unwrap_or_else(|e| panic!("{ext} list: {e}"));
        assert_eq!(info.entries.len(), 1, "{ext} listing");
        assert_eq!(info.entries[0].size as usize, payload.len(), "{ext} size");

        let out = dir.path().join("out");
        let req = ExtractRequest {
            archive: archive.to_string_lossy().into_owned(),
            dest: out.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let done = TarEngine.extract(&req, &mut |_| {}).unwrap_or_else(|e| panic!("{ext} extract: {e}"));
        assert!(done.ok, "{ext} extract ok");
        assert_eq!(
            std::fs::read(out.join("inner").join("report.txt")).unwrap(),
            payload.as_bytes(),
            "{ext} bytes"
        );
    }
}

#[test]
fn tar_writes_the_layers_that_shrink_and_refuses_xz() {
    let dir = Temp::new("write");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("note.txt"), b"bz2 written by us".repeat(500)).unwrap();

    let archive = dir.path().join("a.tar.bz2");
    TarEngine
        .create(
            &qarc_core::CompressOptions {
                archive: archive.to_string_lossy().into_owned(),
                sources: vec![src.to_string_lossy().into_owned()],
                format: Format::TarBz2,
                ..Default::default()
            },
            &mut |_| {},
        )
        .unwrap();
    let info = TarEngine.list(&archive, None).unwrap();
    assert_eq!(info.entries.len(), 1);

    // The engine must refuse what it cannot write, instead of writing a file that
    // is labelled one format and encoded as another.
    let err = TarEngine
        .create(
            &qarc_core::CompressOptions {
                archive: dir.path().join("nope.tar.xz").to_string_lossy().into_owned(),
                sources: vec![src.to_string_lossy().into_owned()],
                format: Format::TarXz,
                ..Default::default()
            },
            &mut |_| {},
        )
        .unwrap_err();
    assert!(
        matches!(err, qarc_core::EngineError::UnsupportedFormat(_)),
        "xz creation must be refused, not silently produce a bigger file: {err:?}"
    );
}

#[test]
fn the_format_table_matches_what_the_pure_rust_layers_can_actually_do() {
    // A stale table is how "支援 xz" ends up meaning "cannot read it".
    assert!(formats::info(Format::BZip2).writable, "bzip2 has a pure-Rust encoder");
    assert!(formats::info(Format::TarBz2).writable);
    // ruzstd's Fastest level really shrinks: 208,000 -> 220, measured.
    assert!(formats::info(Format::Zstd).writable, "zstd write is real");
    assert!(formats::info(Format::TarZst).writable);
    // lzma-rs xz_compress measured 208,000 -> 208,064, i.e. no compression at all.
    assert!(!formats::info(Format::Xz).writable, "xz must not be offered as a writer");
    assert!(!formats::info(Format::TarXz).writable);
    for f in [Format::BZip2, Format::Xz, Format::Zstd, Format::TarBz2, Format::TarXz, Format::TarZst] {
        assert!(formats::info(f).readable, "{f} must be readable");
    }
    assert_eq!(Fmt::TarBz2.label(), "tar.bz2");
}

#[test]
fn a_truncated_xz_stream_is_an_error_not_a_short_file() {
    let dir = Temp::new("trunc");
    let payload: Vec<u8> = (0..50_000).map(|i| i as u8).collect();
    let good = xz_compress(&payload);
    let path = dir.path().join("cut.xz");
    std::fs::write(&path, &good[..good.len() - 12]).unwrap();
    let out = dir.path().join("cut");
    let err = codec::decode_to_file(&path, &out, Format::Xz).unwrap_err();
    assert!(matches!(err, qarc_core::EngineError::Corrupt(_)), "{err:?}");
}

#[test]
fn reading_a_container_leaves_no_temp_file_behind() {
    let dir = Temp::new("clean");
    let payload = b"temp hygiene".to_vec();
    let archive = make_tarball(dir.path(), "a.tar.xz", Format::Xz, &payload);

    // Assert on *this* archive's scratch path.  Scanning the whole temp directory
    // would be non-hermetic: files left by an older, broken build would fail the
    // test even after the leak is fixed -- which is exactly what happened once.
    let scratch = codec::scratch_path(&archive, ".decoded.tar");
    assert!(!scratch.exists(), "not decoded yet");

    let info = TarEngine.list(&archive, None).unwrap();
    assert_eq!(info.entries.len(), 1);
    assert!(!scratch.exists(), "{:?} the decoded temp must be dropped", scratch);

    let out = dir.path().join("out");
    TarEngine
        .extract(
            &ExtractRequest {
                archive: archive.to_string_lossy().into_owned(),
                dest: out.to_string_lossy().into_owned(),
                ..Default::default()
            },
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(std::fs::read(out.join("inner").join("report.txt")).unwrap(), payload);
    assert!(!scratch.exists(), "{:?} extract must clean up too", scratch);
}

#[test]
fn a_bare_gz_bz2_xz_zst_file_lists_one_member_and_decompresses_it() {
    use qarc_core::codec_engine::CodecEngine;

    let payload = "one stream, no container\n".repeat(4000);
    let cases: Vec<(Format, String, Vec<u8>)> = vec![
        (
            Format::GZip,
            "notes.txt.gz".into(),
            {
                let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                std::io::Write::write_all(&mut e, payload.as_bytes()).unwrap();
                e.finish().unwrap()
            },
        ),
        (Format::BZip2, "notes.txt.bz2".into(), bzip2_compress(payload.as_bytes(), 9)),
        (Format::Xz, "notes.txt.xz".into(), xz_compress(payload.as_bytes())),
        (Format::Zstd, "notes.txt.zst".into(), zstd_raw_frame(payload.as_bytes())),
    ];

    for (fmt, name, bytes) in cases {
        let dir = Temp::new("bare");
        let path = dir.path().join(&name);
        std::fs::write(&path, bytes).unwrap();

        let info = CodecEngine.list(&path, None).unwrap_or_else(|e| panic!("{fmt}: {e}"));
        assert_eq!(info.entries.len(), 1, "{fmt} must show exactly one member");
        assert_eq!(info.entries[0].name, "notes.txt", "{fmt} suffix must be stripped");
        assert_eq!(info.format, fmt);

        let out = dir.path().join("out");
        let done = CodecEngine
            .extract(
                &ExtractRequest {
                    archive: path.to_string_lossy().into_owned(),
                    dest: out.to_string_lossy().into_owned(),
                    ..Default::default()
                },
                &mut |_| {},
            )
            .unwrap_or_else(|e| panic!("{fmt} extract: {e}"));
        assert_eq!(done.items, 1);
        assert_eq!(done.bytes_out as usize, payload.len(), "{fmt} decoded size");
        assert_eq!(std::fs::read(out.join("notes.txt")).unwrap(), payload.as_bytes(), "{fmt} bytes");
    }
}

#[test]
fn gzip_reports_the_true_decoded_size_and_the_others_admit_they_cannot_know() {
    use qarc_core::codec_engine::CodecEngine;
    let dir = Temp::new("isize");
    let payload = vec![b'q'; 70_000];

    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut gz, &payload).unwrap();
    let gzpath = dir.path().join("q.gz");
    std::fs::write(&gzpath, gz.finish().unwrap()).unwrap();
    let info = CodecEngine.list(&gzpath, None).unwrap();
    assert_eq!(info.entries[0].size, 70_000, "gzip ISIZE is exact and free to read");

    let xzpath = dir.path().join("q.xz");
    std::fs::write(&xzpath, xz_compress(&payload)).unwrap();
    let info = CodecEngine.list(&xzpath, None).unwrap();
    assert_eq!(info.entries[0].size, 0, "xz has no cheap trailer; claiming a guess would be a lie");
    assert!(info.entries[0].packed.unwrap() > 0, "the compressed size is still real");
}

#[test]
fn the_codec_engine_refuses_passwords_and_unknown_formats() {
    use qarc_core::codec_engine::CodecEngine;
    let dir = Temp::new("refuse");
    let path = dir.path().join("a.gz");
    std::fs::write(&path, b"\x1f\x8b rest").unwrap();
    assert!(matches!(
        CodecEngine.list(&path, Some("pw")),
        Err(qarc_core::EngineError::UnsupportedFormat(_))
    ));

    let plain = dir.path().join("a.txt");
    std::fs::write(&plain, b"not compressed").unwrap();
    // Unknown-to-this-engine must fall through to the next candidate, not fail hard.
    assert!(matches!(
        CodecEngine.list(&plain, None),
        Err(qarc_core::EngineError::UnsupportedFormat(_))
    ));
    assert_eq!(qarc_core::codec_engine::stripped_name(Path::new("C:/x/archive.tar.gz")), "archive.tar");
}

#[test]
fn every_tar_layer_that_actually_shrinks_can_be_created_and_read_back() {
    let dir = Temp::new("tarwrite");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("note.txt"), "repeat me ".repeat(5000)).unwrap();

    for fmt in [Format::TarZst, Format::TarBz2, Format::TarGz] {
        let archive = dir.path().join(format!("a.{}", formats::info(fmt).extension));
        TarEngine
            .create(
                &qarc_core::CompressOptions {
                    archive: archive.to_string_lossy().into_owned(),
                    sources: vec![src.to_string_lossy().into_owned()],
                    format: fmt,
                    level: 6,
                    ..Default::default()
                },
                &mut |_| {},
            )
            .unwrap_or_else(|e| panic!("{fmt} create: {e}"));
        assert!(archive.exists() && archive.metadata().unwrap().len() > 0, "{fmt} wrote nothing");

        let info = TarEngine.list(&archive, None).unwrap_or_else(|e| panic!("{fmt} list: {e}"));
        assert_eq!(info.entries.len(), 1, "{fmt} entries");

        let out = dir.path().join(format!("out-{}", formats::info(fmt).extension));
        TarEngine
            .extract(
                &ExtractRequest {
                    archive: archive.to_string_lossy().into_owned(),
                    dest: out.to_string_lossy().into_owned(),
                    ..Default::default()
                },
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(
            std::fs::read(out.join("src").join("note.txt")).unwrap(),
            "repeat me ".repeat(5000).as_bytes(),
            "{fmt} bytes"
        );
    }
}

#[test]
fn tar_compression_level_changes_the_encoded_size() {
    let dir = Temp::new("tarlevel");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    let payload = "QuickArc compression level probe 0123456789\n".repeat(12_000);
    std::fs::write(src.join("payload.txt"), payload).unwrap();

    for fmt in [Format::TarGz, Format::TarBz2] {
        let ext = formats::info(fmt).extension;
        let mut sizes = Vec::new();
        for level in [1, 9] {
            let archive = dir.path().join(format!("a-{level}.{ext}"));
            TarEngine
                .create(
                    &qarc_core::CompressOptions {
                        archive: archive.to_string_lossy().into_owned(),
                        sources: vec![src.to_string_lossy().into_owned()],
                        format: fmt,
                        level,
                        ..Default::default()
                    },
                    &mut |_| {},
                )
                .unwrap_or_else(|e| panic!("{fmt} level {level}: {e}"));
            sizes.push(archive.metadata().unwrap().len());
        }
        assert!(
            sizes[1] < sizes[0],
            "{fmt} level 9 ({}) should beat level 1 ({})",
            sizes[1],
            sizes[0]
        );
    }
}

#[test]
fn the_staging_temp_used_by_tar_zst_is_removed() {
    let dir = Temp::new("stage");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("f.txt"), b"x".repeat(50_000)).unwrap();
    let archive = dir.path().join("a.tar.zst");
    let staged = codec::scratch_path(&archive, ".staged.tar");

    TarEngine
        .create(
            &qarc_core::CompressOptions {
                archive: archive.to_string_lossy().into_owned(),
                sources: vec![src.to_string_lossy().into_owned()],
                format: Format::TarZst,
                ..Default::default()
            },
            &mut |_| {},
        )
        .unwrap();
    assert!(!staged.exists(), "{staged:?} the two-pass staging file must be cleaned up");
}

#[test]
fn standalone_compression_round_trips_and_refuses_more_than_one_source() {
    use qarc_core::codec_engine::CodecEngine;

    let dir = Temp::new("alone");
    let input = dir.path().join("data.txt");
    let payload = "single stream compression\n".repeat(8000);
    std::fs::write(&input, payload.as_bytes()).unwrap();

    for (fmt, ext) in [
        (Format::GZip, "gz"),
        (Format::BZip2, "bz2"),
        (Format::Zstd, "zst"),
    ] {
        let archive = dir.path().join(format!("data.txt.{ext}"));
        CodecEngine
            .create(
                &qarc_core::CompressOptions {
                    archive: archive.to_string_lossy().into_owned(),
                    sources: vec![input.to_string_lossy().into_owned()],
                    format: fmt,
                    level: 6,
                    ..Default::default()
                },
                &mut |_| {},
            )
            .unwrap_or_else(|e| panic!("{fmt} create: {e}"));
        let compressed = archive.metadata().unwrap().len();
        assert!(compressed < payload.len() as u64, "{fmt} should shrink repetitive text ({compressed})");

        let out = dir.path().join(format!("out-{ext}"));
        let done = CodecEngine
            .extract(
                &ExtractRequest {
                    archive: archive.to_string_lossy().into_owned(),
                    dest: out.to_string_lossy().into_owned(),
                    ..Default::default()
                },
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(done.items, 1);
        assert_eq!(std::fs::read(out.join("data.txt")).unwrap(), payload.as_bytes(), "{fmt} bytes");
    }

    let xz = dir.path().join("data.txt.xz");
    let err = codec::encode_file(&input, &xz, Format::Xz, 6).unwrap_err();
    assert!(
        format!("{err}").contains("純 Rust 編碼器"),
        "the refusal must say why, not just fail: {err}"
    );
    // A refused attempt must leave nothing behind.  It used to truncate the target
    // first, and the 7-Zip fallback then opened that 0-byte file as an existing
    // archive and reported the whole job as 壓縮包損毀.
    assert!(
        !xz.exists(),
        "refusing to write xz must not create {}",
        xz.display()
    );
    assert!(
        !CodecEngine.supports(Format::Xz, qarc_core::Op::Write),
        "the engine must not advertise an xz writer it does not have"
    );
    assert!(
        CodecEngine.supports(Format::Xz, qarc_core::Op::Read),
        "xz still has to read, otherwise the refusal costs a working decoder"
    );

    // Two sources cannot fit one stream, and that must be said out loud.
    let err = CodecEngine
        .create(
            &qarc_core::CompressOptions {
                archive: dir.path().join("two.gz").to_string_lossy().into_owned(),
                sources: vec![input.to_string_lossy().into_owned(), input.to_string_lossy().into_owned()],
                format: Format::GZip,
                ..Default::default()
            },
            &mut |_| {},
        )
        .unwrap_err();
    assert!(matches!(err, qarc_core::EngineError::UnsupportedFormat(_)), "{err:?}");
}

#[test]
fn testing_a_single_file_archive_must_not_write_anything() {
    use qarc_core::codec_engine::CodecEngine;
    let dir = Temp::new("test-only");
    let input = dir.path().join("data.txt");
    std::fs::write(&input, b"repeat repeat repeat").unwrap();
    let gz = dir.path().join("data.txt.gz");
    codec::encode_file(&input, &gz, Format::GZip, 6).unwrap();

    let dest = dir.path().join("out");
    let done = CodecEngine
        .test(
            &ExtractRequest {
                archive: gz.to_string_lossy().into_owned(),
                dest: dest.to_string_lossy().into_owned(),
                ..Default::default()
            },
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(done.items, 1, "the integrity probe still has to report the member");
    assert!(done.bytes_out > 0, "and it has to read the stream to the end");
    assert!(!dest.exists(), "測試完整性 cannot create the destination -- it is a read, not an extract");

    // A truncated stream must be caught by the same path, or "測試" proves nothing.
    let bytes = std::fs::read(&gz).unwrap();
    let cut = dir.path().join("cut.txt.gz");
    std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
    let err = CodecEngine
        .test(
            &ExtractRequest {
                archive: cut.to_string_lossy().into_owned(),
                dest: dir.path().join("out2").to_string_lossy().into_owned(),
                ..Default::default()
            },
            &mut |_| {},
        )
        .err()
        .expect("a half-written gzip must not pass as intact");
    assert!(
        matches!(err, qarc_core::EngineError::Corrupt(_)),
        "expected Corrupt for a truncated frame, got {err:?}"
    );
}

#[test]
fn a_member_that_is_not_there_is_reported_instead_of_ignored() {
    let dir = Temp::new("member-filter");
    let input = dir.path().join("data.txt");
    std::fs::write(&input, b"payload payload").unwrap();
    let gz = dir.path().join("data.txt.gz");
    codec::encode_file(&input, &gz, Format::GZip, 6).unwrap();

    let ops = qarc_core::Ops::discover();
    let dest = dir.path().join("out");
    let done = ops
        .extract(
            &ExtractRequest {
                archive: gz.to_string_lossy().into_owned(),
                dest: dest.to_string_lossy().into_owned(),
                members: vec!["nothing-here.txt".to_string()],
                ..Default::default()
            },
            &mut |_| {},
            None,
        )
        .unwrap();
    // The façade reports "nothing matched" as a failed Outcome rather than an error,
    // matching the .NET product; either way it must not claim success.
    assert!(!done.ok, "{done:?}");
    assert_eq!(done.items, 0, "no member matched, so nothing may be counted");
    assert!(
        done.errors.iter().any(|e| e.contains("找不到這些條目")),
        "asking for a member that is not inside must fail loudly: {:?}",
        done.errors
    );
    assert!(!dest.exists(), "and it must not have written the payload anyway");
}

#[test]
fn compound_alias_names_decode_to_the_tar_they_wrap() {
    use qarc_core::codec_engine::stripped_name;
    for (archive, expected) in [
        ("a.tar.gz", "a.tar"),
        ("a.tgz", "a.tar"),
        ("a.tbz2", "a.tar"),
        ("a.txz", "a.tar"),
        ("a.tzst", "a.tar"),
        ("a.txt.gz", "a.txt"),
        ("a.gz", "a"),
    ] {
        let got = stripped_name(std::path::Path::new(archive));
        assert_eq!(got, expected, "{archive} -> {got}, expected {expected}");
    }
}
