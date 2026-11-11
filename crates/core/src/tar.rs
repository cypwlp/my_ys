//! Managed tar engine: plain `.tar` and `.tar.gz`, written with pure-Rust
//! dependencies so it builds and tests on a machine with no C compiler.
//!
//! This is the engine that proves the architecture end-to-end today: create,
//! list, extract and block traversal all run without 7-Zip being installed.

use crate::cancel::{stopped, Cancel};
use crate::engine::{Engine, EngineError, EngineResult, Op};
use crate::formats::{self, Format};
use crate::models::{ArchiveInfo, EngineKind, Entry, ExtractRequest, Outcome, Progress};
use crate::pathguard;
use crate::repair;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct TarEngine;

/// The `tar` crate quotes the offending header bytes inside its error strings, so
/// `numeric field was not a number: c hello hello hello ...  when getting cksum for
/// <the whole 512-byte block>` lands verbatim in the UI.  Keep the diagnosis and the
/// field name; drop the data dump.  Anything that is not a header fault (a real I/O
/// error, say) is passed through untouched, because relabelling it as corruption
/// would send the user looking in the wrong place.
fn tar_corrupt(e: std::io::Error) -> EngineError {
    let text = e.to_string();
    if let Some(rest) = text.strip_prefix("numeric field was not a number") {
        let field = rest
            .rsplit("when getting ")
            .next()
            .unwrap_or("")
            .split(" for ")
            .next()
            .unwrap_or("")
            .trim();
        let field = if field.is_empty() { "header" } else { field };
        return EngineError::Corrupt(format!("tar 標頭欄位不是數字（{field}）—— 內容可能不是 tar，或已損毀"));
    }
    if text.contains("crc error") || text.contains("checksum") {
        return EngineError::Corrupt("tar 標頭校驗和不符".into());
    }
    EngineError::Corrupt(text)
}

/// Reads a decoded container from a temp file and removes it on drop, so listing
/// an `.tar.xz` does not litter the disk.
struct TempReader {
    file: File,
    path: PathBuf,
}

impl Read for TempReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl Drop for TempReader {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Open the tar stream inside `path`, whatever layer wraps it.
fn reader(path: &Path, format: Format) -> EngineResult<(Box<dyn Read + 'static>, Format)> {
    let file = File::open(path).map_err(|e| EngineError::Io(e.to_string()))?;
    Ok(match format {
        Format::Tar => (Box::new(file), Format::Tar),
        Format::GZip | Format::TarGz => (Box::new(GzDecoder::new(file)), Format::Tar),
        Format::TarBz2 => (Box::new(crate::codec::wrap_read(Format::BZip2, Box::new(file))?), Format::Tar),
        Format::TarXz | Format::TarZst => {
            let layer = crate::codec::layer(format).unwrap_or(Format::Xz);
            let tmp = crate::codec::scratch_path(path, ".decoded.tar");
            // A failed decode must not litter the disk: the guard that deletes the
            // temp only exists once the file has been opened successfully.
            if let Err(e) = crate::codec::decode_to_file(path, &tmp, layer) {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
            let file = match File::open(&tmp) {
                Ok(f) => f,
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(EngineError::Io(e.to_string()));
                }
            };
            (Box::new(TempReader { file, path: tmp }), Format::Tar)
        }
        f => return Err(EngineError::UnsupportedFormat(f.to_string())),
    })
}

fn writer(path: &Path, format: Format) -> EngineResult<Box<dyn Write + 'static>> {
    let file = File::create(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::StorageFull {
            EngineError::DiskFull
        } else {
            EngineError::Io(e.to_string())
        }
    })?;
    Ok(match format {
        Format::Tar => Box::new(file),
        Format::TarGz => Box::new(GzEncoder::new(file, Compression::default())),
        Format::TarBz2 => Box::new(bzip2::write::BzEncoder::new(file, bzip2::Compression::new(6))),
        // zstd has no streaming `Write` in the pure-Rust crate, so the tar is written
        // to a temp file and compressed in a second pass by create_to.  xz is refused
        // outright: see codec::encode_file.
        Format::TarZst => Box::new(file),
        f => return Err(EngineError::UnsupportedFormat(f.to_string())),
    })
}

/// True when `member` is the requested path, or lives under a requested directory.
pub(crate) fn selected(member: &str, wanted: &[String]) -> bool {
    if wanted.is_empty() {
        return true;
    }
    wanted.iter().any(|w| {
        let w = w.trim_end_matches('/');
        member == w
            || member
                .strip_prefix(w)
                .map(|rest| rest.starts_with('/'))
                .unwrap_or(false)
    })
}

impl Engine for TarEngine {
    fn id(&self) -> &'static str {
        "tar-managed"
    }

    fn kind(&self) -> EngineKind {
        EngineKind::Managed
    }

    fn supports(&self, format: Format, op: Op) -> bool {
        match op {
            Op::Read => matches!(
                format,
                Format::Tar | Format::TarGz | Format::TarBz2 | Format::TarXz | Format::TarZst
            ),
            Op::Write => matches!(
                format,
                Format::Tar | Format::TarGz | Format::TarBz2 | Format::TarZst
            ),
        }
    }

    fn priority(&self, format: Format, op: Op) -> i32 {
        if !self.supports(format, op) {
            return -1;
        }
        // Wins over 7-Zip for tar: no subprocess, so no password in the process list.
        110
    }

    fn list(&self, archive: &Path, _password: Option<&str>) -> EngineResult<ArchiveInfo> {
        let format = formats::sniff(archive);
        let (reader, _) = reader(archive, format)?;
        let mut ar = tar::Archive::new(reader);
        let mut entries: Vec<Entry> = Vec::new();
        for item in ar.entries().map_err(tar_corrupt)? {
            let item = item.map_err(tar_corrupt)?;
            let header = item.header();
            let raw = header.path_bytes().to_vec();
            let (name, codec) = repair::repair_bytes(&raw);
            let path = name.replace('\\', "/");
            let is_dir = header.entry_type().is_dir();
            entries.push(Entry {
                unsafe_path: pathguard::is_traversal(&path),
                name: path.rsplit('/').next().unwrap_or(&path).trim_end_matches('/').to_string(),
                is_dir,
                mtime: header.mtime().ok().map(|m| m as i64),
                packed: None,
                size: header.entry_size().unwrap_or(0),
                attrs: format!(
                    "{}{}",
                    if is_dir { "D" } else { "" },
                    mode_letters(header.mode().unwrap_or(0))
                ),
                codec,
                encrypted: false,
                path,
            });
        }
        let total_size = entries.iter().map(|e| e.size).sum();
        Ok(ArchiveInfo {
            path: archive.to_string_lossy().into_owned(),
            format,
            engine: self.id().to_string(),
            entries,
            total_size,
            ..Default::default()
        })
    }

    fn extract_cancellable(
        &self,
        req: &ExtractRequest,
        report: &mut dyn FnMut(Progress),
        cancel: Cancel<'_>,
    ) -> EngineResult<Outcome> {
        let archive = Path::new(&req.archive);
        let dest = Path::new(&req.dest);
        let (reader, _) = reader(archive, formats::sniff(archive))?;
        let mut ar = tar::Archive::new(reader);
        let mut out = Outcome {
            engine: self.id().to_string(),
            ..Default::default()
        };
        std::fs::create_dir_all(dest).map_err(|e| EngineError::Io(e.to_string()))?;

        for item in ar.entries().map_err(tar_corrupt)? {
            if stopped(cancel) {
                out.cancelled = true;
                break;
            }
            let mut item = item.map_err(tar_corrupt)?;
            let header = item.header();
            let (name, _) = repair::repair_bytes(&header.path_bytes().to_vec());
            let member = name.replace('\\', "/");
            if !selected(&member, &req.members) {
                continue;
            }
            let target = match pathguard::safe_join(dest, &member) {
                Ok(p) => p,
                Err(e) => {
                    out.blocked.push(format!("{member} ({e})"));
                    continue;
                }
            };
            if req.test_only {
                // A sink, not a Vec: testing a 4 GB member must not allocate 4 GB.
                let mut sink = std::io::sink();
                let n = std::io::copy(&mut item, &mut sink)
                    .map_err(tar_corrupt)?;
                out.bytes_out += n;
                out.items += 1;
                continue;
            }
            if header.entry_type().is_dir() {
                std::fs::create_dir_all(&target).map_err(|e| EngineError::Io(e.to_string()))?;
                out.items += 1;
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| EngineError::Io(e.to_string()))?;
            }
            let target = apply_overwrite(&target, req.overwrite, header.mtime().unwrap_or(0));
            if target == PathBuf::new() {
                continue; // Skip policy
            }
            let mut file = File::create(&target).map_err(|e| EngineError::Io(e.to_string()))?;
            let n = std::io::copy(&mut item, &mut file)
                .map_err(tar_corrupt)?;
            out.bytes_out += n;
            out.items += 1;
            report(Progress {
                fraction: None,
                done: out.bytes_out,
                total: 0,
                summary: member,
            });
        }
        out.ok = out.errors.is_empty() && !out.cancelled;
        Ok(out)
    }

    fn create_cancellable(
        &self,
        opts: &crate::models::CompressOptions,
        report: &mut dyn FnMut(Progress),
        cancel: Cancel<'_>,
    ) -> EngineResult<Outcome> {
        let sources: Vec<&Path> = opts.sources.iter().map(Path::new).collect();
        create_to(Path::new(&opts.archive), &sources, opts.format, None, report, cancel)
    }
}

/// `Skip` yields an empty path, which the caller reads as "write nothing".
fn apply_overwrite(target: &Path, mode: crate::models::Overwrite, src_mtime: u64) -> PathBuf {
    use crate::models::Overwrite::*;
    match mode {
        Skip if target.exists() => PathBuf::new(),
        Newer if target.exists() => {
            let newer = std::fs::metadata(target)
                .and_then(|m| m.modified())
                .map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            if src_mtime > newer {
                target.to_path_buf()
            } else {
                PathBuf::new()
            }
        }
        KeepBoth => pathguard::unique_name(target),
        _ => target.to_path_buf(),
    }
}

fn mode_letters(mode: u32) -> String {
    let mut s = String::new();
    for (bit, letter) in [(0o400, 'r'), (0o200, 'w'), (0o100, 'x')] {
        s.push(if mode & bit != 0 { letter } else { '-' });
    }
    s
}

/// Build a tar / tar.gz from a list of files and directories.  Directory entries
/// are stored with a trailing slash, which is what every other archiver expects.
pub fn create_to(
    archive: &Path,
    sources: &[&Path],
    format: Format,
    root_label: Option<&str>,
    mut report: impl FnMut(Progress),
    cancel: Cancel<'_>,
) -> EngineResult<Outcome> {
    let mut out = Outcome {
        engine: "tar-managed".into(),
        ..Default::default()
    };
    let needs_second_pass = format == Format::TarZst;
    let sink = if needs_second_pass {
        crate::codec::scratch_path(archive, ".staged.tar")
    } else {
        archive.to_path_buf()
    };
    let mut writer = writer(&sink, format)?;
    {
        let mut tar = tar::Builder::new(&mut writer);
        for source in sources {
            let base = root_label
                .map(|s| s.to_string())
                .unwrap_or_else(|| source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            if source.is_dir() {
                let mut stack = vec![source.to_path_buf()];
                while let Some(dir) = stack.pop() {
                    if stopped(cancel) {
                        out.cancelled = true;
                        out.ok = false;
                        return Ok(out);
                    }
                    for child in std::fs::read_dir(&dir).map_err(|e| EngineError::Io(e.to_string()))? {
                        // Checked per entry: a flat folder never pops the stack, so
                        // a per-directory check cancels too late to matter.
                        if stopped(cancel) {
                            out.cancelled = true;
                            out.ok = false;
                            return Ok(out);
                        }
                        let child = child.map_err(|e| EngineError::Io(e.to_string()))?.path();
                        let rel = child
                            .strip_prefix(source)
                            .map_err(|_| EngineError::Io("outside source root".into()))?;
                        let name = format!("{base}/{}", rel.to_string_lossy().replace('\\', "/"));
                        if child.is_dir() {
                            tar.append_dir(&format!("{name}/"), &child)
                                .map_err(|e| EngineError::Io(e.to_string()))?;
                            stack.push(child);
                        } else {
                            let mut f = File::open(&child).map_err(|e| EngineError::Io(e.to_string()))?;
                            let meta = f.metadata().ok();
                            let mut h = tar::Header::new_ustar();
                            h.set_path(&name).map_err(|e| EngineError::Io(e.to_string()))?;
                            h.set_size(meta.as_ref().map(|m| m.len()).unwrap_or(0));
                            h.set_mode(0o644);
                            h.set_mtime(
                                meta.as_ref()
                                    .and_then(|m| m.modified().ok())
                                    .map(|t| t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
                                    .unwrap_or(0),
                            );
                            h.set_entry_type(tar::EntryType::Regular);
                            tar.append_data(&mut h, &name, &mut f)
                                .map_err(|e| EngineError::Io(e.to_string()))?;
                            out.items += 1;
                            out.bytes_out += h.entry_size().unwrap_or(0);
                            report(Progress {
                                fraction: None,
                                done: out.bytes_out,
                                total: 0,
                                summary: name,
                            });
                        }
                    }
                }
            } else {
                let mut f = File::open(source).map_err(|e| EngineError::Io(e.to_string()))?;
                let mut h = tar::Header::new_ustar();
                h.set_path(&base).map_err(|e| EngineError::Io(e.to_string()))?;
                h.set_size(f.metadata().map(|m| m.len()).unwrap_or(0));
                h.set_mode(0o644);
                h.set_entry_type(tar::EntryType::Regular);
                tar.append_data(&mut h, &base, &mut f)
                    .map_err(|e| EngineError::Io(e.to_string()))?;
                out.items += 1;
                out.bytes_out += h.entry_size().unwrap_or(0);
                report(Progress {
                    fraction: None,
                    done: out.bytes_out,
                    total: 0,
                    summary: base,
                });
            }
        }
        tar.finish().map_err(|e| EngineError::Io(e.to_string()))?;
    }
    writer.flush().map_err(|e| EngineError::Io(e.to_string()))?;
    if needs_second_pass {
        drop(writer);
        let layer = crate::codec::layer(format).unwrap_or(Format::Xz);
        let written = crate::codec::encode_file(&sink, archive, layer, 6)?;
        let _ = std::fs::remove_file(&sink);
        out.bytes_out = written;
    }
    report(Progress { fraction: Some(1.0), done: out.bytes_out, total: out.bytes_out, summary: String::new() });
    out.ok = true;
    Ok(out)
}
