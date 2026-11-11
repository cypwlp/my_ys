//! Data shapes shared by every engine and shipped straight to the UI over Tauri's
//! IPC, so field names are camelCase on the wire.

use crate::formats::Format;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineKind {
    #[default]
    Managed,
    CommandLine,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// Name as displayed, after legacy-encoding repair.
    pub name: String,
    /// Full path inside the archive; this is the key extraction is asked for.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub packed: Option<u64>,
    /// Unix seconds; `None` when the format carries no timestamp.
    pub mtime: Option<i64>,
    pub attrs: String,
    pub encrypted: bool,
    /// Codepage label when the name had to be repaired, e.g. "GB18030".
    pub codec: Option<String>,
    /// Member name would escape the destination; extraction skips it by default.
    pub unsafe_path: bool,
}

impl Entry {
    pub fn ratio(&self) -> Option<f64> {
        match (self.packed, self.size) {
            (Some(p), s) if s > 0 && p <= s => Some(1.0 - p as f64 / s as f64),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveInfo {
    pub path: String,
    pub format: Format,
    pub engine: String,
    pub entries: Vec<Entry>,
    pub total_size: u64,
    pub total_packed: u64,
    pub needs_password: bool,
    pub is_split: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Overwrite {
    #[default]
    Ask,
    Always,
    Skip,
    KeepBoth,
    Newer,
}

impl Overwrite {
    /// The four codes 7-Zip actually accepts, or None when the policy has to be
    /// decided by the core instead of handed to the engine.
    pub fn sevenzip_code(self) -> Option<char> {
        match self {
            Overwrite::Always => Some('s'),
            Overwrite::Skip => Some('t'),
            Overwrite::KeepBoth => Some('u'),
            Overwrite::Ask | Overwrite::Newer => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// 0.0..=1.0; `None` when the engine cannot know the total.
    pub fraction: Option<f64>,
    pub done: u64,
    pub total: u64,
    pub summary: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractRequest {
    pub archive: String,
    pub dest: String,
    /// Empty means "everything".
    pub members: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    pub overwrite: Overwrite,
    pub repair_names: bool,
    pub test_only: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub ok: bool,
    /// The user stopped it.  Distinct from failure: partial work is real and the
    /// files already written must not be described as an error.
    pub cancelled: bool,
    pub items: usize,
    pub bytes_out: u64,
    pub errors: Vec<String>,
    pub blocked: Vec<String>,
    pub engine: String,
}

impl Outcome {
    pub fn fail(&mut self, msg: impl Into<String>) {
        self.ok = false;
        self.errors.push(msg.into());
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressOptions {
    pub archive: String,
    pub sources: Vec<String>,
    pub format: Format,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    pub header_encryption: bool,
    /// 0..=9, mapped onto each engine's own scale.
    pub level: u8,
    pub threads: u8,
}
