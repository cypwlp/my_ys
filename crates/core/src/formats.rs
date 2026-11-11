//! Format registry.  One table drives detection, capability claims and the
//! extension the UI offers, so a new format is one row rather than five edits.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Format {
    Zip,
    SevenZip,
    Tar,
    GZip,
    BZip2,
    Xz,
    Zstd,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    Rar,
    Split,
    #[default]
    Unknown,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FormatInfo {
    pub format: Format,
    pub label: &'static str,
    /// Extension without the dot, lower-case, as it appears on disk.
    pub extension: &'static str,
    pub readable: bool,
    pub writable: bool,
    /// True when the format is a container inside a container (tar.gz): the
    /// compression layer and the archive layer are handled by different engines.
    pub compound: bool,
}

/// Longest-suffix-first: `.tar.gz` must beat `.gz`, or a tarball is offered as a
/// single-file gzip and the archive inside it disappears.
static TABLE: &[FormatInfo] = &[
    FormatInfo { format: Format::SevenZip, label: "7z", extension: "7z", readable: true, writable: true, compound: false },
    FormatInfo { format: Format::Zip, label: "ZIP", extension: "zip", readable: true, writable: true, compound: false },
    FormatInfo { format: Format::Rar, label: "RAR", extension: "rar", readable: true, writable: false, compound: false },
    FormatInfo { format: Format::TarGz, label: "tar.gz", extension: "tar.gz", readable: true, writable: true, compound: true },
    FormatInfo { format: Format::TarBz2, label: "tar.bz2", extension: "tar.bz2", readable: true, writable: true, compound: true },
    FormatInfo { format: Format::TarXz, label: "tar.xz", extension: "tar.xz", readable: true, writable: false, compound: true },
    FormatInfo { format: Format::TarZst, label: "tar.zst", extension: "tar.zst", readable: true, writable: true, compound: true },
    FormatInfo { format: Format::Tar, label: "tar", extension: "tar", readable: true, writable: true, compound: false },
    FormatInfo { format: Format::GZip, label: "gzip", extension: "gz", readable: true, writable: true, compound: false },
    FormatInfo { format: Format::BZip2, label: "bzip2", extension: "bz2", readable: true, writable: true, compound: false },
    FormatInfo { format: Format::Xz, label: "xz", extension: "xz", readable: true, writable: false, compound: false },
    FormatInfo { format: Format::Zstd, label: "zstd", extension: "zst", readable: true, writable: true, compound: false },
    FormatInfo { format: Format::Split, label: "分卷", extension: "001", readable: true, writable: false, compound: false },
];

pub fn info(format: Format) -> &'static FormatInfo {
    TABLE.iter().find(|i| i.format == format).unwrap_or(&TABLE[0])
}

pub fn all() -> &'static [FormatInfo] {
    TABLE
}

impl Format {
    pub fn label(self) -> &'static str {
        info(self).label
    }

    pub fn is_compound(self) -> bool {
        info(self).compound
    }
}

/// Extension-driven detection.  Compound suffixes are tested first; a bare `.gz`
/// that is really `foo.tar.gz` is the bug this ordering exists to prevent.
pub fn from_extension(path: &str) -> Format {
    let lower = path.to_lowercase();
    let name = lower.rsplit(['/', '\\']).next().unwrap_or(&lower);
    let mut best: Option<&FormatInfo> = None;
    for row in TABLE {
        let needle = format!(".{}", row.extension);
        if name.ends_with(&needle) {
            match best {
                Some(b) if b.extension.len() >= row.extension.len() => {}
                _ => best = Some(row),
            }
        }
    }
    if best.is_none() {
        if let Some(tail) = name.rsplit('.').next() {
            if tail.len() == 3 && tail.chars().all(|c| c.is_ascii_digit()) && !tail.is_empty() {
                // .001/.002 style volumes are only a split archive if a sibling exists;
                // the caller decides, we just refuse to call it "unknown".
                if tail == "001" || tail == "002" {
                    return Format::Split;
                }
            }
        }
    }
    best.map(|i| i.format).unwrap_or(Format::Unknown)
}

/// Magic-number sniffing, used when the extension lies (or is missing).  Returns
/// the format only when the signature is unambiguous.
pub fn from_magic(head: &[u8]) -> Option<Format> {
    let starts = |sig: &[u8]| head.len() >= sig.len() && &head[..sig.len()] == sig;
    if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") || starts(b"PK\x07\x08") {
        return Some(Format::Zip);
    }
    if starts(b"7z\x1a\x07\x00") {
        return Some(Format::SevenZip);
    }
    if starts(b"Rar!\x1a\x07") {
        return Some(Format::Rar);
    }
    if head.len() >= 265 && &head[257..265] == b"ustar\x0000" {
        return Some(Format::Tar);
    }
    if starts(b"\x1f\x8b") {
        return Some(Format::GZip);
    }
    if starts(b"BZh") {
        return Some(Format::BZip2);
    }
    if starts(b"\xfd7zXZ\x00") {
        return Some(Format::Xz);
    }
    if starts(&[0x28, 0xb5, 0x2f, 0xfd]) {
        return Some(Format::Zstd);
    }
    None
}

/// Extension plus magic bytes.  Magic decides the container, the extension decides
/// whether a gzip blob is a lone `.gz` or the compression layer of a `.tar.gz`;
/// trusting either one alone gets tarballs wrong.
pub fn sniff(path: &std::path::Path) -> Format {
    let by_name = from_extension(&path.to_string_lossy());
    let head = read_head(path, 512);
    let by_magic = head.as_deref().and_then(from_magic);
    match (by_magic, by_name) {
        (Some(Format::GZip), Format::TarGz) => Format::TarGz,
        (Some(Format::BZip2), Format::TarBz2) => Format::TarBz2,
        (Some(Format::Xz), Format::TarXz) => Format::TarXz,
        (Some(Format::Zstd), Format::TarZst) => Format::TarZst,
        (Some(m), _) => m,
        (None, n) => n,
    }
}

fn read_head(path: &std::path::Path, n: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; n];
    let got = file.read(&mut buf).ok()?;
    buf.truncate(got);
    Some(buf)
}
