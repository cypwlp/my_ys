//! QuickArc core, Tauri refactor.
//!
//! Same contract as the .NET `QuickArc.Core` it is being ported from: engines are
//! discovered rather than assumed, an engine may only be replaced when it cannot
//! do the job, archive-controlled paths never reach the filesystem unguarded, and
//! a password is never echoed.

pub mod browse;
pub mod cancel;
pub mod codec;
pub mod codec_engine;
pub mod engine;
pub mod formats;
pub mod human;
pub mod hash;
pub mod jobs;
pub mod settings;
pub mod models;
pub mod ops;
pub mod pathguard;
pub mod repair;
pub mod sevenzip;
pub mod tar;
pub mod zip_engine;

pub use engine::{Engine, EngineError, EngineResult, Op, Registry};
pub use formats::Format;
pub use models::{ArchiveInfo, CompressOptions, Entry, ExtractRequest, Outcome, Overwrite, Progress};
pub use browse::{children, open_nested, scratch_dir};
pub use cancel::{Cancel, CancelToken};
pub use codec_engine::CodecEngine;
pub use ops::{EngineReport, Ops};
pub use zip_engine::ZipEngine;
pub use hash::{Algo, digest_file};
pub use jobs::{Job, JobState, Jobs};
pub use settings::Settings;

/// Version of the port itself, so a screenshot can be tied to a build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
