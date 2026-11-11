//! Compression layers that are not gzip: bzip2, xz, zstd.
//!
//! All three are pure Rust here (`bzip2`, `lzma-rs`, `ruzstd`) because this
//! machine has no C compiler -- the `zstd`/`xz2` crates would need one and would
//! fail to build.  Streaming where the crate supports it, temp-file otherwise:
//! a 4 GB `.tar.xz` must not be read into memory.

use crate::engine::{EngineError, EngineResult};
use crate::formats::Format;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// A layer that can be turned into a `Read` adapter without touching the disk.
pub fn is_adapter(fmt: Format) -> bool {
    matches!(fmt, Format::GZip | Format::BZip2)
}

/// A layer that must be decoded to a file first (no streaming `Read` adapter in
/// the pure-Rust crate we use).
pub fn is_file_decode(fmt: Format) -> bool {
    matches!(fmt, Format::Xz | Format::Zstd)
}

pub fn inner_tar(fmt: Format) -> Option<Format> {
    match fmt {
        Format::TarGz | Format::TarBz2 | Format::TarXz | Format::TarZst => Some(Format::Tar),
        _ => None,
    }
}

/// The compression layer of a container format.
pub fn layer(fmt: Format) -> Option<Format> {
    Some(match fmt {
        Format::TarGz | Format::GZip => Format::GZip,
        Format::TarBz2 | Format::BZip2 => Format::BZip2,
        Format::TarXz | Format::Xz => Format::Xz,
        Format::TarZst | Format::Zstd => Format::Zstd,
        _ => return None,
    })
}

/// Wrap a reader so the bytes coming out are uncompressed.  Only gzip and bzip2
/// have streaming readers in the pure-Rust set.
pub fn wrap_read(fmt: Format, reader: Box<dyn Read + 'static>) -> EngineResult<Box<dyn Read + 'static>> {
    Ok(match fmt {
        Format::GZip => Box::new(flate2::read::GzDecoder::new(reader)),
        Format::BZip2 => Box::new(bzip2::read::BzDecoder::new(reader)),
        other => return Err(EngineError::UnsupportedFormat(other.to_string())),
    })
}

/// Decode a whole file into `dest`, streaming through a buffered writer so memory
/// stays flat regardless of archive size.  Returns the decoded byte count.
pub fn decode_to_file(from: &Path, dest: &Path, fmt: Format) -> EngineResult<u64> {
    let src = BufReader::with_capacity(256 * 1024, File::open(from).map_err(|e| EngineError::Io(e.to_string()))?);
    let mut out = BufWriter::new(File::create(dest).map_err(|e| EngineError::Io(e.to_string()))?);
    let written: u64 = match fmt {
        Format::GZip => {
            let mut d = flate2::read::GzDecoder::new(src);
            std::io::copy(&mut d, &mut out).map_err(|e| EngineError::Corrupt(e.to_string()))?
        }
        Format::BZip2 => {
            let mut d = bzip2::read::BzDecoder::new(src);
            std::io::copy(&mut d, &mut out).map_err(|e| EngineError::Corrupt(e.to_string()))?
        }
        Format::Xz => {
            // lzma-rs drives the writer itself, so memory stays flat; a truncated
            // stream surfaces as an error instead of a silently short file.
            let mut src = src;
            lzma_rs::xz_decompress(&mut src, &mut out)
                .map_err(|e| EngineError::Corrupt(format!("xz 解碼失敗：{}", e)))?;
            out.flush().map_err(|e| EngineError::Io(e.to_string()))?;
            std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0)
        }
        Format::Zstd => {
            let mut decoder = ruzstd::decoding::StreamingDecoder::new(src)
                .map_err(|e| EngineError::Corrupt(format!("zstd 框架無效：{e:?}")))?;
            std::io::copy(&mut decoder, &mut out).map_err(|e| EngineError::Corrupt(e.to_string()))?
        }
        other => return Err(EngineError::UnsupportedFormat(other.to_string())),
    };
    out.flush().map_err(|e| EngineError::Io(e.to_string()))?;
    Ok(written)
}

/// Encoder for the layers we can *write* in pure Rust.
pub fn wrap_write(fmt: Format, writer: Box<dyn Write + 'static>, level: u8) -> EngineResult<Box<dyn Write + 'static>> {
    Ok(match fmt {
        Format::GZip => Box::new(flate2::write::GzEncoder::new(
            writer,
            flate2::Compression::new(level.max(1) as u32),
        )),
        Format::BZip2 => Box::new(bzip2::write::BzEncoder::new(
            writer,
            bzip2::Compression::new(level.clamp(1, 9) as u32),
        )),
        other => return Err(EngineError::UnsupportedFormat(other.to_string())),
    })
}

/// A writable layer, for the format table's honesty check.
pub fn can_write(fmt: Format) -> bool {
    matches!(fmt, Format::GZip | Format::BZip2 | Format::Zstd)
}

/// Compress one file into `dest`.  gzip/bzip2 stream through a `Write` adapter;
/// xz and zstd are driven by their crates, which take a reader and a writer.
/// Returns the compressed size.
pub fn encode_file(from: &Path, dest: &Path, fmt: Format, level: u8) -> EngineResult<u64> {
    // Gate before creating `dest`.  An attempt that truncates the archive first and
    // only then reports "cannot do it" leaves a 0-byte file behind, and the 7-Zip
    // fallback opens that file for update -- "Cannot open the file as [xz] archive",
    // which surfaced to the user as 壓縮包損毀 for a perfectly good source.
    // xz is the case that matters: lzma-rs 0.3's xz_compress emits uncompressed
    // LZMA2 chunks, so 208,000 bytes of repetitive text came back as 208,064.  A
    // writer that makes files bigger is worse than no writer, so xz creation stays
    // closed until that crate (or another pure-Rust one) really compresses.
    if !can_write(fmt) {
        return Err(if fmt == Format::Xz {
            EngineError::UnsupportedFormat(
                "xz 沒有可用的純 Rust 編碼器（實測輸出比原檔大），請改用 7z 或 tar.zst".into())
        } else {
            EngineError::UnsupportedFormat(fmt.to_string())
        });
    }
    let src = BufReader::with_capacity(256 * 1024, File::open(from).map_err(|e| EngineError::Io(e.to_string()))?);
    let out = File::create(dest).map_err(|e| EngineError::Io(e.to_string()))?;
    match fmt {
        Format::GZip => {
            let mut e = flate2::write::GzEncoder::new(out, flate2::Compression::new(level.clamp(1, 9) as u32));
            std::io::copy(&mut { src }, &mut e).map_err(|e| EngineError::Io(e.to_string()))?;
            e.finish().map_err(|e| EngineError::Io(e.to_string()))?;
        }
        Format::BZip2 => {
            let mut e = bzip2::write::BzEncoder::new(out, bzip2::Compression::new(level.clamp(1, 9) as u32));
            std::io::copy(&mut { src }, &mut e).map_err(|e| EngineError::Io(e.to_string()))?;
            e.finish().map_err(|e| EngineError::Io(e.to_string()))?;
        }
        Format::Zstd => {
            // ruzstd 0.9 implements only Uncompressed and Fastest; Default/Better/Best
            // are marked UNIMPLEMENTED upstream.  Accepting the level and quietly
            // producing Fastest output is fine, pretending it is level 9 is not, so
            // the UI says so and the test only asserts a round trip.
            let _ = level;
            ruzstd::encoding::compress(src, BufWriter::new(out), ruzstd::encoding::CompressionLevel::Fastest);
        }
        other => return Err(EngineError::UnsupportedFormat(other.to_string())),
    }
    std::fs::metadata(dest).map(|m| m.len()).map_err(|e| EngineError::Io(e.to_string()))
}

/// Scratch path for a decoded container, keyed by the source so two archives do
/// not collide and a rerun overwrites its own temp file.
pub fn scratch_path(source: &Path, suffix: &str) -> PathBuf {
    let hash = crate::human::simple_hash(&source.to_string_lossy());
    let name = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".into());
    std::env::temp_dir().join(format!("quickarc-{name}-{hash:016x}{suffix}"))
}
