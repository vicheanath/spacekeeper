//! The data model every other module speaks in.
//!
//! These types cross the Tauri boundary, so they all serialise to camelCase
//! JSON and are mirrored one-to-one in `apps/desktop/src/lib/types.ts`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A size in bytes.
///
/// A newtype rather than a bare `u64` so that a byte count can never be
/// accidentally swapped with a file count, a score, or a day count — all of
/// which are also integers floating around the scan pipeline.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash,
)]
#[serde(transparent)]
pub struct Bytes(pub u64);

impl Bytes {
    /// Zero bytes.
    pub const ZERO: Self = Self(0);

    /// Build from a raw byte count.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Build from a whole number of mebibytes.
    pub const fn from_mib(mib: u64) -> Self {
        Self(mib * 1024 * 1024)
    }

    /// Build from a whole number of gibibytes.
    pub const fn from_gib(gib: u64) -> Self {
        Self(gib * 1024 * 1024 * 1024)
    }

    /// The raw byte count.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Saturating addition, because a wrapping disk size would be nonsense.
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    /// Human readable rendering, e.g. `1.4 GB`.
    ///
    /// Uses SI units (powers of 1000) to match what macOS, Windows Explorer and
    /// most Linux desktops show users, so our numbers agree with theirs.
    pub fn human(self) -> String {
        const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
        let mut value = self.0 as f64;
        let mut unit = 0;
        while value >= 1000.0 && unit < UNITS.len() - 1 {
            value /= 1000.0;
            unit += 1;
        }
        if unit == 0 {
            format!("{} {}", self.0, UNITS[0])
        } else if value < 10.0 {
            format!("{value:.1} {}", UNITS[unit])
        } else {
            format!("{value:.0} {}", UNITS[unit])
        }
    }
}

impl std::fmt::Display for Bytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.human())
    }
}

impl std::iter::Sum for Bytes {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Bytes::saturating_add)
    }
}

/// How risky it is to delete an item.
///
/// Ordered on purpose: policy code combines a scanner's proposal with the
/// protected-path rules using [`Ord::max`], so risk can only ever be raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")]
pub enum RiskLevel {
    /// Regenerated automatically by the owning application. Caches, logs, trash.
    Safe,
    /// Possibly still wanted by the user. Downloads, duplicates, big videos.
    Review,
    /// System files, applications, configuration. Never one-click deletable.
    Dangerous,
}

impl RiskLevel {
    /// Weight used by the recommendation score.
    pub const fn weight(self) -> f64 {
        match self {
            Self::Safe => 1.0,
            Self::Review => 0.55,
            Self::Dangerous => 0.1,
        }
    }

    /// Whether a single click may delete items at this level.
    pub const fn allows_one_click(self) -> bool {
        matches!(self, Self::Safe)
    }

    /// Stable identifier used as the stored database value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Review => "review",
            Self::Dangerous => "dangerous",
        }
    }

    /// Parse a stored value, falling back to the most cautious level.
    ///
    /// An unrecognised value means the database was written by a newer or
    /// corrupted version; treating it as `Dangerous` keeps the failure safe.
    pub fn parse(value: &str) -> Self {
        match value {
            "safe" => Self::Safe,
            "review" => Self::Review,
            _ => Self::Dangerous,
        }
    }
}

/// What kind of thing a result is, used for grouping and for the UI icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    /// Application caches.
    Cache,
    /// Log files.
    Logs,
    /// Temporary files.
    Temp,
    /// The OS trash / recycle bin.
    Trash,
    /// The user's Downloads folder.
    Downloads,
    /// The user's Desktop folder.
    Desktop,
    /// Byte-identical copies of another file.
    Duplicate,
    /// Unusually large individual files.
    LargeFile,
    /// Directories containing nothing.
    EmptyFolder,
    /// Package manager caches and stores.
    PackageCache,
    /// Compiler and build output.
    BuildArtifact,
    /// Container images, volumes and layers.
    Container,
    /// Browser profile caches.
    BrowserCache,
    /// Downloaded machine learning model weights.
    AiModel,
    /// Simulators, emulators and device images.
    DeviceImage,
    /// Anything that does not fit above.
    Other,
}

impl Category {
    /// Stable lowercase identifier, also used as the SQLite column value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cache => "cache",
            Self::Logs => "logs",
            Self::Temp => "temp",
            Self::Trash => "trash",
            Self::Downloads => "downloads",
            Self::Desktop => "desktop",
            Self::Duplicate => "duplicate",
            Self::LargeFile => "largeFile",
            Self::EmptyFolder => "emptyFolder",
            Self::PackageCache => "packageCache",
            Self::BuildArtifact => "buildArtifact",
            Self::Container => "container",
            Self::BrowserCache => "browserCache",
            Self::AiModel => "aiModel",
            Self::DeviceImage => "deviceImage",
            Self::Other => "other",
        }
    }

    /// Short label for the dashboard.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cache => "Caches",
            Self::Logs => "Logs",
            Self::Temp => "Temporary files",
            Self::Trash => "Trash",
            Self::Downloads => "Downloads",
            Self::Desktop => "Desktop",
            Self::Duplicate => "Duplicates",
            Self::LargeFile => "Large files",
            Self::EmptyFolder => "Empty folders",
            Self::PackageCache => "Package caches",
            Self::BuildArtifact => "Build artifacts",
            Self::Container => "Containers",
            Self::BrowserCache => "Browser caches",
            Self::AiModel => "AI models",
            Self::DeviceImage => "Simulators & emulators",
            Self::Other => "Other",
        }
    }

    /// Parse a stored value, falling back to [`Category::Other`].
    pub fn parse(value: &str) -> Self {
        [
            Self::Cache,
            Self::Logs,
            Self::Temp,
            Self::Trash,
            Self::Downloads,
            Self::Desktop,
            Self::Duplicate,
            Self::LargeFile,
            Self::EmptyFolder,
            Self::PackageCache,
            Self::BuildArtifact,
            Self::Container,
            Self::BrowserCache,
            Self::AiModel,
            Self::DeviceImage,
        ]
        .into_iter()
        .find(|c| c.as_str() == value)
        .unwrap_or(Self::Other)
    }
}

/// Which section of the UI a scanner belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Family {
    /// Everyday cleanup everyone benefits from.
    General,
    /// Toolchain and build caches.
    Developer,
    /// Web browser data.
    Browser,
    /// Local AI model weights.
    Ai,
}

impl Family {
    /// Label for the sidebar.
    pub const fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Developer => "Developer",
            Self::Browser => "Browsers",
            Self::Ai => "AI",
        }
    }
}

/// Whether a result points at a single file or a whole directory tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    /// A single file.
    File,
    /// A directory, whose size is the recursive total of its contents.
    Directory,
}

/// Static description of a scanner, shown in settings and used for filtering.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannerMetadata {
    /// Stable identifier, e.g. `node_modules`. Persisted in the database, so
    /// changing it is a breaking change.
    pub id: String,
    /// Display name, e.g. "Node modules".
    pub name: String,
    /// One sentence explaining what the scanner looks for.
    pub description: String,
    /// Plain-language explanation of what the found data actually is. This is
    /// the text that turns "DerivedData" into something a normal person can
    /// make a decision about.
    pub explanation: String,
    /// UI grouping.
    pub family: Family,
    /// Risk the scanner proposes for its results. Policy may raise it.
    pub default_risk: RiskLevel,
    /// Operating systems the scanner can run on (`macos`, `linux`, `windows`).
    pub platforms: Vec<String>,
    /// Whether the scanner runs unless the user turns it off.
    pub enabled_by_default: bool,
    /// External CLI the scanner shells out to, if any.
    pub requires_tool: Option<String>,
}

impl ScannerMetadata {
    /// Whether this scanner is meaningful on the current platform.
    pub fn supports_current_platform(&self) -> bool {
        self.platforms.iter().any(|p| p == std::env::consts::OS)
    }
}

/// One cleanup opportunity found by a scanner.
///
/// This is the single unit the UI renders, the recommendation engine ranks and
/// the cleanup engine acts on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    /// Stable identity derived from scanner id + path, so the same item keeps
    /// the same id across scans and selections survive a rescan.
    pub id: String,
    /// Id of the scanner that produced this result.
    pub scanner: String,
    /// Short headline, e.g. "node_modules in ~/code/api".
    pub title: String,
    /// Friendly explanation of what this is and what happens if it is removed.
    pub description: String,
    /// Absolute path on disk.
    pub path: PathBuf,
    /// Total size on disk.
    pub size: Bytes,
    /// File or directory.
    pub kind: EntryKind,
    /// Number of files covered (1 for a plain file).
    pub file_count: u64,
    /// Grouping category.
    pub category: Category,
    /// Risk of deleting it.
    pub risk: RiskLevel,
    /// Whether SpaceKeeper is willing to delete it at all.
    pub deletable: bool,
    /// Whether deletion can be undone (true when it goes to the OS trash).
    pub recoverable: bool,
    /// Last access time, when the filesystem records one.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_accessed: Option<OffsetDateTime>,
    /// Last modification time.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_modified: Option<OffsetDateTime>,
    /// Ranking score in `0..=100`, filled in by the recommendation engine.
    pub score: f64,
    /// How sure the scanner is that this is really disposable, in `0.0..=1.0`.
    pub confidence: f64,
    /// Extra scanner-specific facts, e.g. the duplicate group hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

impl ScanResult {
    /// Start building a result. See [`ScanResultBuilder`].
    pub fn builder(scanner: impl Into<String>, path: impl Into<PathBuf>) -> ScanResultBuilder {
        ScanResultBuilder::new(scanner, path)
    }

    /// Days since the item was last touched, preferring access time and
    /// falling back to modification time.
    pub fn unused_days(&self, now: OffsetDateTime) -> f64 {
        let reference = self.last_accessed.or(self.last_modified);
        match reference {
            Some(ts) => ((now - ts).whole_seconds().max(0) as f64) / 86_400.0,
            // No timestamp at all: assume recent so we never over-recommend.
            None => 0.0,
        }
    }
}

/// Builder for [`ScanResult`].
///
/// Scanners only have to supply what they actually know; everything else gets a
/// conservative default (not deletable until said otherwise, `Review` risk).
#[derive(Debug, Clone)]
pub struct ScanResultBuilder {
    inner: ScanResult,
}

impl ScanResultBuilder {
    /// Create a builder for `path`, attributed to `scanner`.
    pub fn new(scanner: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        let scanner = scanner.into();
        let path = path.into();
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        Self {
            inner: ScanResult {
                id: result_id(&scanner, &path),
                scanner,
                title,
                description: String::new(),
                path,
                size: Bytes::ZERO,
                kind: EntryKind::Directory,
                file_count: 0,
                category: Category::Other,
                risk: RiskLevel::Review,
                deletable: false,
                recoverable: true,
                last_accessed: None,
                last_modified: None,
                score: 0.0,
                confidence: 0.5,
                detail: None,
            },
        }
    }

    /// Set the headline.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.inner.title = title.into();
        self
    }

    /// Set the plain-language description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.inner.description = description.into();
        self
    }

    /// Set the size.
    pub fn size(mut self, size: Bytes) -> Self {
        self.inner.size = size;
        self
    }

    /// Set the entry kind.
    pub fn kind(mut self, kind: EntryKind) -> Self {
        self.inner.kind = kind;
        self
    }

    /// Set how many files this result covers.
    pub fn file_count(mut self, count: u64) -> Self {
        self.inner.file_count = count;
        self
    }

    /// Set the category.
    pub fn category(mut self, category: Category) -> Self {
        self.inner.category = category;
        self
    }

    /// Set the proposed risk level.
    pub fn risk(mut self, risk: RiskLevel) -> Self {
        self.inner.risk = risk;
        self
    }

    /// Mark the item as deletable.
    pub fn deletable(mut self, deletable: bool) -> Self {
        self.inner.deletable = deletable;
        self
    }

    /// Set whether deletion can be undone.
    pub fn recoverable(mut self, recoverable: bool) -> Self {
        self.inner.recoverable = recoverable;
        self
    }

    /// Set the access timestamp.
    pub fn last_accessed(mut self, ts: Option<OffsetDateTime>) -> Self {
        self.inner.last_accessed = ts;
        self
    }

    /// Set the modification timestamp.
    pub fn last_modified(mut self, ts: Option<OffsetDateTime>) -> Self {
        self.inner.last_modified = ts;
        self
    }

    /// Set the scanner's confidence, clamped to `0.0..=1.0`.
    pub fn confidence(mut self, confidence: f64) -> Self {
        self.inner.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Attach scanner-specific structured detail.
    pub fn detail(mut self, detail: serde_json::Value) -> Self {
        self.inner.detail = Some(detail);
        self
    }

    /// Finish building.
    pub fn build(self) -> ScanResult {
        self.inner
    }
}

/// Deterministic id for a result, so selections survive rescans.
///
/// BLAKE3 of `scanner\0path` truncated to 16 hex characters: short enough to
/// read in logs, wide enough that collisions are not a practical concern.
pub fn result_id(scanner: &str, path: &Path) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(scanner.as_bytes());
    hasher.update(b"\0");
    hasher.update(path.as_os_str().as_encoded_bytes());
    hasher.finalize().to_hex()[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes_use_si_units() {
        assert_eq!(Bytes(0).human(), "0 B");
        assert_eq!(Bytes(999).human(), "999 B");
        assert_eq!(Bytes(1_000).human(), "1.0 KB");
        assert_eq!(Bytes(1_500).human(), "1.5 KB");
        assert_eq!(Bytes(15_000).human(), "15 KB");
        assert_eq!(Bytes(2_400_000_000).human(), "2.4 GB");
    }

    #[test]
    fn bytes_addition_saturates() {
        assert_eq!(Bytes(u64::MAX).saturating_add(Bytes(10)), Bytes(u64::MAX));
    }

    #[test]
    fn risk_is_ordered_so_policy_can_only_escalate() {
        assert!(RiskLevel::Safe < RiskLevel::Review);
        assert!(RiskLevel::Review < RiskLevel::Dangerous);
        assert_eq!(
            RiskLevel::Safe.max(RiskLevel::Dangerous),
            RiskLevel::Dangerous
        );
    }

    #[test]
    fn only_safe_items_are_one_click() {
        assert!(RiskLevel::Safe.allows_one_click());
        assert!(!RiskLevel::Review.allows_one_click());
        assert!(!RiskLevel::Dangerous.allows_one_click());
    }

    #[test]
    fn ids_are_stable_and_path_specific() {
        let a = result_id("cache", Path::new("/tmp/a"));
        let b = result_id("cache", Path::new("/tmp/a"));
        let c = result_id("cache", Path::new("/tmp/b"));
        let d = result_id("logs", Path::new("/tmp/a"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn builder_defaults_are_conservative() {
        let result = ScanResult::builder("x", "/tmp/foo").build();
        assert!(!result.deletable, "items must opt in to deletion");
        assert_eq!(result.risk, RiskLevel::Review);
        assert_eq!(result.title, "foo");
    }

    #[test]
    fn unused_days_prefers_access_time() {
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(100);
        let result = ScanResult::builder("x", "/tmp/foo")
            .last_accessed(Some(OffsetDateTime::UNIX_EPOCH + time::Duration::days(90)))
            .last_modified(Some(OffsetDateTime::UNIX_EPOCH))
            .build();
        assert!((result.unused_days(now) - 10.0).abs() < 0.001);
    }

    #[test]
    fn unused_days_is_zero_without_timestamps() {
        let result = ScanResult::builder("x", "/tmp/foo").build();
        assert_eq!(result.unused_days(OffsetDateTime::UNIX_EPOCH), 0.0);
    }
}
