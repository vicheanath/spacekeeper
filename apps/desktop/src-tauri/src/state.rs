//! Application state shared by every command.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use spacekeeper_core::browse::SizeCache;
use spacekeeper_core::db::Database;
use spacekeeper_core::{
    Control, PathGuard, ScanContext, ScanEngine, ScanOptions, ScanReport, ScannerRegistry,
};

/// Settings key in the `settings` table.
const SETTINGS_KEY: &str = "scan_options";

/// Everything the commands need, built once at startup.
///
/// Only two things are mutable: the handle for the scan currently running, and
/// the most recent report. Both are small and are guarded individually so a
/// long scan never blocks a settings read.
pub struct AppState {
    engine: ScanEngine,
    db: Database,
    home: PathBuf,
    /// Handle for the in-flight scan. Replaced on every new scan, which is how
    /// a stale "stop" from a previous run can never cancel the current one.
    control: Mutex<Control>,
    last_report: RwLock<Option<ScanReport>>,
    options: RwLock<ScanOptions>,
    /// Memoised directory sizes for the explorer.
    ///
    /// Behind an `Arc` because background measuring tasks outlive the command
    /// that started them.
    size_cache: Arc<SizeCache>,
    /// Paths from the most recent directory listing.
    ///
    /// The explorer is inherently path-based, which would otherwise give the
    /// webview the ability to name any path for deletion. Requiring a path to
    /// have been listed by the backend first keeps the same property the scan
    /// flow has: the frontend can only act on what the backend already showed
    /// it.
    listed_paths: RwLock<HashSet<PathBuf>>,
    /// Cancels the measurement pass for the folder the user just left.
    browse_control: Mutex<Control>,
    /// Generation counter for listings.
    ///
    /// Measurements are streamed, so results for a folder the user has already
    /// navigated away from can still be in flight. Every event carries the
    /// token of the listing it belongs to and the UI drops the rest, which is
    /// cheaper and more reliable than trying to guarantee none are emitted.
    browse_token: AtomicU64,
}

impl AppState {
    /// Build the state, registering every built-in scanner.
    ///
    /// This is the composition root: the one place that decides which scanners
    /// exist. Everything downstream works through the registry.
    pub fn new() -> spacekeeper_core::Result<Self> {
        let mut registry = ScannerRegistry::new();
        registry.register_all(spacekeeper_scanners::builtin());

        let db = Database::open(&Database::default_path())?;
        let home = dirs_home();

        // Settings live in the database, but a fresh install has none yet.
        let mut options: ScanOptions = db.get_setting(SETTINGS_KEY)?.unwrap_or_default();
        // The ignore list is authoritative in its own table, so it is always
        // reloaded rather than trusted from the serialised settings blob.
        options.ignored_paths = db.ignored_paths()?;

        Ok(Self {
            engine: ScanEngine::new(registry),
            db,
            home,
            control: Mutex::new(Control::new()),
            last_report: RwLock::new(None),
            options: RwLock::new(options),
            size_cache: Arc::new(SizeCache::new()),
            listed_paths: RwLock::new(HashSet::new()),
            browse_control: Mutex::new(Control::new()),
            browse_token: AtomicU64::new(0),
        })
    }

    /// The current user's home directory, and the only root the explorer opens
    /// at.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Memoised directory sizes.
    pub fn size_cache(&self) -> &SizeCache {
        &self.size_cache
    }

    /// A shareable handle to the size cache, for background measuring.
    pub fn size_cache_handle(&self) -> Arc<SizeCache> {
        Arc::clone(&self.size_cache)
    }

    /// The generation of the listing currently on screen.
    pub fn current_browse_token(&self) -> u64 {
        self.browse_token.load(Ordering::SeqCst)
    }

    /// Record the paths a listing just showed the user.
    pub fn remember_listing(&self, paths: impl IntoIterator<Item = PathBuf>) {
        *self.listed_paths.write().unwrap_or_else(|e| e.into_inner()) = paths.into_iter().collect();
    }

    /// Whether `path` was part of the listing the user is looking at.
    pub fn was_listed(&self, path: &Path) -> bool {
        self.listed_paths.read().unwrap_or_else(|e| e.into_inner()).contains(path)
    }

    /// Forget cached sizes for a path whose contents just changed.
    pub fn invalidate_size(&self, path: &Path) {
        self.size_cache.invalidate(path);
    }

    /// Start a new listing: abandon the previous folder's measuring and take a
    /// fresh token. Returns the token and the handle for the new pass.
    pub fn begin_browse(&self) -> (u64, Control) {
        let control = Control::new();
        {
            let mut current = self
                .browse_control
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            current.cancel();
            *current = control.clone();
        }
        let token = self.browse_token.fetch_add(1, Ordering::SeqCst) + 1;
        (token, control)
    }

    /// The scan engine, holding the scanner registry.
    pub fn engine(&self) -> &ScanEngine {
        &self.engine
    }

    /// The database.
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// A snapshot of the current settings.
    pub fn options(&self) -> ScanOptions {
        self.options
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Replace the settings and persist them.
    pub fn set_options(&self, options: ScanOptions) -> spacekeeper_core::Result<()> {
        self.db.set_setting(SETTINGS_KEY, &options)?;
        *self.options.write().unwrap_or_else(|e| e.into_inner()) = options;
        Ok(())
    }

    /// Refresh the ignore list from the database into the live settings.
    pub fn reload_ignored(&self) -> spacekeeper_core::Result<()> {
        let ignored = self.db.ignored_paths()?;
        self.options
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .ignored_paths = ignored;
        Ok(())
    }

    /// The path guard for the current settings.
    pub fn guard(&self) -> Arc<PathGuard> {
        Arc::new(PathGuard::new(&self.home).with_user_ignored(self.options().ignored_paths))
    }

    /// Build a scan context for a new run, installing a fresh control handle.
    pub fn begin_scan(&self, progress: spacekeeper_core::ProgressReporter) -> ScanContext {
        let control = Control::new();
        *self.control.lock().unwrap_or_else(|e| e.into_inner()) = control.clone();

        ScanContext::new(self.home.clone())
            .with_options(self.options())
            .with_progress(progress)
            .with_control(control)
    }

    /// The handle for the scan currently running.
    pub fn control(&self) -> Control {
        self.control
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Remember the most recent report.
    pub fn set_last_report(&self, report: ScanReport) {
        *self.last_report.write().unwrap_or_else(|e| e.into_inner()) = Some(report);
    }

    /// The most recent report, if a scan has run in this session.
    pub fn last_report(&self) -> Option<ScanReport> {
        self.last_report
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

fn dirs_home() -> PathBuf {
    #[allow(clippy::option_if_let_else)]
    match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        Some(home) => PathBuf::from(home),
        None => PathBuf::from("."),
    }
}
