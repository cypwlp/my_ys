//! One level of a listing, the way a file manager shows it.
//!
//! Two bugs this file exists to prevent, both found in the .NET build:
//!   * a row whose path equals the current prefix must not be emitted (it is the
//!     folder you are already in, and the old code indexed past the end on it);
//!   * folder rows are keyed by their *full* internal path, not the basename, so
//!     `a/報告/` and `b/報告/` are two rows instead of one duplicated pair.

use crate::engine::{EngineError, EngineResult};
use crate::models::{Entry, ExtractRequest, Overwrite};
use crate::ops::Ops;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Direct children of `prefix` ("" for the archive root, otherwise ending in `/`).
///
/// Folder rows carry the aggregated size of everything beneath them, because a
/// folder that always reads "0 B" tells the user nothing.
pub fn children(entries: &[Entry], prefix: &str) -> Vec<Entry> {
    let prefix = normalize_prefix(prefix);
    let mut folders: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut files: Vec<Entry> = Vec::new();

    for e in entries {
        let path = e.path.replace('\\', "/");
        let rest = match path.strip_prefix(prefix.as_str()) {
            Some(rest) => rest,
            None => continue,
        };
        if rest.is_empty() {
            continue; // the folder you are already standing in
        }
        // An escaping member must never be grouped into a folder called "..":
        // that would look like ordinary navigation and hide why it is blocked.
        if crate::pathguard::is_traversal(&path) {
            let mut row = e.clone();
            row.name = path.trim_end_matches('/').to_string();
            row.path = path;
            row.unsafe_path = true;
            files.push(row);
            continue;
        }
        // A directory row whose stored path has no trailing separator, which is how
        // 7z reports folders.
        if e.is_dir && !rest.contains('/') {
            let key = format!("{}{rest}/", prefix);
            folders.entry(key).or_insert((0, 0));
            continue;
        }
        match rest.split_once('/') {
            Some((head, tail)) => {
                let key = format!("{}{head}/", prefix);
                if tail.is_empty() {
                    // An explicit `dir/` entry has no bytes of its own; recording it
                    // keeps the folder visible even when it is empty.
                    folders.entry(key).or_insert((0, 0));
                } else {
                    let row = folders.entry(key).or_insert((0, 0));
                    row.0 += e.size;
                    row.1 += e.packed.unwrap_or(0);
                }
            }
            None => {
                if rest.ends_with('/') {
                    let key = format!("{}{rest}", prefix);
                    folders.entry(key).or_insert((0, 0));
                } else {
                    let mut child = e.clone();
                    child.name = rest.to_string();
                    child.path = format!("{}{rest}", prefix);
                    files.push(child);
                }
            }
        }
    }

    let mut out: Vec<Entry> = Vec::new();
    for (path, (size, packed)) in folders {
        let name = path[prefix.len()..].trim_end_matches('/').to_string();
        out.push(Entry {
            name,
            path,
            is_dir: true,
            size,
            packed: (packed > 0).then_some(packed),
            mtime: None,
            attrs: "D".into(),
            encrypted: false,
            codec: None,
            unsafe_path: false,
        });
    }
    out.extend(files);
    out.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => return std::cmp::Ordering::Less,
            (false, true) => return std::cmp::Ordering::Greater,
            _ => {}
        }
        a.name.cmp(&b.name)
    });
    out
}

fn normalize_prefix(prefix: &str) -> String {
    let unified = prefix.replace('\\', "/");
    if unified.is_empty() || unified == "/" {
        return String::new();
    }
    let trimmed = unified.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    format!("{trimmed}/")
}

/// A workspace folder for nested archives, so an inner zip is opened from a real
/// file the engine can re-read instead of a stream we would have to invent.
pub fn scratch_dir(archive: &Path) -> PathBuf {
    let stem = archive
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".into());
    let hash = crate::human::simple_hash(&archive.to_string_lossy());
    std::env::temp_dir().join(format!("quickarc-{stem}-{hash:016x}"))
}

/// Extract a single member that is itself an archive and list it.  Returns the
/// scratch file plus its listing so the caller can push a breadcrumb level.
pub fn open_nested(
    ops: &Ops,
    archive: &Path,
    member: &str,
    password: Option<&str>,
) -> EngineResult<(PathBuf, crate::models::ArchiveInfo)> {
    let scratch = scratch_dir(archive);
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| EngineError::Io(e.to_string()))?;
    let req = ExtractRequest {
        archive: archive.to_string_lossy().into_owned(),
        dest: scratch.to_string_lossy().into_owned(),
        members: vec![member.to_string()],
        password: password.map(|s| s.to_string()),
        overwrite: Overwrite::Always,
        repair_names: true,
        test_only: false,
    };
    let done = ops.extract(&req, &mut |_| {}, None)?;
    if done.items == 0 {
        return Err(EngineError::Corrupt(format!("找不到條目：{member}")));
    }
    let inner = pathguard_join(&scratch, member)?;
    let info = ops.list(&inner, password)?;
    Ok((inner, info))
}

fn pathguard_join(root: &Path, member: &str) -> EngineResult<PathBuf> {
    crate::pathguard::safe_join(root, member).map_err(|e| EngineError::PathTraversal(e.to_string()))
}
