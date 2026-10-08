//! Deleting things, carefully.
//!
//! Design rules, in order of importance:
//!
//! 1. **Nothing is deleted that was not explicitly named.** The engine takes a
//!    list of paths. There is no "clean all" entry point anywhere in the crate.
//! 2. **Trash by default.** Permanent deletion exists but has to be asked for.
//! 3. **The guard runs again here.** Results may be minutes old by the time a
//!    user clicks; paths are re-validated against [`PathGuard`] at the moment
//!    of deletion, not at the moment of discovery.
//! 4. **A failure on one item never aborts the batch**, and every item's fate
//!    is recorded.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::Result;
use crate::protect::PathGuard;
use crate::types::{Bytes, RiskLevel, ScanResult};

/// How to remove files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CleanupMode {
    /// Move to the OS trash. Reversible, and the default.
    #[default]
    Trash,
    /// Delete outright. Not reversible.
    Permanent,
}

impl CleanupMode {
    /// Whether items removed in this mode can be restored.
    pub const fn is_recoverable(self) -> bool {
        matches!(self, Self::Trash)
    }
}

/// One thing the user asked to remove.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupItem {
    /// Id of the originating scan result.
    pub id: String,
    /// Absolute path to remove.
    pub path: PathBuf,
    /// Size recorded at scan time, used for the "freed" total.
    pub size: Bytes,
    /// Risk recorded at scan time.
    pub risk: RiskLevel,
}

impl From<&ScanResult> for CleanupItem {
    fn from(result: &ScanResult) -> Self {
        Self {
            id: result.id.clone(),
            path: result.path.clone(),
            size: result.size,
            risk: result.risk,
        }
    }
}

/// Who is asking, which decides how much the guard will permit.
///
/// This is the only knob in the product that can widen what gets deleted, and
/// it widens it by exactly one tier — personal data — never past the things
/// that are absolutely refused. Cleanup driven by scan results can never set
/// it: the commands that build those requests do not expose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Authority {
    /// Cleanup driven by scan results. Refuses anything protected at all.
    #[default]
    Automated,
    /// The user is looking at this exact item in the file explorer and has
    /// confirmed it. Personal data may be removed; system files, credentials,
    /// applications and volume roots still may not.
    Confirmed,
}

/// A cleanup request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupRequest {
    /// Items to remove.
    pub items: Vec<CleanupItem>,
    /// Trash or permanent.
    #[serde(default)]
    pub mode: CleanupMode,
    /// When true, nothing is touched and the record describes what *would*
    /// happen. This powers the confirmation dialog, so the numbers a user
    /// confirms come from the same code that does the work.
    #[serde(default)]
    pub dry_run: bool,
    /// How much the guard will permit. Defaults to the strictest setting, so
    /// forgetting to set it can only ever fail safe.
    #[serde(default)]
    pub authority: Authority,
}

impl CleanupRequest {
    /// A trash-mode request for the given items.
    pub fn trash(items: Vec<CleanupItem>) -> Self {
        Self {
            items,
            mode: CleanupMode::Trash,
            dry_run: false,
            authority: Authority::Automated,
        }
    }

    /// Turn this request into a preview.
    pub fn as_dry_run(&self) -> Self {
        Self {
            dry_run: true,
            ..self.clone()
        }
    }

    /// Mark this request as an explicit, item-by-item user choice.
    pub fn confirmed(mut self) -> Self {
        self.authority = Authority::Confirmed;
        self
    }
}

/// What happened to one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemStatus {
    /// Moved to the trash.
    Trashed,
    /// Permanently deleted.
    Deleted,
    /// Would have been removed; this was a dry run.
    Planned,
    /// Deliberately not removed. See the message.
    Skipped,
    /// Removal was attempted and failed. See the message.
    Failed,
}

impl ItemStatus {
    /// Whether the item actually left the disk.
    pub const fn freed_space(self) -> bool {
        matches!(self, Self::Trashed | Self::Deleted)
    }
}

/// Per-item outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemOutcome {
    /// Id of the originating scan result.
    pub id: String,
    /// Path that was operated on.
    pub path: PathBuf,
    /// Size credited to this item.
    pub size: Bytes,
    /// Result of the operation.
    pub status: ItemStatus,
    /// Why it was skipped or how it failed.
    pub message: Option<String>,
}

/// The audit record of one cleanup, stored in the database and shown in the
/// history screen.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupRecord {
    /// Unique id for this cleanup.
    pub id: String,
    /// When it ran.
    #[serde(with = "time::serde::rfc3339")]
    pub performed_at: OffsetDateTime,
    /// Mode used.
    pub mode: CleanupMode,
    /// Whether this was a preview.
    pub dry_run: bool,
    /// Space actually reclaimed (or that would be, in a dry run).
    pub freed: Bytes,
    /// Number of items removed.
    pub removed: usize,
    /// Number of items deliberately not removed.
    pub skipped: usize,
    /// Number of items whose removal failed.
    pub failed: usize,
    /// Whether the removal can be undone.
    pub undoable: bool,
    /// Per-item detail.
    pub items: Vec<ItemOutcome>,
}

impl CleanupRecord {
    /// Human summary for the history list.
    pub fn headline(&self) -> String {
        let verb = if self.dry_run {
            "Would free"
        } else if self.mode.is_recoverable() {
            "Moved to Trash, freeing"
        } else {
            "Deleted, freeing"
        };
        format!("{verb} {} across {} items", self.freed, self.removed)
    }
}

/// Performs cleanups.
#[derive(Debug, Clone)]
pub struct CleanupEngine {
    guard: Arc<PathGuard>,
}

impl CleanupEngine {
    /// Build an engine that enforces `guard`.
    pub fn new(guard: Arc<PathGuard>) -> Self {
        Self { guard }
    }

    /// Execute a request, returning a full audit record.
    ///
    /// This is intentionally synchronous and blocking: it is called from a
    /// `spawn_blocking` task by the desktop app. Deletion is a filesystem
    /// operation with no useful async story, and keeping it synchronous makes
    /// the ordering of the safety checks obvious.
    pub fn execute(&self, request: &CleanupRequest) -> CleanupRecord {
        let mut items = Vec::with_capacity(request.items.len());
        let mut freed = Bytes::ZERO;
        let (mut removed, mut skipped, mut failed) = (0, 0, 0);

        for item in &request.items {
            let outcome = self.execute_one(item, request.mode, request.dry_run, request.authority);
            match outcome.status {
                ItemStatus::Trashed | ItemStatus::Deleted => {
                    removed += 1;
                    freed = freed.saturating_add(outcome.size);
                }
                ItemStatus::Planned => {
                    removed += 1;
                    freed = freed.saturating_add(outcome.size);
                }
                ItemStatus::Skipped => skipped += 1,
                ItemStatus::Failed => failed += 1,
            }
            items.push(outcome);
        }

        CleanupRecord {
            id: uuid::Uuid::new_v4().to_string(),
            performed_at: OffsetDateTime::now_utc(),
            mode: request.mode,
            dry_run: request.dry_run,
            freed,
            removed,
            skipped,
            failed,
            undoable: !request.dry_run && request.mode.is_recoverable() && undo_supported(),
            items,
        }
    }

    fn execute_one(
        &self,
        item: &CleanupItem,
        mode: CleanupMode,
        dry_run: bool,
        authority: Authority,
    ) -> ItemOutcome {
        let outcome = |status, message: Option<String>| ItemOutcome {
            id: item.id.clone(),
            path: item.path.clone(),
            size: item.size,
            status,
            message,
        };

        // 1. Policy. Re-checked here because the scan may be minutes old, so
        //    the report a user is clicking on is not evidence of anything.
        //    `Confirmed` relaxes this by exactly one tier — personal data —
        //    and by nothing else.
        let guarded = match authority {
            Authority::Automated => self.guard.check(&item.path),
            Authority::Confirmed => self.guard.check_confirmed(&item.path),
        };
        if let Err(err) = guarded {
            let protection = self.guard.classify(&item.path);
            return outcome(
                ItemStatus::Skipped,
                Some(protection.reason().map_or_else(|| err.to_string(), str::to_string)),
            );
        }

        // 2. Dangerous items are never removed, whatever the caller claims.
        if item.risk == RiskLevel::Dangerous {
            return outcome(
                ItemStatus::Skipped,
                Some("item is classified dangerous and cannot be removed by SpaceKeeper".into()),
            );
        }

        // 3. The path must still be there, and must not be a symlink: removing
        //    a link is never what the user meant, and following one could take
        //    us somewhere the guard already refused.
        let meta = match std::fs::symlink_metadata(&item.path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return outcome(ItemStatus::Skipped, Some("already gone".into()));
            }
            Err(err) => return outcome(ItemStatus::Failed, Some(err.to_string())),
        };
        if meta.file_type().is_symlink() {
            return outcome(
                ItemStatus::Skipped,
                Some("refusing to remove a symbolic link".into()),
            );
        }

        if dry_run {
            return outcome(ItemStatus::Planned, None);
        }

        match remove(&item.path, mode, meta.is_dir()) {
            Ok(()) => match mode {
                CleanupMode::Trash => outcome(ItemStatus::Trashed, None),
                CleanupMode::Permanent => outcome(ItemStatus::Deleted, None),
            },
            Err(err) => outcome(ItemStatus::Failed, Some(err.to_string())),
        }
    }
}

fn remove(path: &Path, mode: CleanupMode, is_dir: bool) -> Result<()> {
    match mode {
        CleanupMode::Trash => {
            trash::delete(path)?;
            Ok(())
        }
        CleanupMode::Permanent if is_dir => {
            std::fs::remove_dir_all(path).map_err(|e| crate::error::Error::io(path, e))
        }
        CleanupMode::Permanent => {
            std::fs::remove_file(path).map_err(|e| crate::error::Error::io(path, e))
        }
    }
}

/// Whether this platform can restore items from the trash programmatically.
///
/// Linux (freedesktop) and Windows expose an API for it. macOS does not: the
/// only supported route is Finder's "Put Back", so we tell the user that
/// rather than pretending an undo button will work.
pub const fn undo_supported() -> bool {
    cfg!(any(target_os = "windows", target_os = "linux"))
}

/// Restore previously trashed items.
///
/// Returns the paths that were restored. On platforms without trash-restore
/// support this returns an error explaining what to do instead.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub fn undo(record: &CleanupRecord) -> Result<Vec<PathBuf>> {
    use std::collections::HashSet;

    if !record.undoable {
        return Err(crate::error::Error::Other(anyhow::anyhow!(
            "this cleanup cannot be undone"
        )));
    }

    let wanted: HashSet<&Path> = record
        .items
        .iter()
        .filter(|item| item.status == ItemStatus::Trashed)
        .map(|item| item.path.as_path())
        .collect();

    let candidates: Vec<_> = trash::os_limited::list()?
        .into_iter()
        .filter(|entry| wanted.contains(entry.original_path().as_path()))
        .collect();

    let restored: Vec<PathBuf> = candidates.iter().map(|c| c.original_path()).collect();
    trash::os_limited::restore_all(candidates)?;
    Ok(restored)
}

/// Restore previously trashed items. Unsupported on this platform.
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn undo(_record: &CleanupRecord) -> Result<Vec<PathBuf>> {
    Err(crate::error::Error::Other(anyhow::anyhow!(
        "this platform cannot restore from the Trash automatically — open the Trash and choose \"Put Back\""
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn engine(home: &Path) -> CleanupEngine {
        CleanupEngine::new(Arc::new(PathGuard::new(home)))
    }

    fn item(path: &Path, size: u64, risk: RiskLevel) -> CleanupItem {
        CleanupItem {
            id: "id".into(),
            path: path.to_path_buf(),
            size: Bytes(size),
            risk,
        }
    }

    fn permanent(items: Vec<CleanupItem>) -> CleanupRequest {
        CleanupRequest {
            items,
            mode: CleanupMode::Permanent,
            dry_run: false,
            authority: Authority::Automated,
        }
    }

    #[test]
    fn permanently_removes_a_directory_tree() {
        let home = tempfile::tempdir().expect("tempdir");
        let target = home.path().join("cache/app");
        fs::create_dir_all(&target).expect("create");
        fs::write(target.join("blob"), b"data").expect("write");

        let record =
            engine(home.path()).execute(&permanent(vec![item(&target, 4, RiskLevel::Safe)]));

        assert!(!target.exists());
        assert_eq!(record.removed, 1);
        assert_eq!(record.freed, Bytes(4));
        assert_eq!(record.items[0].status, ItemStatus::Deleted);
        assert!(!record.undoable, "permanent deletion is never undoable");
    }

    #[test]
    fn removes_a_single_file() {
        let home = tempfile::tempdir().expect("tempdir");
        let target = home.path().join("cache/blob.bin");
        fs::create_dir_all(target.parent().expect("parent")).expect("create");
        fs::write(&target, b"data").expect("write");

        let record =
            engine(home.path()).execute(&permanent(vec![item(&target, 4, RiskLevel::Safe)]));
        assert!(!target.exists());
        assert_eq!(record.items[0].status, ItemStatus::Deleted);
    }

    #[test]
    fn dry_run_touches_nothing_but_reports_the_total() {
        let home = tempfile::tempdir().expect("tempdir");
        let target = home.path().join("cache/app");
        fs::create_dir_all(&target).expect("create");

        let request = permanent(vec![item(&target, 500, RiskLevel::Safe)]).as_dry_run();
        let record = engine(home.path()).execute(&request);

        assert!(target.exists(), "a dry run must not delete anything");
        assert_eq!(record.freed, Bytes(500));
        assert_eq!(record.items[0].status, ItemStatus::Planned);
        assert!(!record.undoable);
    }

    #[test]
    fn protected_paths_are_skipped_even_when_requested() {
        let home = tempfile::tempdir().expect("tempdir");
        let target = home.path().join(".ssh");
        fs::create_dir_all(&target).expect("create");

        let record =
            engine(home.path()).execute(&permanent(vec![item(&target, 10, RiskLevel::Safe)]));

        assert!(
            target.exists(),
            "credentials must survive an explicit request"
        );
        assert_eq!(record.items[0].status, ItemStatus::Skipped);
        assert_eq!(record.skipped, 1);
        assert_eq!(record.freed, Bytes::ZERO);
    }

    #[test]
    fn dangerous_items_are_skipped_even_when_requested() {
        let home = tempfile::tempdir().expect("tempdir");
        let target = home.path().join("app");
        fs::create_dir_all(&target).expect("create");

        let record =
            engine(home.path()).execute(&permanent(vec![item(&target, 10, RiskLevel::Dangerous)]));

        assert!(target.exists());
        assert_eq!(record.items[0].status, ItemStatus::Skipped);
    }

    #[test]
    fn missing_paths_are_skipped_not_failed() {
        let home = tempfile::tempdir().expect("tempdir");
        let target = home.path().join("gone");

        let record =
            engine(home.path()).execute(&permanent(vec![item(&target, 10, RiskLevel::Safe)]));
        assert_eq!(record.items[0].status, ItemStatus::Skipped);
        assert_eq!(record.items[0].message.as_deref(), Some("already gone"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_refused() {
        let home = tempfile::tempdir().expect("tempdir");
        let real = home.path().join("real");
        fs::create_dir_all(&real).expect("create");
        let link = home.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let record =
            engine(home.path()).execute(&permanent(vec![item(&link, 10, RiskLevel::Safe)]));
        assert_eq!(record.items[0].status, ItemStatus::Skipped);
        assert!(link.exists());
        assert!(real.exists());
    }

    #[test]
    fn one_bad_item_does_not_stop_the_batch() {
        let home = tempfile::tempdir().expect("tempdir");
        let good = home.path().join("cache/good");
        fs::create_dir_all(&good).expect("create");
        let protected = home.path().join("Documents");
        fs::create_dir_all(&protected).expect("create");

        let record = engine(home.path()).execute(&permanent(vec![
            item(&protected, 100, RiskLevel::Safe),
            item(&good, 50, RiskLevel::Safe),
        ]));

        assert!(!good.exists());
        assert!(protected.exists());
        assert_eq!(record.removed, 1);
        assert_eq!(record.skipped, 1);
        assert_eq!(record.freed, Bytes(50));
    }

    #[test]
    fn an_empty_request_is_a_no_op() {
        let home = tempfile::tempdir().expect("tempdir");
        let record = engine(home.path()).execute(&permanent(Vec::new()));
        assert_eq!(record.removed, 0);
        assert_eq!(record.freed, Bytes::ZERO);
    }

    #[test]
    fn headline_reads_naturally() {
        let home = tempfile::tempdir().expect("tempdir");
        let record = engine(home.path()).execute(&permanent(Vec::new()).as_dry_run());
        assert!(record.headline().starts_with("Would free"));
    }

    #[test]
    fn automated_cleanup_refuses_personal_data() {
        let home = tempfile::tempdir().expect("tempdir");
        let target = home.path().join("Documents/holiday.mov");
        fs::create_dir_all(target.parent().expect("parent")).expect("create");
        fs::write(&target, b"data").expect("write");

        let record = engine(home.path()).execute(&permanent(vec![item(&target, 4, RiskLevel::Safe)]));

        assert!(target.exists(), "a scan result must never remove a personal file");
        assert_eq!(record.items[0].status, ItemStatus::Skipped);
        assert_eq!(
            record.items[0].message.as_deref(),
            Some("your documents"),
            "the skip reason must be readable, not a path dump"
        );
    }

    #[test]
    fn a_confirmed_request_may_remove_personal_data() {
        let home = tempfile::tempdir().expect("tempdir");
        let target = home.path().join("Documents/holiday.mov");
        fs::create_dir_all(target.parent().expect("parent")).expect("create");
        fs::write(&target, b"data").expect("write");

        let request = permanent(vec![item(&target, 4, RiskLevel::Safe)]).confirmed();
        let record = engine(home.path()).execute(&request);

        assert!(!target.exists(), "the explorer may remove it once confirmed");
        assert_eq!(record.items[0].status, ItemStatus::Deleted);
    }

    #[test]
    fn a_confirmed_request_still_refuses_credentials_and_system_files() {
        let home = tempfile::tempdir().expect("tempdir");
        let key = home.path().join(".ssh/id_ed25519");
        fs::create_dir_all(key.parent().expect("parent")).expect("create");
        fs::write(&key, b"secret").expect("write");

        let request = permanent(vec![item(&key, 6, RiskLevel::Safe)]).confirmed();
        let record = engine(home.path()).execute(&request);

        assert!(key.exists(), "no amount of confirming may remove an SSH key");
        assert_eq!(record.items[0].status, ItemStatus::Skipped);
    }

    #[test]
    fn requests_default_to_the_strictest_authority() {
        let request = CleanupRequest::trash(Vec::new());
        assert_eq!(request.authority, Authority::Automated);
        // Deserialising a payload with no `authority` field must also be strict.
        let decoded: CleanupRequest =
            serde_json::from_str(r#"{"items":[],"mode":"trash"}"#).expect("decode");
        assert_eq!(decoded.authority, Authority::Automated);
    }

    #[test]
    fn cleanup_items_can_be_built_from_scan_results() {
        let result = ScanResult::builder("cache", "/home/tester/.cache/x")
            .size(Bytes(10))
            .risk(RiskLevel::Safe)
            .build();
        let item = CleanupItem::from(&result);
        assert_eq!(item.path, result.path);
        assert_eq!(item.size, Bytes(10));
    }
}
