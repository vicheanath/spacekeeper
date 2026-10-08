//! Declarative scanners.
//!
//! Most of what SpaceKeeper looks for is "this well-known folder, if it
//! exists": npm's cache, Xcode's DerivedData, Chrome's cache, Ollama's models.
//! Writing thirty near-identical `impl Scanner` blocks for those would be
//! thirty places for a safety bug to hide.
//!
//! Instead a [`PathRule`] is *data*, and [`RuleScanner`] is the single
//! implementation that runs it. Adding support for Unity, Steam or JetBrains is
//! one entry in [`crate::catalog`] — no new code, no new tests of the walking
//! logic, and the measurement, cancellation and safety behaviour is by
//! construction identical to every other scanner.

use std::path::{Path, PathBuf};

use spacekeeper_core::{
    fsutil, Bytes, Category, Family, Result, RiskLevel, ScanContext, ScanResult, Scanner,
    ScannerMetadata,
};

/// A well-known location worth reporting, described declaratively.
#[derive(Debug, Clone, Copy)]
pub struct PathRule {
    /// Stable scanner id. Persisted, so treat changes as breaking.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// What the scanner looks for.
    pub description: &'static str,
    /// Plain-language explanation of what the data is. This is what the user
    /// reads before deciding, so it must say what happens after deletion.
    pub explanation: &'static str,
    /// UI grouping.
    pub family: Family,
    /// Result category.
    pub category: Category,
    /// Proposed risk. The core may raise it; it can never be lowered.
    pub risk: RiskLevel,
    /// How certain we are that this location is really disposable.
    pub confidence: f64,
    /// Locations to look in. A leading `~` is the user's home directory, and a
    /// `*` component matches any single directory name.
    pub paths: &'static [&'static str],
    /// Directory names a `*` expansion must skip.
    ///
    /// This is how a broad sweep cedes ground to a scanner that knows better.
    /// `app_cache` walks every folder in `~/Library/Caches`, but `Google` is
    /// excluded because `chrome_cache` owns it and can say "your bookmarks are
    /// not touched" — which the generic rule cannot.
    ///
    /// Without this the overlap would still be *safe* (the engine collapses
    /// nested results so nothing is double counted) but the user would get the
    /// vaguer explanation of the two.
    pub exclude: &'static [&'static str],
    /// Platforms this rule applies to.
    pub platforms: &'static [&'static str],
    /// External tool that owns this data, shown in the UI.
    pub requires_tool: Option<&'static str>,
}

impl PathRule {
    /// Build the scanner that runs this rule.
    pub const fn scanner(self) -> RuleScanner {
        RuleScanner { rule: self }
    }
}

/// Runs a single [`PathRule`].
#[derive(Debug, Clone)]
pub struct RuleScanner {
    rule: PathRule,
}

impl RuleScanner {
    /// The rule being run.
    pub const fn rule(&self) -> &PathRule {
        &self.rule
    }
}

#[async_trait::async_trait]
impl Scanner for RuleScanner {
    fn metadata(&self) -> ScannerMetadata {
        ScannerMetadata {
            id: self.rule.id.to_string(),
            name: self.rule.name.to_string(),
            description: self.rule.description.to_string(),
            explanation: self.rule.explanation.to_string(),
            family: self.rule.family,
            default_risk: self.rule.risk,
            platforms: self
                .rule
                .platforms
                .iter()
                .map(|p| (*p).to_string())
                .collect(),
            enabled_by_default: true,
            requires_tool: self.rule.requires_tool.map(str::to_string),
        }
    }

    fn is_available(&self, context: &ScanContext) -> bool {
        if !self.metadata().supports_current_platform() {
            return false;
        }
        // Cheap existence check so a machine without Xcode does not show an
        // Xcode scanner that found nothing.
        expand_all_excluding(self.rule.paths, self.rule.exclude, &context.home)
            .iter()
            .any(|p| p.exists())
    }

    async fn scan(&self, context: &ScanContext) -> Result<Vec<ScanResult>> {
        let rule = self.rule;
        let progress = context.progress_for(rule.id);
        let candidates = expand_all_excluding(rule.paths, rule.exclude, &context.home);

        let control = context.control.clone();
        let guard = std::sync::Arc::clone(&context.guard);
        let min_size = context.options.min_result_size;

        // Directory measurement is blocking and CPU/IO heavy; keep it off the
        // async runtime's worker threads.
        let measured = tokio::task::spawn_blocking(move || {
            let existing: Vec<PathBuf> = candidates
                .into_iter()
                .filter(|path| guard.check(path).is_ok() && path.exists())
                .collect();
            fsutil::measure_all(&existing, &control)
        })
        .await
        .map_err(|err| spacekeeper_core::Error::Other(anyhow::anyhow!(err)))?;

        let mut results = Vec::new();
        let mut total = Bytes::ZERO;
        for (path, stats) in measured {
            if stats.size < min_size {
                continue;
            }
            total = total.saturating_add(stats.size);
            progress.scanning(stats.files, total, Some(path.display().to_string()));
            results.push(build_result(&rule, &path, &stats, &context.home));
        }

        Ok(results)
    }
}

fn build_result(rule: &PathRule, path: &Path, stats: &fsutil::DirStats, home: &Path) -> ScanResult {
    // A wildcard rule yields one result per application, so naming the app is
    // far more useful than repeating the rule's own name on every row.
    let title = if rule.paths.iter().any(|pattern| pattern.contains('*')) {
        match path.file_name().and_then(|name| name.to_str()) {
            Some(name) => format!("{} — {}", friendly_name(name), display_path(path, home)),
            None => display_path(path, home),
        }
    } else {
        format!("{} — {}", rule.name, display_path(path, home))
    };

    ScanResult::builder(rule.id, path)
        .title(title)
        .description(rule.explanation.to_string())
        .size(stats.size)
        .file_count(stats.files)
        .kind(spacekeeper_core::EntryKind::Directory)
        .category(rule.category)
        .risk(rule.risk)
        // A cache folder that spans filesystems has something mounted inside
        // it. Removing it would reach onto another disk, so it is reported for
        // information and never offered for deletion.
        .deletable(rule.risk != RiskLevel::Dangerous && !stats.crosses_mount)
        .recoverable(true)
        .last_accessed(stats.last_accessed)
        .last_modified(stats.last_modified)
        .confidence(rule.confidence)
        .build()
}

/// Render a path with the home directory shortened to `~`.
pub fn display_path(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Turn a cache directory name into something a person recognises.
///
/// macOS cache folders are reverse-DNS bundle identifiers, which are
/// meaningless to anyone who has not shipped software. `com.google.Chrome`
/// should read as "Chrome", and `com.spotify.client` as "Spotify" — so the
/// last segment is used unless it is a generic suffix, in which case the one
/// before it carries the actual name.
pub fn friendly_name(raw: &str) -> String {
    const GENERIC: &[&str] = &[
        "client", "app", "desktop", "mac", "macos", "osx", "helper", "shared", "browser",
    ];

    let segments: Vec<&str> = raw.split('.').filter(|s| !s.is_empty()).collect();
    let chosen = if segments.len() < 2 {
        raw
    } else {
        let last = segments[segments.len() - 1];
        if GENERIC.contains(&last.to_ascii_lowercase().as_str()) && segments.len() >= 2 {
            segments[segments.len() - 2]
        } else {
            last
        }
    };

    // Leave existing capitalisation alone (`Chrome`, `DerivedData`); only lift
    // an all-lowercase name so it does not look like a typo.
    let mut chars = chosen.chars();
    match chars.next() {
        Some(first) if chosen.chars().all(|c| !c.is_uppercase()) => {
            first.to_uppercase().collect::<String>() + chars.as_str()
        }
        _ => chosen.to_string(),
    }
}

/// Expand every pattern, skipping any wildcard match named in `exclude`.
pub fn expand_all_excluding(patterns: &[&str], exclude: &[&str], home: &Path) -> Vec<PathBuf> {
    let mut out = expand_all(patterns, home);
    out.retain(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| !exclude.iter().any(|skip| skip.eq_ignore_ascii_case(name)))
            .unwrap_or(true)
    });
    out
}

/// Expand every pattern in `patterns` against `home`.
pub fn expand_all(patterns: &[&str], home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for pattern in patterns {
        out.extend(expand(pattern, home));
    }
    out.sort();
    out.dedup();
    out
}

/// Expand one pattern.
///
/// Supports a leading `~` and `*` as a whole path component. That covers every
/// real case in the catalogue (browser profile folders, versioned SDK
/// directories) without pulling in a glob engine, and — more importantly —
/// without supporting `..`, which would be a way around the path guard.
pub fn expand(pattern: &str, home: &Path) -> Vec<PathBuf> {
    let expanded = fsutil::expand_home(pattern, home);
    if !expanded.to_string_lossy().contains('*') {
        return vec![expanded];
    }

    let mut matches: Vec<PathBuf> = vec![PathBuf::new()];
    for component in expanded.components() {
        let part = component.as_os_str();
        if part == "*" {
            matches = matches
                .into_iter()
                .flat_map(|base| {
                    std::fs::read_dir(&base)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .map(|entry| entry.path())
                        .collect::<Vec<_>>()
                })
                .collect();
        } else {
            for base in &mut matches {
                base.push(part);
            }
        }
        if matches.is_empty() {
            break;
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use spacekeeper_core::{Control, ScanOptions};
    use std::fs;

    const RULE: PathRule = PathRule {
        id: "test_cache",
        name: "Test cache",
        description: "A cache used by tests",
        explanation: "Temporary files that are recreated when needed.",
        family: Family::General,
        category: Category::Cache,
        risk: RiskLevel::Safe,
        confidence: 0.95,
        paths: &["~/.cache/testapp", "~/.cache/other/*"],
        exclude: &[],
        platforms: &["macos", "linux", "windows"],
        requires_tool: None,
    };

    fn context(home: &Path) -> ScanContext {
        ScanContext::new(home.to_path_buf()).with_options(ScanOptions {
            min_result_size: Bytes::ZERO,
            ..ScanOptions::default()
        })
    }

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, vec![b'x'; bytes]).expect("write");
    }

    #[test]
    fn expands_a_leading_tilde() {
        let home = Path::new("/home/tester");
        assert_eq!(
            expand("~/.cache", home),
            vec![PathBuf::from("/home/tester/.cache")]
        );
    }

    #[test]
    fn expands_a_star_component_to_existing_children() {
        let home = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(home.path().join("profiles/alice")).expect("mkdir");
        fs::create_dir_all(home.path().join("profiles/bob")).expect("mkdir");

        let mut found = expand("~/profiles/*", home.path());
        found.sort();
        assert_eq!(found.len(), 2);
        assert!(found[0].ends_with("alice"));
        assert!(found[1].ends_with("bob"));
    }

    #[test]
    fn a_star_that_matches_nothing_yields_nothing() {
        let home = tempfile::tempdir().expect("tempdir");
        assert!(expand("~/nope/*", home.path()).is_empty());
    }

    #[test]
    fn star_expansion_appends_trailing_components() {
        let home = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(home.path().join("p/alice/Cache")).expect("mkdir");
        let found = expand("~/p/*/Cache", home.path());
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("alice/Cache"));
    }

    #[tokio::test]
    async fn reports_existing_directories_with_their_size() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join(".cache/testapp/blob"), 2048);

        let results = RULE
            .scanner()
            .scan(&context(home.path()))
            .await
            .expect("scan");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].size, Bytes(2048));
        assert_eq!(results[0].scanner, "test_cache");
        assert_eq!(results[0].risk, RiskLevel::Safe);
        assert!(results[0].deletable);
        assert!(results[0].description.contains("recreated"));
    }

    #[tokio::test]
    async fn missing_directories_produce_no_results() {
        let home = tempfile::tempdir().expect("tempdir");
        let results = RULE
            .scanner()
            .scan(&context(home.path()))
            .await
            .expect("scan");
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn results_below_the_minimum_size_are_dropped() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join(".cache/testapp/blob"), 10);

        let context = ScanContext::new(home.path().to_path_buf()).with_options(ScanOptions {
            min_result_size: Bytes(1000),
            ..ScanOptions::default()
        });
        let results = RULE.scanner().scan(&context).await.expect("scan");
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn protected_locations_are_never_reported() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join(".ssh/id_rsa"), 4096);

        const SNEAKY: PathRule = PathRule {
            id: "sneaky",
            name: "Sneaky",
            description: "",
            explanation: "",
            family: Family::General,
            category: Category::Cache,
            risk: RiskLevel::Safe,
            confidence: 1.0,
            paths: &["~/.ssh"],
            exclude: &[],
            platforms: &["macos", "linux", "windows"],
            requires_tool: None,
        };

        let results = SNEAKY
            .scanner()
            .scan(&context(home.path()))
            .await
            .expect("scan");
        assert!(
            results.is_empty(),
            "a rule must not be able to target credentials"
        );
    }

    #[tokio::test]
    async fn cancellation_is_honoured() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join(".cache/testapp/blob"), 1024);

        let control = Control::new();
        control.cancel();
        let context = context(home.path()).with_control(control);
        // Measurement returns no usable stats once cancelled.
        let results = RULE.scanner().scan(&context).await.expect("scan");
        assert!(results.is_empty());
    }

    #[test]
    fn availability_follows_the_filesystem() {
        let home = tempfile::tempdir().expect("tempdir");
        let scanner = RULE.scanner();
        assert!(!scanner.is_available(&context(home.path())));

        fs::create_dir_all(home.path().join(".cache/testapp")).expect("mkdir");
        assert!(scanner.is_available(&context(home.path())));
    }

    #[test]
    fn metadata_is_derived_from_the_rule() {
        let meta = RULE.scanner().metadata();
        assert_eq!(meta.id, "test_cache");
        assert_eq!(meta.name, "Test cache");
        assert_eq!(meta.default_risk, RiskLevel::Safe);
        assert_eq!(meta.platforms.len(), 3);
    }

    #[test]
    fn display_path_shortens_home() {
        let home = Path::new("/home/tester");
        assert_eq!(
            display_path(Path::new("/home/tester/.cache"), home),
            "~/.cache"
        );
        assert_eq!(display_path(Path::new("/opt/thing"), home), "/opt/thing");
    }
}
