//! Engine contract and the fallback policy that goes with it.
//!
//! The rule that matters: an engine may only be replaced when it *cannot do the
//! job*.  A wrong password, a corrupt volume or a user cancel must surface as
//! itself — silently retrying those on another engine is how an archiver ends up
//! reporting success on a file it never opened.

use crate::formats::Format;
use crate::cancel::Cancel;
use crate::models::{ArchiveInfo, CompressOptions, EngineKind, ExtractRequest, Outcome, Progress};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    /// This engine has no code path for the format at all.
    UnsupportedFormat(String),
    /// The binary is missing, not executable, or too old.
    EngineUnavailable(String),
    WrongPassword,
    EncryptedHeaders,
    PasswordRequired,
    Corrupt(String),
    PathTraversal(String),
    DiskFull,
    Cancelled,
    Io(String),
}

impl EngineError {
    /// Capability faults only.  Everything else is data or user intent, and
    /// re-running it elsewhere just hides it.
    pub fn is_capability_fault(&self) -> bool {
        matches!(self, EngineError::UnsupportedFormat(_) | EngineError::EngineUnavailable(_))
    }

    pub fn hint(&self) -> Option<&'static str> {
        Some(match self {
            EngineError::EncryptedHeaders => "此 7z 檔的檔頭已加密，需密碼才能列出內容",
            EngineError::WrongPassword => "密碼錯誤",
            EngineError::PasswordRequired => "這個壓縮包需要密碼",
            EngineError::PathTraversal(_) => "已阻擋越界路徑，其餘檔案照常解出",
            EngineError::EngineUnavailable(_) => "請安裝 7-Zip 或在設定中指定路徑",
            EngineError::DiskFull => "磁碟空間不足",
            _ => return None,
        })
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            EngineError::UnsupportedFormat(x) => format!("不支援的格式：{x}"),
            EngineError::EngineUnavailable(x) => format!("引擎不可用：{x}"),
            EngineError::WrongPassword => "密碼錯誤".into(),
            EngineError::EncryptedHeaders => "檔頭已加密，需要密碼".into(),
            EngineError::PasswordRequired => "需要密碼".into(),
            EngineError::Corrupt(x) => format!("壓縮包損毀：{x}"),
            EngineError::PathTraversal(x) => format!("越界路徑：{x}"),
            EngineError::DiskFull => "磁碟空間不足".into(),
            EngineError::Cancelled => "已取消".into(),
            EngineError::Io(x) => format!("I/O 錯誤：{x}"),
        };
        match self.hint() {
            Some(h) if !s.contains(h) => write!(f, "{s}（{h}）"),
            _ => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for EngineError {}

pub type EngineResult<T> = Result<T, EngineError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Read,
    Write,
}

pub trait Engine: Send + Sync {
    fn id(&self) -> &'static str;
    fn kind(&self) -> EngineKind;
    fn supports(&self, format: Format, op: Op) -> bool;
    /// Higher wins.  Negative means "cannot", which keeps the registry a plain sort.
    fn priority(&self, format: Format, op: Op) -> i32 {
        if self.supports(format, op) {
            0
        } else {
            -1
        }
    }

    fn list(&self, archive: &Path, password: Option<&str>) -> EngineResult<ArchiveInfo>;

    /// Where the engine came from, for the settings pane.  Managed engines are
    /// compiled in and answer None; command-line ones name the binary.
    fn location(&self) -> Option<String> {
        None
    }

    /// (format label, can-create) pairs, walked from the shared table so a new
    /// format appears in the UI without the engine having to know about it.
    fn matrix(&self) -> Vec<(crate::formats::Format, bool)> {
        crate::formats::all()
            .iter()
            .filter_map(|row| {
                let read = self.supports(row.format, Op::Read);
                let write = self.supports(row.format, Op::Write);
                read.then_some((row.format, write))
            })
            .collect()
    }

    /// The plain entry points delegate to the cancel-aware ones, so an engine only
    /// ever implements the `_cancellable` pair.  Engines that can only read leave
    /// those alone, which the registry turns into a fallback rather than a dead end.
    fn extract(&self, req: &ExtractRequest, report: &mut dyn FnMut(Progress)) -> EngineResult<Outcome> {
        self.extract_cancellable(req, report, None)
    }

    /// Cancel-aware engines override this.  The default ignores the token, which
    /// is honest: a job that cannot be stopped should not pretend it can.
    fn extract_cancellable(
        &self,
        req: &ExtractRequest,
        report: &mut dyn FnMut(Progress),
        _cancel: Cancel<'_>,
    
    ) -> EngineResult<Outcome> {
        self.extract(req, report)
    }

    fn create(&self, opts: &CompressOptions, report: &mut dyn FnMut(Progress)) -> EngineResult<Outcome> {
        self.create_cancellable(opts, report, None)
    }

    fn create_cancellable(
        &self,
        opts: &CompressOptions,
        report: &mut dyn FnMut(Progress),
        _cancel: Cancel<'_>,
    
    ) -> EngineResult<Outcome> {
        self.create(opts, report)
    }

    fn test(&self, req: &ExtractRequest, report: &mut dyn FnMut(Progress)) -> EngineResult<Outcome> {
        let probe = ExtractRequest { test_only: true, ..req.clone() };
        self.extract(&probe, report)
    }
}

/// Ordered candidate list for one operation.  The caller walks it and only moves to
/// the next entry when the previous one failed for a capability reason.
pub struct Registry {
    engines: Vec<Box<dyn Engine>>,
}

impl Registry {
    pub fn new(engines: Vec<Box<dyn Engine>>) -> Self {
        Self { engines }
    }

    pub fn add(&mut self, engine: Box<dyn Engine>) {
        self.engines.push(engine);
    }

    pub fn candidates(&self, format: Format, op: Op) -> Vec<&dyn Engine> {
        let mut pairs: Vec<(i32, &dyn Engine)> = self
            .engines
            .iter()
            .map(|e| (e.priority(format, op), e.as_ref()))
            .filter(|(p, _)| *p >= 0)
            .collect();
        pairs.sort_by(|a, b| b.0.cmp(&a.0));
        pairs.into_iter().map(|(_, e)| e).collect()
    }

    pub fn engines(&self) -> &[Box<dyn Engine>] {
        &self.engines
    }

    pub fn ids(&self) -> Vec<&'static str> {
        self.engines.iter().map(|e| e.id()).collect()
    }
}

/// Run `attempt` against each candidate in order, stopping at the first success and
/// re-throwing the first non-capability failure.  Returns the last error if every
/// engine was simply unable.
pub fn with_fallback<T>(
    candidates: &[&dyn Engine],
    mut attempt: impl FnMut(&dyn Engine) -> EngineResult<T>,
) -> EngineResult<T> {
    let mut last: Option<EngineError> = None;
    for engine in candidates {
        match attempt(*engine) {
            Ok(v) => return Ok(v),
            Err(e) if e.is_capability_fault() => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| EngineError::EngineUnavailable("無可用引擎".into())))
}
