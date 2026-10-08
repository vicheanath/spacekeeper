//! The command surface the React app talks to.
//!
//! Every command is thin: it validates, delegates to `spacekeeper-core`, and
//! serialises the result. No safety decision is made here — that would put it
//! outside the reach of the core's tests.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use spacekeeper_core::browse::{list_directory, measure_entries, DirectoryListing, Measured, SortBy};
use spacekeeper_core::cleanup::{
    Authority, CleanupEngine, CleanupItem, CleanupMode, CleanupRecord, CleanupRequest,
};
use spacekeeper_core::db::{DiskSnapshot, ScanHistoryEntry, ScannerStats};
use spacekeeper_core::explain::{explain, Explanation};
use spacekeeper_core::protect::ProtectedEntry;
use spacekeeper_core::{
    disk, Bytes, ProgressReporter, RiskLevel, ScanOptions, ScanReport, ScannerMetadata,
};
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;

/// Event name for progress updates.
pub const PROGRESS_EVENT: &str = "scan://progress";

/// Event name for streamed directory sizes.
pub const MEASURED_EVENT: &str = "browse://measured";

/// One streamed directory size, tagged with the listing it belongs to.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredEvent {
    /// Generation of the listing this belongs to. The UI drops anything that
    /// does not match the folder currently on screen.
    pub token: u64,
    /// The measurement itself.
    #[serde(flatten)]
    pub measured: Measured,
}

/// Error type returned to the frontend.
///
/// Tauri needs a serialisable error; this keeps the message and a machine
/// readable kind so the UI can distinguish "nothing to do" from "went wrong".
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    /// Machine-readable discriminator.
    pub kind: &'static str,
    /// Human readable message, safe to show in the UI.
    pub message: String,
}

impl From<spacekeeper_core::Error> for CommandError {
    fn from(error: spacekeeper_core::Error) -> Self {
        let kind = match &error {
            spacekeeper_core::Error::Cancelled => "cancelled",
            spacekeeper_core::Error::ScannerUnavailable { .. } => "unavailable",
            spacekeeper_core::Error::ProtectedPath(_) => "protected",
            spacekeeper_core::Error::NotDeletable(_)
            | spacekeeper_core::Error::ConfirmationRequired(_) => "refused",
            spacekeeper_core::Error::Database(_) => "database",
            _ => "internal",
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

/// Result alias for commands.
pub type CommandResult<T> = Result<T, CommandError>;

// ------------------------------------------------------------------ scanning

/// Metadata for every registered scanner, for the settings screen.
#[tauri::command]
pub fn list_scanners(state: State<'_, AppState>) -> Vec<ScannerMetadata> {
    state.engine().registry().metadata()
}

/// Run a scan.
///
/// Progress is streamed on the [`PROGRESS_EVENT`] channel while this runs; the
/// return value is the finished report.
#[tauri::command]
pub async fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    scanners: Option<Vec<String>>,
) -> CommandResult<ScanReport> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let context = state.begin_scan(ProgressReporter::channel(tx));

    // Forward progress to the webview. If the window has gone, the send fails
    // and the loop ends; the scan itself is unaffected.
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if app.emit(PROGRESS_EVENT, &event).is_err() {
                break;
            }
        }
    });

    let only: Option<HashSet<String>> = scanners.map(|ids| ids.into_iter().collect());
    let report = state.engine().run(&context, only.as_ref()).await;

    // History is best-effort: a failure to write it must not lose the results
    // the user is waiting for.
    if let Err(err) = state.db().record_scan(&report) {
        tracing::warn!(%err, "failed to record scan history");
    }
    if let Some(disk) = disk::primary() {
        if let Err(err) = state.db().record_disk_usage(&disk) {
            tracing::warn!(%err, "failed to record disk usage");
        }
    }

    state.set_last_report(report.clone());
    Ok(report)
}

/// Ask the running scan to stop. Results found so far are kept.
#[tauri::command]
pub fn cancel_scan(state: State<'_, AppState>) {
    state.control().cancel();
}

/// Pause the running scan.
#[tauri::command]
pub fn pause_scan(state: State<'_, AppState>) {
    state.control().pause();
}

/// Resume a paused scan.
#[tauri::command]
pub fn resume_scan(state: State<'_, AppState>) {
    state.control().resume();
}

/// Whether a scan is currently paused.
#[tauri::command]
pub fn scan_status(state: State<'_, AppState>) -> ScanStatus {
    let control = state.control();
    ScanStatus {
        paused: control.is_paused(),
        cancelled: control.is_cancelled(),
    }
}

/// Live state of the running scan.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    /// Whether the scan is paused.
    pub paused: bool,
    /// Whether the scan was cancelled.
    pub cancelled: bool,
}

// ------------------------------------------------------------------ cleanup

/// A cleanup asked for by the UI, addressed by result id.
///
/// The UI sends ids, never paths: the paths come from the report the backend
/// produced, so the frontend cannot ask for an arbitrary path to be deleted.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupCommand {
    /// Ids of results to remove.
    pub item_ids: Vec<String>,
    /// Trash (default) or permanent.
    #[serde(default)]
    pub mode: CleanupMode,
}

/// Preview a cleanup without touching anything.
///
/// The confirmation dialog is driven by this, so the numbers a user confirms
/// come from the same code path that does the work.
#[tauri::command]
pub fn preview_cleanup(
    state: State<'_, AppState>,
    command: CleanupCommand,
) -> CommandResult<CleanupRecord> {
    let request = build_request(&state, &command)?.as_dry_run();
    Ok(CleanupEngine::new(state.guard()).execute(&request))
}

/// Perform a cleanup.
#[tauri::command]
pub async fn run_cleanup(
    state: State<'_, AppState>,
    command: CleanupCommand,
) -> CommandResult<CleanupRecord> {
    let request = build_request(&state, &command)?;
    let engine = CleanupEngine::new(state.guard());

    // Deleting is blocking filesystem work; keep it off the async runtime.
    let record = tokio::task::spawn_blocking(move || engine.execute(&request))
        .await
        .map_err(|err| CommandError {
            kind: "internal",
            message: err.to_string(),
        })?;

    state.db().record_cleanup(&record)?;
    Ok(record)
}

/// Restore a previous cleanup from the Trash, where the platform supports it.
#[tauri::command]
pub fn undo_cleanup(state: State<'_, AppState>, cleanup_id: String) -> CommandResult<Vec<PathBuf>> {
    let record = state
        .db()
        .cleanup_history(200)?
        .into_iter()
        .find(|record| record.id == cleanup_id)
        .ok_or_else(|| CommandError {
            kind: "not_found",
            message: "that cleanup is no longer in the history".into(),
        })?;

    Ok(spacekeeper_core::cleanup::undo(&record)?)
}

/// Whether this platform can restore from the Trash programmatically.
#[tauri::command]
pub fn undo_supported() -> bool {
    spacekeeper_core::cleanup::undo_supported()
}

/// Turn the UI's item ids into a validated request against the last report.
fn build_request(
    state: &State<'_, AppState>,
    command: &CleanupCommand,
) -> CommandResult<CleanupRequest> {
    let report = state.last_report().ok_or_else(|| CommandError {
        kind: "no_scan",
        message: "run a scan before cleaning up".into(),
    })?;

    let wanted: HashSet<&str> = command.item_ids.iter().map(String::as_str).collect();
    let items = report
        .results
        .iter()
        .filter(|result| wanted.contains(result.id.as_str()))
        // An id the backend never marked deletable cannot be resurrected by
        // asking for it: the request is built from the report, not from input.
        .filter(|result| result.deletable)
        .map(Into::into)
        .collect();

    Ok(CleanupRequest {
        items,
        mode: command.mode,
        dry_run: false,
        // Scan-driven cleanup never gets elevated authority. The explorer is
        // the only caller that may set it, and it does so on its own path.
        authority: Authority::Automated,
    })
}

// -------------------------------------------------------------- information

// ------------------------------------------------------------------ explorer

/// List a directory, with every child measured.
///
/// Opening at `null` starts in the home directory. Paths outside the allowed
/// roots are refused rather than clamped, so a mistake is visible instead of
/// silently landing somewhere else.
#[tauri::command]
pub async fn browse_directory(
    app: AppHandle,
    state: State<'_, AppState>,
    path: Option<PathBuf>,
    sort: Option<SortBy>,
) -> CommandResult<DirectoryListing> {
    let _ = sort; // ordering is applied client-side as sizes stream in
    let root = state.home().to_path_buf();
    let target = path.unwrap_or_else(|| root.clone());

    if !target.starts_with(&root) {
        return Err(CommandError {
            kind: "protected",
            message: "SpaceKeeper only browses inside your home folder".into(),
        });
    }

    let guard = state.guard();

    // Listing is cheap — one `read_dir` and a `stat` per child — so it happens
    // inline and the user sees the folder immediately.
    let listing = {
        let target = target.clone();
        let root = root.clone();
        let cache_owner = state.inner();
        tokio::task::block_in_place(move || {
            list_directory(&target, &[root], &guard, cache_owner.size_cache())
        })
    }?;

    state.remember_listing(listing.entries.iter().map(|entry| entry.path.clone()));

    // Measuring is the expensive part, so it runs in the background and each
    // size is pushed to the UI as it lands. Taking a new token here cancels
    // the previous folder's pass.
    let (token, control) = state.begin_browse();
    let pending: Vec<_> = listing
        .entries
        .iter()
        .filter(|entry| !entry.measured && !entry.is_symlink)
        .cloned()
        .collect();

    if !pending.is_empty() {
        let cache = state.inner().size_cache_handle();
        tauri::async_runtime::spawn_blocking(move || {
            measure_entries(&pending, &control, &cache, |measured| {
                // A dropped receiver (window closed) is not worth reacting to.
                let _ = app.emit(MEASURED_EVENT, MeasuredEvent { token, measured });
            });
        });
    }

    Ok(DirectoryListing { ..listing })
}

/// The generation token for the listing currently on screen.
#[tauri::command]
pub fn browse_token(state: State<'_, AppState>) -> u64 {
    state.current_browse_token()
}

/// Remove items the user picked in the explorer.
///
/// Unlike the scan-driven cleanup this is path-addressed, so it carries two
/// extra checks: the path must have appeared in the listing the backend just
/// produced, and the guard is consulted with `Confirmed` authority — which
/// permits personal data and still refuses everything else.
#[tauri::command]
pub async fn delete_browsed(
    state: State<'_, AppState>,
    paths: Vec<PathBuf>,
    mode: Option<CleanupMode>,
    dry_run: Option<bool>,
) -> CommandResult<CleanupRecord> {
    let unlisted = paths.iter().find(|path| !state.was_listed(path));
    if let Some(path) = unlisted {
        return Err(CommandError {
            kind: "refused",
            message: format!(
                "{} was not in the folder you are looking at — refresh and try again",
                path.display()
            ),
        });
    }

    let items: Vec<CleanupItem> = paths
        .iter()
        .map(|path| CleanupItem {
            id: path.to_string_lossy().into_owned(),
            path: path.clone(),
            // The explorer has no scan result to inherit a size from, so the
            // size is measured now. It is only used for the "freed" total.
            size: spacekeeper_core::fsutil::measure(path, &state.control())
                .map(|stats| stats.size)
                .unwrap_or_default(),
            risk: RiskLevel::Review,
        })
        .collect();

    let request = CleanupRequest {
        items,
        mode: mode.unwrap_or_default(),
        dry_run: dry_run.unwrap_or(false),
        authority: Authority::Confirmed,
    };

    let engine = CleanupEngine::new(state.guard());
    let record = tokio::task::block_in_place(|| engine.execute(&request));

    if !record.dry_run {
        for path in &paths {
            state.invalidate_size(path);
        }
        state.db().record_cleanup(&record)?;
    }
    Ok(record)
}

// -------------------------------------------------------------- information

/// Mounted volumes.
#[tauri::command]
pub fn list_disks() -> Vec<disk::DiskInfo> {
    disk::disks()
}

/// Plain-language answer to "why is my disk full?".
#[tauri::command]
pub fn explain_disk(state: State<'_, AppState>) -> CommandResult<Option<Explanation>> {
    let Some(report) = state.last_report() else {
        return Ok(None);
    };
    let primary = disk::primary();
    Ok(Some(explain(&report, primary.as_ref())))
}

/// Recent scans.
#[tauri::command]
pub fn scan_history(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> CommandResult<Vec<ScanHistoryEntry>> {
    Ok(state.db().recent_scans(limit.unwrap_or(20))?)
}

/// Recent cleanups.
#[tauri::command]
pub fn cleanup_history(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> CommandResult<Vec<CleanupRecord>> {
    Ok(state.db().cleanup_history(limit.unwrap_or(20))?)
}

/// Space reclaimed since installation.
#[tauri::command]
pub fn total_freed(state: State<'_, AppState>) -> CommandResult<Bytes> {
    Ok(state.db().total_freed()?)
}

/// Disk usage over time for the primary volume.
#[tauri::command]
pub fn disk_trend(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> CommandResult<Vec<DiskSnapshot>> {
    let Some(primary) = disk::primary() else {
        return Ok(Vec::new());
    };
    Ok(state
        .db()
        .disk_trend(&primary.mount_point, limit.unwrap_or(60))?)
}

/// Lifetime statistics per scanner.
#[tauri::command]
pub fn scanner_stats(state: State<'_, AppState>) -> CommandResult<Vec<ScannerStats>> {
    Ok(state.db().scanner_stats()?)
}

// ----------------------------------------------------------------- settings

/// Current settings.
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> ScanOptions {
    state.options()
}

/// Replace the settings.
#[tauri::command]
pub fn save_settings(
    state: State<'_, AppState>,
    options: ScanOptions,
) -> CommandResult<ScanOptions> {
    state.set_options(options)?;
    Ok(state.options())
}

/// Folders SpaceKeeper has been told to leave alone.
#[tauri::command]
pub fn ignored_paths(state: State<'_, AppState>) -> CommandResult<Vec<PathBuf>> {
    Ok(state.db().ignored_paths()?)
}

/// Add a folder to the ignore list.
#[tauri::command]
pub fn add_ignored_path(state: State<'_, AppState>, path: PathBuf) -> CommandResult<Vec<PathBuf>> {
    state.db().add_ignored(&path)?;
    state.reload_ignored()?;
    Ok(state.db().ignored_paths()?)
}

/// Remove a folder from the ignore list.
#[tauri::command]
pub fn remove_ignored_path(
    state: State<'_, AppState>,
    path: PathBuf,
) -> CommandResult<Vec<PathBuf>> {
    state.db().remove_ignored(&path)?;
    state.reload_ignored()?;
    Ok(state.db().ignored_paths()?)
}

/// Everything SpaceKeeper protects, with the reason for each.
///
/// Generated from the same guard that enforces it, so the settings screen
/// cannot drift from the actual behaviour.
#[tauri::command]
pub fn protected_paths(state: State<'_, AppState>) -> Vec<ProtectedEntry> {
    state.guard().protected_entries()
}

/// Show a path in the system file manager.
///
/// Only reveals — it never opens the file itself, so a malicious path in a
/// scan result cannot be turned into code execution.
#[tauri::command]
pub fn reveal_path(path: PathBuf) -> CommandResult<()> {
    if !path.is_absolute() || !path.exists() {
        return Err(CommandError {
            kind: "not_found",
            message: "that path no longer exists".into(),
        });
    }

    let mut command = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(&path);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(&path));
        c
    };

    command.spawn().map(|_| ()).map_err(|err| CommandError {
        kind: "internal",
        message: err.to_string(),
    })
}
