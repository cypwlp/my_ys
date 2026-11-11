//! Standalone single-file codecs: `foo.gz`, `foo.bz2`, `foo.xz`, `foo.zst`.
//!
//! These are not archives -- they hold one stream -- but a user dragging one in
//! expects the same two clicks as with any archive, so they get a one-entry
//! listing and a normal extract.

use crate::codec;
use crate::engine::{Engine, EngineError, EngineResult, Op};
use crate::formats::Format;
use crate::models::{ArchiveInfo, EngineKind, Entry, ExtractRequest, Outcome, Progress};
use crate::pathguard;
use std::path::Path;

pub struct CodecEngine;

/// Strip the codec suffix so `a.txt.gz` extracts as `a.txt`.
pub fn stripped_name(path: &Path) -> String {
    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    for suffix in [".gz", ".bz2", ".xz", ".zst"] {
        if let Some(base) = name.strip_suffix(suffix) {
            if !base.is_empty() {
                return base.to_string();
            }
        }
    }
    // The one-letter aliases already mean "tar inside": `x.tar.tzst` decoding to
    // `x.tar.tar` would be wrong twice over.  Without this, `n2.tzst` decoded as
    // `n2.tzst.out` -- an unnamed blob next to the archive.
    for (alias, base) in [(".tgz", ".tar"), (".tbz2", ".tar"), (".tbz", ".tar"), (".txz", ".tar"), (".tzst", ".tar")] {
        if let Some(stem) = name.strip_suffix(alias) {
            if !stem.is_empty() {
                return format!("{stem}{base}");
            }
        }
    }
    format!("{name}.out")
}

/// gzip carries the uncompressed size in its last four bytes; the other layers do
/// not, and guessing is worse than reporting nothing.
fn gzip_isize(path: &Path) -> Option<u64> {
    use std::io::Seek;
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < 8 {
        return None;
    }
    let mut buf = [0u8; 4];
    file.seek(std::io::SeekFrom::End(-4)).ok()?;
    std::io::Read::read_exact(&mut file, &mut buf).ok()?;
    Some(u32::from_le_bytes(buf) as u64)
}

impl Engine for CodecEngine {
    fn id(&self) -> &'static str {
        "codec-managed"
    }

    fn kind(&self) -> EngineKind {
        EngineKind::Managed
    }

    fn supports(&self, format: Format, op: Op) -> bool {
        match op {
            Op::Read => matches!(format, Format::GZip | Format::BZip2 | Format::Xz | Format::Zstd),
            // xz reads fine but lzma-rs cannot write it; claiming otherwise is what
            // made the engine truncate the destination and then hand a 0-byte file to
            // the 7-Zip fallback.
            Op::Write => codec::can_write(format),
        }
    }

    fn priority(&self, format: Format, op: Op) -> i32 {
        if !self.supports(format, op) {
            return -1;
        }
        // Above 7-Zip: no subprocess, no password in the process list, no install needed.
        130
    }

    fn list(&self, archive: &Path, password: Option<&str>) -> EngineResult<ArchiveInfo> {
        if password.is_some() {
            return Err(EngineError::UnsupportedFormat("單檔壓縮不支援密碼".into()));
        }
        if !archive.is_file() {
            return Err(EngineError::Io(format!("檔案不存在：{}", archive.display())));
        }
        let format = crate::formats::sniff(archive);
        if !self.supports(format, Op::Read) {
            return Err(EngineError::UnsupportedFormat(format.to_string()));
        }
        let compressed = std::fs::metadata(archive).map(|m| m.len()).unwrap_or(0);
        let decoded = if format == Format::GZip { gzip_isize(archive) } else { None };
        let name = stripped_name(archive);
        Ok(ArchiveInfo {
            path: archive.to_string_lossy().into_owned(),
            format,
            engine: self.id().to_string(),
            total_size: decoded.unwrap_or(compressed),
            total_packed: compressed,
            entries: vec![Entry {
                name: name.clone(),
                path: name,
                is_dir: false,
                size: decoded.unwrap_or(0),
                packed: Some(compressed),
                mtime: std::fs::metadata(archive)
                    .and_then(|m| m.modified())
                    .ok()
                    .map(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0)
                    }),
                attrs: "A".into(),
                encrypted: false,
                codec: None,
                unsafe_path: false,
            }],
            ..Default::default()
        })
    }

    /// `a out.gz input.txt` -- one input file per archive, because that is what a
    /// single-stream format can hold.
    fn create_cancellable(
        &self,
        opts: &crate::models::CompressOptions,
        report: &mut dyn FnMut(Progress),
        cancel: crate::cancel::Cancel<'_>,
    ) -> EngineResult<Outcome> {
        if crate::cancel::stopped(cancel) {
            return Ok(Outcome {
                ok: false,
                cancelled: true,
                engine: self.id().to_string(),
                ..Default::default()
            });
        }
        if opts.sources.len() != 1 {
            return Err(EngineError::UnsupportedFormat(format!(
                "單檔壓縮一次只能收一個來源（收到 {} 個）",
                opts.sources.len()
            )));
        }
        let source = Path::new(&opts.sources[0]);
        if !source.is_file() {
            return Err(EngineError::UnsupportedFormat(
                "單檔壓縮的來源必須是檔案，資料夾請用 tar.gz".into(),
            ));
        }
        let format = crate::formats::from_extension(&opts.archive);
        if !self.supports(format, Op::Read) {
            return Err(EngineError::UnsupportedFormat(format.to_string()));
        }
        let written = codec::encode_file(source, Path::new(&opts.archive), format, opts.level)?;
        report(Progress {
            fraction: Some(1.0),
            done: written,
            total: written,
            summary: opts.archive.clone(),
        });
        Ok(Outcome {
            ok: true,
            items: 1,
            bytes_out: written,
            engine: self.id().to_string(),
            ..Default::default()
        })
    }

    fn extract_cancellable(
        &self,
        req: &ExtractRequest,
        report: &mut dyn FnMut(Progress),
        cancel: crate::cancel::Cancel<'_>,
    ) -> EngineResult<Outcome> {
        if crate::cancel::stopped(cancel) {
            return Ok(Outcome {
                ok: false,
                cancelled: true,
                engine: self.id().to_string(),
                ..Default::default()
            });
        }
        let archive = Path::new(&req.archive);
        let format = crate::formats::sniff(archive);
        let dest_dir = Path::new(&req.dest);
        let member = stripped_name(archive);
        // A single-stream archive still has to respect the filter, otherwise a
        // request for a member that is not there writes the payload anyway and the
        // facade's "找不到這些條目" guard never fires.
        if !crate::tar::selected(&member, &req.members) {
            return Ok(Outcome {
                ok: true,
                items: 0,
                engine: self.id().to_string(),
                ..Default::default()
            });
        }
        if req.test_only {
            // "測試完整性" must not create files, and must not even create the
            // destination directory.  The stream still has to be read to the end --
            // a truncated or corrupt frame only shows up in the decode -- but the
            // bytes go to a sink, not to disk.
            use std::io::Read;
            let file = std::fs::File::open(archive).map_err(|e| EngineError::Io(e.to_string()))?;
            let mut reader = crate::codec::wrap_read(format, Box::new(file))?;
            let mut buf = [0u8; 256 * 1024];
            let mut decoded = 0u64;
            loop {
                if crate::cancel::stopped(cancel) {
                    return Ok(Outcome {
                        ok: false,
                        cancelled: true,
                        engine: self.id().to_string(),
                        ..Default::default()
                    });
                }
                let n = reader
                    .read(&mut buf)
                    .map_err(|e| EngineError::Corrupt(format!("解到 {decoded} 位元組時失敗：{e}")))?;
                if n == 0 {
                    break;
                }
                decoded += n as u64;
            }
            report(Progress {
                fraction: Some(1.0),
                done: decoded,
                total: decoded,
                summary: member,
            });
            return Ok(Outcome {
                ok: true,
                items: 1,
                bytes_out: decoded,
                engine: self.id().to_string(),
                ..Default::default()
            });
        }
        std::fs::create_dir_all(dest_dir).map_err(|e| EngineError::Io(e.to_string()))?;
        let target = pathguard::safe_join(dest_dir, &member)
            .map_err(|e| EngineError::PathTraversal(e.to_string()))?;
        let written = codec::decode_to_file(archive, &target, format)?;
        report(Progress {
            fraction: Some(1.0),
            done: written,
            total: written,
            summary: target.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        });
        Ok(Outcome {
            ok: true,
            items: 1,
            bytes_out: written,
            engine: self.id().to_string(),
            ..Default::default()
        })
    }
}
