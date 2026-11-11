//! Path hygiene for archive members.  An archive's member names are attacker
//! controlled text; this is the only thing standing between `../../windows/x` and
//! a write outside the destination.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// `../` escaping the destination, including after normalisation.
    Traversal(String),
    /// Absolute path or drive letter — never taken from an archive.
    Absolute(String),
    /// A component that is not a plain segment (root, prefix, verbatim `\\?\`).
    WeirdComponent(String),
    /// The destination itself could not be resolved.
    Io(String),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            PathError::Traversal(n) => format!("路徑嘗試逃出目標資料夾：{n}"),
            PathError::Absolute(n) => format!("壓縮包內含絕對路徑，已阻擋：{n}"),
            PathError::WeirdComponent(n) => format!("無法安全解析的路徑成分：{n}"),
            PathError::Io(n) => format!("檔案系統錯誤：{n}"),
        };
        f.write_str(&s)
    }
}

impl std::error::Error for PathError {}

/// True when a member name would land outside the extraction root.  Catching this
/// separately from `safe_join` lets the UI list blocked members instead of aborting.
pub fn is_traversal(name: &str) -> bool {
    let unified = name.replace('\\', "/");
    if Path::new(&unified).is_absolute() || unified.starts_with('/') {
        return true;
    }
    // Windows drive prefix, e.g. "C:/" or "c:\\".
    if unified.len() >= 2 && unified.as_bytes()[1] == b':' && unified.as_bytes()[0].is_ascii_alphabetic()
    {
        return true;
    }
    let mut depth = 0i32;
    for part in unified.split('/') {
        match part {
            "" | "." => {}
            ".." => depth -= 1,
            _ => depth += 1,
        }
        if depth < 0 {
            return true;
        }
    }
    false
}

/// Normalise separators and drop `.` segments; keeps case, drops leading slashes.
pub fn normalize(name: &str) -> String {
    let unified = name.replace('\\', "/");
    let parts: Vec<&str> = unified.split('/').filter(|p| *p != "." && !p.is_empty()).collect();
    parts.join("/")
}

/// Join a member name under `root`, refusing anything that escapes it.
///
/// Two guards, not one: the lexical check catches `../` before anything touches the
/// disk, and the canonical check catches a pre-existing symlink inside `root` that
/// would otherwise redirect the write.
pub fn safe_join(root: &Path, name: &str) -> Result<PathBuf, PathError> {
    if is_traversal(name) {
        return Err(if name.contains("..") {
            PathError::Traversal(name.to_string())
        } else {
            PathError::Absolute(name.to_string())
        });
    }
    let clean = normalize(name);
    if clean.is_empty() {
        return Err(PathError::WeirdComponent(name.to_string()));
    }
    let mut out = root.to_path_buf();
    for part in clean.split('/') {
        match Path::new(part).components().next() {
            Some(Component::Normal(_)) => out.push(part),
            _ => return Err(PathError::WeirdComponent(part.to_string())),
        }
    }
    // If an ancestor already exists as a link, writing through it escapes the root.
    if let Ok(existing) = out.canonicalize() {
        let root_real = root
            .canonicalize()
            .map_err(|e| PathError::Io(e.to_string()))?;
        if !existing.starts_with(&root_real) {
            return Err(PathError::Traversal(name.to_string()));
        }
    } else if out.symlink_metadata().is_ok() {
        return Err(PathError::Traversal(name.to_string()));
    }
    Ok(out)
}

/// `report.txt` -> `report (2).txt` when the first is taken.  Never overwrites.
pub fn unique_name(target: &Path) -> PathBuf {
    if !target.exists() {
        return target.to_path_buf();
    }
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = target
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    let dir = target.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    target.to_path_buf()
}
