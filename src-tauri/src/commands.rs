//! The command surface the React layer invokes.  Everything here is thin on
//! purpose: policy lives in qarc-core so it stays testable without a window.
//!
//! Long operations are jobs, not blocking calls.  A synchronous `extract` command
//! holds the IPC thread, so the UI could neither show progress nor cancel.

use qarc_core::{
    ArchiveInfo, CancelToken, CompressOptions, EngineReport, ExtractRequest, Job, JobState, Jobs,
    Ops, Outcome, Settings,
};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, State};

pub struct Shared {
    pub ops: Arc<Ops>,
    pub jobs: Arc<Jobs>,
    pub settings: Mutex<Settings>,
    /// Set from the command line: this is what the Explorer verbs and
    /// "open with" hand us, and the page does not exist yet when main runs.
    pub startup_path: Mutex<Option<String>>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub jobs: Vec<Job>,
    /// Drained since the previous poll, so the UI cannot react to one job twice.
    pub events: Vec<Job>,
    pub busy: bool,
}

fn base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Run the work on a worker thread, streaming progress into the registry.
fn spawn_job(
    app: &AppHandle,
    kind: &'static str,
    label: String,
    detail: String,
    work: impl FnOnce(Arc<Ops>, u64, CancelToken) -> Outcome + Send + 'static,
) -> Result<u64, String> {
    let shared = app.state::<Shared>();
    let (id, token) = shared
        .jobs
        .begin(kind, label, detail)
        .ok_or_else(|| "工作佇列已滿，請等目前的作業結束".to_string())?;
    let (ops, jobs) = (shared.ops.clone(), shared.jobs.clone());
    std::thread::spawn(move || {
        let outcome = work(ops, id, token);
        let state = if outcome.cancelled {
            JobState::Cancelled
        } else if outcome.ok {
            JobState::Done
        } else {
            JobState::Failed
        };
        let error = if outcome.cancelled {
            Some("已取消".to_string())
        } else if outcome.ok {
            None
        } else {
            Some(outcome.errors.join("；"))
        };
        jobs.finish(id, state, outcome.items, outcome.bytes_out, outcome.blocked.len(), error);
    });
    Ok(id)
}

/// An engine error becomes an Outcome, so cancellation reads as partial work
/// rather than as a crash the user cannot interpret.
fn settle(result: qarc_core::EngineResult<Outcome>) -> Outcome {
    result.unwrap_or_else(|e| Outcome {
        ok: false,
        cancelled: matches!(e, qarc_core::EngineError::Cancelled),
        errors: vec![e.to_string()],
        ..Default::default()
    })
}

/// One-shot: the UI asks once at mount and gets the path, if any.
#[tauri::command]
pub fn take_startup_path(shared: State<'_, Shared>) -> Option<String> {
    shared.startup_path.lock().ok().and_then(|mut p| p.take())
}

#[tauri::command]
pub fn open_archive(
    archive: String,
    password: Option<String>,
    shared: State<'_, Shared>,
) -> Result<ArchiveInfo, String> {
    let info = shared
        .ops
        .list(Path::new(&archive), password.as_deref())
        .map_err(|e| e.to_string())?;
    if let Ok(mut s) = shared.settings.lock() {
        s.record(&archive);
        let _ = s.save();
    }
    Ok(info)
}

/// Open an archive stored inside another one.  The inner file is materialised in a
/// scratch folder so every engine can re-read it as a normal path.
#[tauri::command]
pub fn open_member(
    archive: String,
    member: String,
    password: Option<String>,
    shared: State<'_, Shared>,
) -> Result<ArchiveInfo, String> {
    let (path, info) =
        qarc_core::open_nested(&shared.ops, Path::new(&archive), &member, password.as_deref())
            .map_err(|e| e.to_string())?;
    Ok(ArchiveInfo {
        path: path.to_string_lossy().into_owned(),
        ..info
    })
}

#[tauri::command]
pub fn start_extract(app: AppHandle, req: ExtractRequest) -> Result<u64, String> {
    let detail = if req.test_only {
        "測試完整性"
    } else if req.members.is_empty() {
        "解壓全部"
    } else {
        "解壓選取項目"
    };
    let kind = if req.test_only { "test" } else { "extract" };
    let jobs = app.state::<Shared>().jobs.clone();
    spawn_job(
        &app,
        kind,
        base_name(&req.archive),
        detail.into(),
        move |ops, id, token| {
            let report = &mut |p| jobs.report(id, &p);
            settle(ops.extract(&req, report, Some(&token)))
        },
    )
}

#[tauri::command]
pub fn start_compress(app: AppHandle, opts: CompressOptions) -> Result<u64, String> {
    let jobs = app.state::<Shared>().jobs.clone();
    spawn_job(
        &app,
        "compress",
        base_name(&opts.archive),
        "壓縮中".into(),
        move |ops, id, token| {
            let report = &mut |p| jobs.report(id, &p);
            settle(ops.create(&opts, report, Some(&token)))
        },
    )
}

#[tauri::command]
pub fn job_snapshot(shared: State<'_, Shared>) -> Snapshot {
    let jobs = shared.jobs.snapshot();
    let events = shared
        .jobs
        .take_events()
        .into_iter()
        .map(|(_, job)| job)
        .collect();
    let busy = jobs.iter().any(|j| j.state == JobState::Running);
    Snapshot { jobs, events, busy }
}

#[tauri::command]
pub fn cancel_job(shared: State<'_, Shared>, id: u64) -> bool {
    shared.jobs.cancel(id)
}

#[tauri::command]
pub fn clear_jobs(shared: State<'_, Shared>) -> usize {
    shared.jobs.clear_finished()
}

#[tauri::command]
pub fn hash_file(path: String, algo: String) -> Result<String, String> {
    let algo = match algo.to_lowercase().as_str() {
        "md5" => qarc_core::Algo::Md5,
        _ => qarc_core::Algo::Sha256,
    };
    qarc_core::digest_file(Path::new(&path), algo).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_engines(shared: State<'_, Shared>) -> Vec<EngineReport> {
    shared.ops.fleet_report()
}

#[tauri::command]
pub fn get_settings(shared: State<'_, Shared>) -> Settings {
    shared.settings.lock().map(|s| s.clone()).unwrap_or_default()
}

#[tauri::command]
pub fn set_settings(settings: Settings, shared: State<'_, Shared>) -> Result<(), String> {
    // Save first: if the disk rejects it the in-memory copy stays untouched, so the
    // UI never claims a setting that will vanish on restart.
    settings.save().map_err(|e| e.to_string())?;
    let mut current = shared.settings.lock().map_err(|e| e.to_string())?;
    *current = settings;
    Ok(())
}

#[tauri::command]
pub fn clear_history(shared: State<'_, Shared>) -> Result<(), String> {
    let mut current = shared.settings.lock().map_err(|e| e.to_string())?;
    current.history.clear();
    current.save().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn shell_preview_text() -> String {
    crate::shell::reg_preview(
        &std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
    )
}

#[tauri::command]
pub fn shell_is_installed() -> bool {
    crate::shell::installed()
}

#[tauri::command]
pub fn recycle_paths(paths: Vec<String>) -> Result<(), String> {
    crate::shell::recycle(&paths)
}
