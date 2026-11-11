//! Managed ZIP engine (pure Rust).
//!
//! Deliberately refuses password-protected archives rather than half-supporting
//! them: returning `UnsupportedFormat` sends the job to the registry's next
//! candidate, so "needs the full 7-Zip" is an accurate message instead of a
//! corrupt-looking output.

use crate::cancel::{stopped, Cancel};
use crate::engine::{Engine, EngineError, EngineResult, Op};
use crate::formats::Format;
use crate::models::{ArchiveInfo, CompressOptions, EngineKind, Entry, ExtractRequest, Outcome, Progress};
use crate::pathguard;
use crate::repair;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

pub struct ZipEngine;

fn epoch_of(dt: zip::DateTime) -> Option<i64> {
    let (y, mo, d, h, mi, s) = (dt.year(), dt.month(), dt.day(), dt.hour(), dt.minute(), dt.second());
    if !(1980..=2107).contains(&y) || mo == 0 {
        return None;
    }
    Some(crate::sevenzip::days_from_civil(y as i64, mo as i64, d as i64) * 86_400
        + h as i64 * 3_600
        + mi as i64 * 60
        + s as i64)
}

fn selected(member: &str, wanted: &[String]) -> bool {
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

/// Sample the head of a file; if deflate cannot shrink it, store it raw.
/// Without this, 256 MB of already-compressed data is deflated to a stream
/// 42 KB *larger* than the input while burning the whole CPU budget for nothing.
/// 7-Zip makes the same call; the sample keeps it to one 64 KB trial.
fn incompressible(path: &Path, level: u8) -> bool {
    use std::io::{Read, Write};
    const SAMPLE: usize = 64 * 1024;
    let mut buf = [0u8; SAMPLE];
    let Ok(mut f) = File::open(path) else { return false };
    let Ok(n) = f.read(&mut buf) else { return false };
    if n < 4096 {
        return false; // too small to judge; deflate overhead is negligible either way
    }
    let mut enc = flate2::write::DeflateEncoder::new(
        Vec::new(),
        flate2::Compression::new(level.clamp(1, 9) as u32),
    );
    if enc.write_all(&buf[..n]).is_err() {
        return false;
    }
    match enc.finish() {
        Ok(out) => out.len() * 100 >= n * 98,
        Err(_) => false,
    }
}

/// Uncompressed size above which a file is streamed straight into the archive
/// instead of being compressed to an in-memory fragment.  The fragment is held
/// whole until the writer reaches its turn, so this ceiling bounds memory.
const MAX_FRAGMENT_SRC: u64 = 32 * 1024 * 1024;

enum Task {
    Dir(String),
    File { name: String, path: std::path::PathBuf, size: u64 },
}

enum Job {
    Skip,
    Stream { path: std::path::PathBuf, stored: bool },
    Fragment { bytes: Vec<u8>, size: u64 },
    Failed(String),
}

fn file_options(stored: bool, level: u8) -> zip::write::SimpleFileOptions {
    // Stored rejects a compression level outright in the zip crate.
    if stored {
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)
    } else {
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(level as i64))
    }
}

/// Walk the sources once up front so workers and writer see the same ordered
/// entry list; directory entries are kept inline because ZIP stores them first.
fn collect_tasks(opts: &CompressOptions) -> EngineResult<Vec<Task>> {
    let mut tasks = Vec::new();
    for source in &opts.sources {
        let path = Path::new(source);
        let base = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if path.is_file() {
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            tasks.push(Task::File { name: base, path: path.to_path_buf(), size });
            continue;
        }
        let mut stack = vec![path.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for child in std::fs::read_dir(&dir).map_err(|e| EngineError::Io(e.to_string()))? {
                let child = child.map_err(|e| EngineError::Io(e.to_string()))?.path();
                let rel = child
                    .strip_prefix(path)
                    .map_err(|_| EngineError::Io("outside source root".into()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                let name = format!("{base}/{rel}");
                if child.is_dir() {
                    tasks.push(Task::Dir(format!("{name}/")));
                    stack.push(child);
                } else {
                    let size = std::fs::metadata(&child).map(|m| m.len()).unwrap_or(0);
                    tasks.push(Task::File { name, path: child, size });
                }
            }
        }
    }
    Ok(tasks)
}

/// Compress one file into a self-contained single-entry zip held in memory.
/// `Ok(None)` means the sample says "store it raw"; the writer streams from disk.
fn deflate_fragment(path: &Path, level: u8) -> EngineResult<Option<Vec<u8>>> {
    if incompressible(path, level) {
        return Ok(None);
    }
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file("entry", file_options(false, level))
        .map_err(|e| EngineError::Io(e.to_string()))?;
    let mut src = File::open(path).map_err(|e| EngineError::Io(e.to_string()))?;
    std::io::copy(&mut src, &mut writer).map_err(|e| EngineError::Io(e.to_string()))?;
    let cursor = writer.finish().map_err(|e| EngineError::Io(e.to_string()))?;
    Ok(Some(cursor.into_inner()))
}

fn build_job(task: &Task, level: u8) -> Job {
    match task {
        Task::Dir(_) => Job::Skip,
        Task::File { path, size, .. } => {
            if *size <= MAX_FRAGMENT_SRC {
                match deflate_fragment(path, level) {
                    Ok(Some(bytes)) => Job::Fragment { bytes, size: *size },
                    Ok(None) => Job::Stream { path: path.clone(), stored: true },
                    Err(e) => Job::Failed(e.to_string()),
                }
            } else {
                Job::Stream { path: path.clone(), stored: incompressible(path, level) }
            }
        }
    }
}

impl ZipEngine {
    fn open(archive: &Path, password: Option<&str>) -> EngineResult<zip::ZipArchive<File>> {
        if password.is_some() {
            return Err(EngineError::UnsupportedFormat(
                "加密 ZIP 交給 7-Zip（純 Rust 引擎不做加密）".into(),
            ));
        }
        let file = File::open(archive).map_err(|e| EngineError::Io(e.to_string()))?;
        zip::ZipArchive::new(file).map_err(|e| EngineError::Corrupt(e.to_string()))
    }
}

impl Engine for ZipEngine {
    fn id(&self) -> &'static str {
        "zip-managed"
    }

    fn kind(&self) -> EngineKind {
        EngineKind::Managed
    }

    fn supports(&self, format: Format, _op: Op) -> bool {
        format == Format::Zip
    }

    fn priority(&self, format: Format, op: Op) -> i32 {
        if !self.supports(format, op) {
            return -1;
        }
        120
    }

    fn list(&self, archive: &Path, password: Option<&str>) -> EngineResult<ArchiveInfo> {
        let mut zip = ZipEngine::open(archive, password)?;
        let mut entries = Vec::with_capacity(zip.len());
        for i in 0..zip.len() {
            let f = zip.by_index_raw(i).map_err(|e| EngineError::Corrupt(e.to_string()))?;
            let (name, codec) = repair::repair_bytes(f.name_raw());
            let path = name.replace('\\', "/");
            let encrypted = f.encrypted();
            entries.push(Entry {
                unsafe_path: pathguard::is_traversal(&path),
                is_dir: f.is_dir(),
                name: path.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string(),
                size: f.size(),
                packed: Some(f.compressed_size()),
                mtime: f.last_modified().and_then(epoch_of),
                attrs: format!(
                    "{}{}",
                    if f.is_dir() { "D" } else { "A" },
                    if encrypted { "E" } else { "" }
                ),
                encrypted,
                codec,
                path,
            });
        }
        let total_size = entries.iter().map(|e| e.size).sum();
        let total_packed = entries.iter().filter_map(|e| e.packed).sum();
        Ok(ArchiveInfo {
            path: archive.to_string_lossy().into_owned(),
            format: Format::Zip,
            engine: self.id().to_string(),
            needs_password: entries.iter().any(|e| e.encrypted),
            total_size,
            total_packed,
            entries,
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
        let mut zip = ZipEngine::open(archive, req.password.as_deref())?;
        let total = zip.len();
        std::fs::create_dir_all(dest).map_err(|e| EngineError::Io(e.to_string()))?;
        let mut out = Outcome { engine: self.id().to_string(), ..Default::default() };

        for i in 0..total {
            if stopped(cancel) {
                out.cancelled = true;
                break;
            }
            let mut f = zip.by_index(i).map_err(|e| EngineError::Corrupt(e.to_string()))?;
            let (name, _) = repair::repair_bytes(f.name_raw());
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
            if f.is_dir() {
                if !req.test_only {
                    std::fs::create_dir_all(&target).map_err(|e| EngineError::Io(e.to_string()))?;
                }
                out.items += 1;
                continue;
            }
            if req.test_only {
                // io::sink, not a Vec: a 3 GB member inside a test run must not
                // become a 3 GB allocation.
                let mut sink = std::io::sink();
                out.bytes_out += std::io::copy(&mut f, &mut sink)
                    .map_err(|e| EngineError::Corrupt(e.to_string()))?;
                out.items += 1;
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| EngineError::Io(e.to_string()))?;
            }
            let target = match req.overwrite {
                crate::models::Overwrite::Skip if target.exists() => {
                    out.items += 1;
                    continue;
                }
                crate::models::Overwrite::KeepBoth => pathguard::unique_name(&target),
                _ => target,
            };
            let mut file = File::create(&target).map_err(|e| EngineError::Io(e.to_string()))?;
            let n = std::io::copy(&mut f, &mut file).map_err(|e| EngineError::Corrupt(e.to_string()))?;
            out.bytes_out += n;
            out.items += 1;
            report(Progress {
                fraction: Some((i + 1) as f64 / total as f64),
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
        opts: &CompressOptions,
        report: &mut dyn FnMut(Progress),
        cancel: Cancel<'_>,
    ) -> EngineResult<Outcome> {
        if opts.password.is_some() {
            return Err(EngineError::UnsupportedFormat("加密 ZIP 交給 7-Zip".into()));
        }
        let mut out = Outcome { engine: self.id().to_string(), ..Default::default() };
        if stopped(cancel) {
            out.cancelled = true;
            out.ok = false;
            return Ok(out);
        }
        let tasks = collect_tasks(opts)?;
        let level = opts.level.clamp(1, 9);
        let file_count = tasks.iter().filter(|t| matches!(t, Task::File { .. })).count();
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(8)
            .min(file_count.max(1));
        let next = AtomicUsize::new(0);
        let (tx, rx) = mpsc::channel::<(usize, Job)>();

        std::thread::scope(|scope| -> EngineResult<()> {
            // Workers compress out of order; the writer below re-sequences by
            // index, so the archive layout still follows the source walk.
            if file_count > 0 {
                for _ in 0..workers {
                    let tx = tx.clone();
                    let tasks = &tasks;
                    let next = &next;
                    scope.spawn(move || loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= tasks.len() {
                            break;
                        }
                        if stopped(cancel) {
                            let _ = tx.send((i, Job::Skip));
                            continue;
                        }
                        if tx.send((i, build_job(&tasks[i], level))).is_err() {
                            break;
                        }
                    });
                }
            }
            drop(tx);

            let file = File::create(&opts.archive).map_err(|e| EngineError::Io(e.to_string()))?;
            let mut writer = zip::ZipWriter::new(file);
            let mut pending: HashMap<usize, Job> = HashMap::new();
            for (i, task) in tasks.iter().enumerate() {
                if stopped(cancel) {
                    out.cancelled = true;
                    out.ok = false;
                    return Ok(());
                }
                let name = match task {
                    Task::Dir(name) => {
                        writer
                            .add_directory(name, zip::write::SimpleFileOptions::default())
                            .map_err(|e| EngineError::Io(e.to_string()))?;
                        continue;
                    }
                    Task::File { name, .. } => name,
                };
                let job = loop {
                    if let Some(job) = pending.remove(&i) {
                        break job;
                    }
                    match rx.recv() {
                        Ok((k, job)) => {
                            pending.insert(k, job);
                        }
                        Err(_) => break Job::Failed("worker 意外結束".into()),
                    }
                };
                match job {
                    // Workers only answer Skip when the run was cancelled.
                    Job::Skip => {
                        out.cancelled = true;
                        out.ok = false;
                        return Ok(());
                    }
                    Job::Failed(msg) => return Err(EngineError::Io(msg)),
                    Job::Stream { path, stored } => {
                        writer
                            .start_file(name, file_options(stored, level))
                            .map_err(|e| EngineError::Io(e.to_string()))?;
                        let mut src = File::open(&path).map_err(|e| EngineError::Io(e.to_string()))?;
                        let n = std::io::copy(&mut src, &mut writer)
                            .map_err(|e| EngineError::Io(e.to_string()))?;
                        out.items += 1;
                        out.bytes_out += n;
                    }
                    Job::Fragment { bytes, size } => {
                        let mut cursor = std::io::Cursor::new(bytes);
                        let mut fragment = zip::ZipArchive::new(&mut cursor)
                            .map_err(|e| EngineError::Corrupt(e.to_string()))?;
                        let entry = fragment
                            .by_index(0)
                            .map_err(|e| EngineError::Corrupt(e.to_string()))?;
                        writer
                            .raw_copy_file_rename(entry, name.as_str())
                            .map_err(|e| EngineError::Io(e.to_string()))?;
                        out.items += 1;
                        out.bytes_out += size;
                    }
                }
                report(Progress {
                    fraction: None,
                    done: out.bytes_out,
                    total: 0,
                    summary: name.clone(),
                });
            }
            writer.finish().map_err(|e| EngineError::Io(e.to_string()))?;
            Ok(())
        })?;
        report(Progress { fraction: Some(1.0), done: out.bytes_out, total: out.bytes_out, summary: String::new() });
        out.ok = true;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incompressible_sample_picks_store_only_for_random_bytes() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("qarc-inc-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut x: u64 = 0x2545_F491_4F6C_DD1D;
        let mut random = Vec::with_capacity(64 * 1024);
        for _ in 0..64 * 1024 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            random.push(x as u8);
        }
        let text = "hello world ".repeat(64 * 1024 / 12);
        let p1 = dir.join("random.bin");
        let p2 = dir.join("text.txt");
        std::fs::write(&p1, &random).unwrap();
        std::fs::write(&p2, &text).unwrap();
        assert!(incompressible(&p1, 5), "a random sample must be stored, not deflated");
        assert!(!incompressible(&p2, 5), "a repetitive text sample must be deflated");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
