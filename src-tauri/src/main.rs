#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod shell;

use std::sync::{Arc, Mutex};

fn main() {
    let startup = std::env::args()
        .skip(1)
        .find(|a| !a.starts_with('-') && std::path::Path::new(a).is_file());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::Shared {
            ops: Arc::new(qarc_core::Ops::discover()),
            jobs: Arc::new(qarc_core::Jobs::default()),
            settings: Mutex::new(qarc_core::Settings::load()),
            startup_path: Mutex::new(startup),
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_archive,
            commands::take_startup_path,
            commands::open_member,
            commands::start_extract,
            commands::start_compress,
            commands::job_snapshot,
            commands::cancel_job,
            commands::clear_jobs,
            commands::hash_file,
            commands::list_engines,
            commands::get_settings,
            commands::set_settings,
            commands::clear_history,
            commands::shell_preview_text,
            commands::shell_is_installed,
            commands::recycle_paths
        ])
        .run(tauri::generate_context!())
        .expect("QuickArc 啟動失敗");
}
