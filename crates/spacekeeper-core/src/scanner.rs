//! The scanner interface and the registry that holds them.
//!
//! Adding a scanner means writing a type that implements [`Scanner`] and
//! registering it. Nothing in this crate needs to change, which is what keeps
//! the built-in scanners and third-party plugins on exactly the same footing.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::control::Control;
use crate::error::Result;
use crate::progress::ProgressReporter;
use crate::protect::PathGuard;
use crate::types::{Bytes, ScanResult, ScannerMetadata};

/// Tunables a user can change in settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanOptions {
    /// Results smaller than this are not worth showing a user.
    pub min_result_size: Bytes,
    /// A file at or above this size counts as "large".
    pub large_file_threshold: Bytes,
    /// Days without access after which something counts as stale.
    pub stale_after_days: u32,
    /// Smallest file the duplicate finder will hash.
    pub duplicate_min_size: Bytes,
    /// Maximum directory depth for the general-purpose walkers.
    pub max_depth: usize,
    /// Scanners the user explicitly turned off.
    pub disabled_scanners: HashSet<String>,
    /// Folders the user asked SpaceKeeper to ignore.
    pub ignored_paths: Vec<PathBuf>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            min_result_size: Bytes::from_mib(1),
            large_file_threshold: Bytes::from_mib(256),
            stale_after_days: 90,
            duplicate_min_size: Bytes::from_mib(1),
            max_depth: 12,
            disabled_scanners: HashSet::new(),
            ignored_paths: Vec::new(),
        }
    }
}

/// Everything a scanner is given when it runs.
///
/// Cloning is cheap enough to hand one to every scanner task: the heavyweight
/// parts ([`Control`], [`PathGuard`]) are behind an `Arc`.
#[derive(Debug, Clone)]
pub struct ScanContext {
    /// The current user's home directory.
    pub home: PathBuf,
    /// Roots that whole-disk scanners such as "large files" should walk.
    pub roots: Vec<PathBuf>,
    /// User settings.
    pub options: ScanOptions,
    /// Cancel / pause handle.
    pub control: Control,
    /// Where to publish progress.
    pub progress: ProgressReporter,
    /// Shared protected-path policy.
    pub guard: Arc<PathGuard>,
    /// How many scanners are running in this batch. Set by the engine; used
    /// only to populate progress events.
    pub scanner_count: usize,
}

impl ScanContext {
    /// Context for the current user with default options.
    pub fn for_current_user() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self::new(home)
    }

    /// Context rooted at `home`, scanning the home directory only.
    pub fn new(home: PathBuf) -> Self {
        let guard = Arc::new(PathGuard::new(&home));
        Self {
            roots: vec![home.clone()],
            home,
            options: ScanOptions::default(),
            control: Control::new(),
            progress: ProgressReporter::noop(),
            guard,
            scanner_count: 1,
        }
    }

    /// A progress emitter scoped to one scanner.
    pub fn progress_for(&self, scanner: &str) -> crate::progress::ScannerProgress {
        crate::progress::ScannerProgress::new(self.progress.clone(), scanner, self.scanner_count)
    }

    /// Replace the scan options, refreshing the guard with any ignored folders.
    pub fn with_options(mut self, options: ScanOptions) -> Self {
        self.guard = Arc::new(
            PathGuard::new(&self.home).with_user_ignored(options.ignored_paths.iter().cloned()),
        );
        self.options = options;
        self
    }

    /// Replace the roots that whole-disk scanners walk.
    pub fn with_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.roots = roots;
        self
    }

    /// Attach a progress reporter.
    pub fn with_progress(mut self, progress: ProgressReporter) -> Self {
        self.progress = progress;
        self
    }

    /// Attach a cancellation handle.
    pub fn with_control(mut self, control: Control) -> Self {
        self.control = control;
        self
    }

    /// Resolve a `~`-prefixed path against this context's home.
    pub fn resolve(&self, path: &str) -> PathBuf {
        crate::fsutil::expand_home(path, &self.home)
    }

    /// Whether a result of this size is worth reporting.
    pub fn is_worth_reporting(&self, size: Bytes) -> bool {
        size >= self.options.min_result_size
    }
}

/// Discovers cleanup opportunities of one particular kind.
///
/// Implementations must be independent: no scanner may depend on another
/// having run, and any ordering between them is an implementation detail of
/// the engine.
#[async_trait::async_trait]
pub trait Scanner: Send + Sync {
    /// Static description of this scanner.
    fn metadata(&self) -> ScannerMetadata;

    /// Whether this scanner can do useful work on this machine.
    ///
    /// Returning `false` is not an error; the scanner is reported as skipped.
    /// The default checks only the platform list in the metadata.
    fn is_available(&self, _context: &ScanContext) -> bool {
        self.metadata().supports_current_platform()
    }

    /// Find cleanup opportunities.
    ///
    /// The context is borrowed rather than owned (as in the original sketch)
    /// because every scanner in a run shares one context; cloning it per
    /// scanner would duplicate the roots and options for no benefit.
    async fn scan(&self, context: &ScanContext) -> Result<Vec<ScanResult>>;
}

/// The set of scanners available to a run.
///
/// This is the only place that knows which concrete scanners exist, and it is
/// populated by the application at startup. Plugins register here too.
#[derive(Default, Clone)]
pub struct ScannerRegistry {
    scanners: Vec<Arc<dyn Scanner>>,
}

impl ScannerRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a scanner. Later registrations with the same id replace earlier
    /// ones, which is how a plugin can override a built-in scanner.
    pub fn register(&mut self, scanner: Arc<dyn Scanner>) -> &mut Self {
        let id = scanner.metadata().id;
        self.scanners
            .retain(|existing| existing.metadata().id != id);
        self.scanners.push(scanner);
        self
    }

    /// Add several scanners at once.
    pub fn register_all(
        &mut self,
        scanners: impl IntoIterator<Item = Arc<dyn Scanner>>,
    ) -> &mut Self {
        for scanner in scanners {
            self.register(scanner);
        }
        self
    }

    /// Every registered scanner.
    pub fn all(&self) -> &[Arc<dyn Scanner>] {
        &self.scanners
    }

    /// Look up a scanner by id.
    pub fn get(&self, id: &str) -> Option<&Arc<dyn Scanner>> {
        self.scanners.iter().find(|s| s.metadata().id == id)
    }

    /// How many scanners are registered.
    pub fn len(&self) -> usize {
        self.scanners.len()
    }

    /// Whether no scanners are registered.
    pub fn is_empty(&self) -> bool {
        self.scanners.is_empty()
    }

    /// Metadata for every scanner, for the settings screen.
    pub fn metadata(&self) -> Vec<ScannerMetadata> {
        self.scanners.iter().map(|s| s.metadata()).collect()
    }

    /// Scanners that should actually run given the user's settings.
    pub fn selected(
        &self,
        context: &ScanContext,
        only: Option<&HashSet<String>>,
    ) -> Vec<Arc<dyn Scanner>> {
        self.scanners
            .iter()
            .filter(|scanner| {
                let meta = scanner.metadata();
                if context.options.disabled_scanners.contains(&meta.id) {
                    return false;
                }
                if let Some(only) = only {
                    if !only.contains(&meta.id) {
                        return false;
                    }
                }
                meta.supports_current_platform()
            })
            .cloned()
            .collect()
    }
}

impl std::fmt::Debug for ScannerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScannerRegistry")
            .field(
                "scanners",
                &self
                    .scanners
                    .iter()
                    .map(|s| s.metadata().id)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Category, Family, RiskLevel};

    struct Fake {
        id: &'static str,
        platforms: Vec<String>,
        results: usize,
    }

    impl Fake {
        fn new(id: &'static str) -> Self {
            Self {
                id,
                platforms: vec![std::env::consts::OS.to_string()],
                results: 1,
            }
        }

        fn on_platform(id: &'static str, platform: &str) -> Self {
            Self {
                id,
                platforms: vec![platform.to_string()],
                results: 1,
            }
        }
    }

    #[async_trait::async_trait]
    impl Scanner for Fake {
        fn metadata(&self) -> ScannerMetadata {
            ScannerMetadata {
                id: self.id.to_string(),
                name: self.id.to_string(),
                description: String::new(),
                explanation: String::new(),
                family: Family::General,
                default_risk: RiskLevel::Safe,
                platforms: self.platforms.clone(),
                enabled_by_default: true,
                requires_tool: None,
            }
        }

        async fn scan(&self, _context: &ScanContext) -> Result<Vec<ScanResult>> {
            Ok((0..self.results)
                .map(|i| {
                    ScanResult::builder(self.id, format!("/tmp/{}/{i}", self.id))
                        .category(Category::Cache)
                        .build()
                })
                .collect())
        }
    }

    fn context() -> ScanContext {
        ScanContext::new(PathBuf::from("/home/tester"))
    }

    #[test]
    fn registry_starts_empty() {
        let registry = ScannerRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn registering_the_same_id_replaces_it() {
        let mut registry = ScannerRegistry::new();
        registry.register(Arc::new(Fake::new("cache")));
        registry.register(Arc::new(Fake {
            id: "cache",
            platforms: vec![std::env::consts::OS.into()],
            results: 5,
        }));
        assert_eq!(
            registry.len(),
            1,
            "a plugin may override a built-in scanner"
        );
    }

    #[test]
    fn lookup_by_id_works() {
        let mut registry = ScannerRegistry::new();
        registry.register(Arc::new(Fake::new("logs")));
        assert!(registry.get("logs").is_some());
        assert!(registry.get("nope").is_none());
    }

    #[test]
    fn disabled_scanners_are_not_selected() {
        let mut registry = ScannerRegistry::new();
        registry.register_all([
            Arc::new(Fake::new("cache")) as Arc<dyn Scanner>,
            Arc::new(Fake::new("logs")) as Arc<dyn Scanner>,
        ]);
        let mut options = ScanOptions::default();
        options.disabled_scanners.insert("logs".into());
        let context = context().with_options(options);
        let selected = registry.selected(&context, None);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].metadata().id, "cache");
    }

    #[test]
    fn scanners_for_other_platforms_are_not_selected() {
        let mut registry = ScannerRegistry::new();
        registry.register(Arc::new(Fake::on_platform("alien", "plan9")));
        assert!(registry.selected(&context(), None).is_empty());
    }

    #[test]
    fn an_explicit_subset_limits_selection() {
        let mut registry = ScannerRegistry::new();
        registry.register_all([
            Arc::new(Fake::new("cache")) as Arc<dyn Scanner>,
            Arc::new(Fake::new("logs")) as Arc<dyn Scanner>,
        ]);
        let only = HashSet::from(["logs".to_string()]);
        let selected = registry.selected(&context(), Some(&only));
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].metadata().id, "logs");
    }

    #[test]
    fn ignored_paths_reach_the_guard() {
        let options = ScanOptions {
            ignored_paths: vec![PathBuf::from("/home/tester/keep")],
            ..ScanOptions::default()
        };
        let context = context().with_options(options);
        assert!(context
            .guard
            .check(std::path::Path::new("/home/tester/keep/x"))
            .is_err());
    }

    #[test]
    fn resolve_expands_home() {
        assert_eq!(
            context().resolve("~/.cache"),
            PathBuf::from("/home/tester/.cache")
        );
    }
}
