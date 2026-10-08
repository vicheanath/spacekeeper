//! SpaceKeeper desktop application.
//!
//! This crate is the wiring: it builds the application state, registers the
//! commands and starts Tauri. All the logic — and every safety decision —
//! lives in `spacekeeper-core`, where it is unit tested without a window.

#![warn(missing_docs)]

mod commands;
mod state;

use state::AppState;

/// Start the application.
///
/// # Panics
///
/// Panics if the application state cannot be built (usually a database that
/// cannot be created) or if Tauri fails to start. Both mean there is no
/// working application to show, so failing loudly is the honest outcome.
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "spacekeeper=info,warn".into()),
        )
        .init();

    let state = AppState::new().expect("failed to initialise SpaceKeeper state");

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::list_scanners,
            commands::start_scan,
            commands::cancel_scan,
            commands::pause_scan,
            commands::resume_scan,
            commands::scan_status,
            commands::preview_cleanup,
            commands::run_cleanup,
            commands::undo_cleanup,
            commands::undo_supported,
            commands::browse_directory,
            commands::browse_token,
            commands::delete_browsed,
            commands::list_disks,
            commands::explain_disk,
            commands::scan_history,
            commands::cleanup_history,
            commands::total_freed,
            commands::disk_trend,
            commands::scanner_stats,
            commands::get_settings,
            commands::save_settings,
            commands::ignored_paths,
            commands::add_ignored_path,
            commands::remove_ignored_path,
            commands::protected_paths,
            commands::reveal_path,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start SpaceKeeper");
}
