//! SQLite persistence: scan history, cleanup history, settings, ignore list.
//!
//! Everything stays on the user's machine. The database is a plain file in the
//! platform config directory that can be deleted at any time without breaking
//! the app — nothing here is load-bearing for a scan, it is history and
//! preferences only.
//!
//! Schema changes are applied through the `user_version` pragma; each migration
//! is append-only and idempotent.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::cleanup::{CleanupMode, CleanupRecord, ItemOutcome, ItemStatus};
use crate::disk::DiskInfo;
use crate::error::{Error, Result};
use crate::scan::ScanReport;
use crate::types::Bytes;

/// Current schema version. Bump when adding a migration below.
const SCHEMA_VERSION: i64 = 1;

/// A handle to the SpaceKeeper database.
///
/// `rusqlite::Connection` is `Send` but not `Sync`, so it lives behind a mutex.
/// Contention is irrelevant here: writes happen once per scan or cleanup, and
/// reads are a handful of history queries per screen.
#[derive(Debug)]
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Open (creating if needed) the database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// Open a throwaway in-memory database, used by tests.
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    /// The conventional on-disk location for the database.
    pub fn default_path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("SpaceKeeper")
            .join("spacekeeper.db")
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        // WAL keeps the UI responsive while a large scan is being written;
        // FULL synchronous is unnecessary for a history database.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut guard = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut guard)
    }

    /// Apply any outstanding migrations.
    fn migrate(&self) -> Result<()> {
        self.with_conn(|conn| {
            let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
            if version < 1 {
                conn.execute_batch(MIGRATION_001)?;
            }
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            Ok(())
        })
    }

    /// The schema version currently applied.
    pub fn schema_version(&self) -> Result<i64> {
        self.with_conn(|conn| Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?))
    }

    // ---------------------------------------------------------------- scans

    /// Store a scan and its results.
    pub fn record_scan(&self, report: &ScanReport) -> Result<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT OR REPLACE INTO scans
                 (id, started_at, duration_ms, cancelled, total_items, total_size, safe_size, review_size)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    report.id,
                    to_ts(report.started_at),
                    report.duration_ms as i64,
                    report.cancelled as i64,
                    report.summary.total_items as i64,
                    report.summary.total_size.get() as i64,
                    report.summary.safe_size.get() as i64,
                    report.summary.review_size.get() as i64,
                ],
            )?;

            {
                let mut stmt = tx.prepare(
                    "INSERT OR REPLACE INTO scan_items
                     (scan_id, item_id, scanner, title, path, size, category, risk, deletable, score)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                )?;
                for result in &report.results {
                    stmt.execute(params![
                        report.id,
                        result.id,
                        result.scanner,
                        result.title,
                        result.path.to_string_lossy(),
                        result.size.get() as i64,
                        result.category.as_str(),
                        result.risk.as_str(),
                        result.deletable as i64,
                        result.score,
                    ])?;
                }

                let mut stats = tx.prepare(
                    "INSERT INTO scanner_stats (scanner, runs, total_items, total_size, total_duration_ms, last_run_at)
                     VALUES (?1, 1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(scanner) DO UPDATE SET
                        runs = runs + 1,
                        total_items = total_items + excluded.total_items,
                        total_size = total_size + excluded.total_size,
                        total_duration_ms = total_duration_ms + excluded.total_duration_ms,
                        last_run_at = excluded.last_run_at",
                )?;
                for outcome in &report.scanners {
                    stats.execute(params![
                        outcome.scanner,
                        outcome.items as i64,
                        outcome.size.get() as i64,
                        outcome.duration_ms as i64,
                        to_ts(report.started_at),
                    ])?;
                }
            }

            tx.commit()?;
            Ok(())
        })
    }

    /// Recent scans, newest first.
    pub fn recent_scans(&self, limit: usize) -> Result<Vec<ScanHistoryEntry>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, started_at, duration_ms, total_items, total_size, safe_size
                 FROM scans ORDER BY started_at DESC LIMIT ?1",
            )?;
            let rows = stmt
                .query_map([limit as i64], |row| {
                    Ok(ScanHistoryEntry {
                        id: row.get(0)?,
                        started_at: from_ts(&row.get::<_, String>(1)?),
                        duration_ms: row.get::<_, i64>(2)? as u64,
                        total_items: row.get::<_, i64>(3)? as usize,
                        total_size: Bytes(row.get::<_, i64>(4)? as u64),
                        safe_size: Bytes(row.get::<_, i64>(5)? as u64),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    // ------------------------------------------------------------- cleanups

    /// Store a cleanup record and its per-item outcomes.
    pub fn record_cleanup(&self, record: &CleanupRecord) -> Result<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT OR REPLACE INTO cleanups
                 (id, performed_at, mode, dry_run, freed, removed, skipped, failed, undoable)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    record.id,
                    to_ts(record.performed_at),
                    match record.mode {
                        CleanupMode::Trash => "trash",
                        CleanupMode::Permanent => "permanent",
                    },
                    record.dry_run as i64,
                    record.freed.get() as i64,
                    record.removed as i64,
                    record.skipped as i64,
                    record.failed as i64,
                    record.undoable as i64,
                ],
            )?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO cleanup_items (cleanup_id, item_id, path, size, status, message)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?;
                for item in &record.items {
                    stmt.execute(params![
                        record.id,
                        item.id,
                        item.path.to_string_lossy(),
                        item.size.get() as i64,
                        status_to_str(item.status),
                        item.message,
                    ])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// Cleanup history, newest first. Dry runs are excluded — they are not
    /// something that happened to the user's disk.
    pub fn cleanup_history(&self, limit: usize) -> Result<Vec<CleanupRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, performed_at, mode, dry_run, freed, removed, skipped, failed, undoable
                 FROM cleanups WHERE dry_run = 0 ORDER BY performed_at DESC LIMIT ?1",
            )?;
            let mut records = stmt
                .query_map([limit as i64], |row| {
                    Ok(CleanupRecord {
                        id: row.get(0)?,
                        performed_at: from_ts(&row.get::<_, String>(1)?),
                        mode: if row.get::<_, String>(2)? == "permanent" {
                            CleanupMode::Permanent
                        } else {
                            CleanupMode::Trash
                        },
                        dry_run: row.get::<_, i64>(3)? != 0,
                        freed: Bytes(row.get::<_, i64>(4)? as u64),
                        removed: row.get::<_, i64>(5)? as usize,
                        skipped: row.get::<_, i64>(6)? as usize,
                        failed: row.get::<_, i64>(7)? as usize,
                        undoable: row.get::<_, i64>(8)? != 0,
                        items: Vec::new(),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let mut items = conn.prepare(
                "SELECT item_id, path, size, status, message FROM cleanup_items WHERE cleanup_id = ?1",
            )?;
            for record in &mut records {
                record.items = items
                    .query_map([&record.id], |row| {
                        Ok(ItemOutcome {
                            id: row.get(0)?,
                            path: PathBuf::from(row.get::<_, String>(1)?),
                            size: Bytes(row.get::<_, i64>(2)? as u64),
                            status: status_from_str(&row.get::<_, String>(3)?),
                            message: row.get(4)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
            }
            Ok(records)
        })
    }

    /// Total space reclaimed across all recorded cleanups.
    pub fn total_freed(&self) -> Result<Bytes> {
        self.with_conn(|conn| {
            let total: i64 = conn.query_row(
                "SELECT COALESCE(SUM(freed), 0) FROM cleanups WHERE dry_run = 0",
                [],
                |row| row.get(0),
            )?;
            Ok(Bytes(total.max(0) as u64))
        })
    }

    // -------------------------------------------------------- ignored paths

    /// Add a folder to the ignore list.
    pub fn add_ignored(&self, path: &Path) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO ignored_paths (path, added_at) VALUES (?1, ?2)",
                params![path.to_string_lossy(), to_ts(OffsetDateTime::now_utc())],
            )?;
            Ok(())
        })
    }

    /// Remove a folder from the ignore list.
    pub fn remove_ignored(&self, path: &Path) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM ignored_paths WHERE path = ?1",
                params![path.to_string_lossy()],
            )?;
            Ok(())
        })
    }

    /// The current ignore list.
    pub fn ignored_paths(&self) -> Result<Vec<PathBuf>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT path FROM ignored_paths ORDER BY path")?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows.into_iter().map(PathBuf::from).collect())
        })
    }

    // ------------------------------------------------------------- settings

    /// Read a JSON-encoded setting.
    pub fn get_setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let raw: Option<String> = self.with_conn(|conn| {
            Ok(conn
                .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                    row.get(0)
                })
                .optional()?)
        })?;
        match raw {
            Some(raw) => Ok(Some(serde_json::from_str(&raw)?)),
            None => Ok(None),
        }
    }

    /// Write a JSON-encoded setting.
    pub fn set_setting<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let encoded = serde_json::to_string(value)?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, encoded],
            )?;
            Ok(())
        })
    }

    // -------------------------------------------------------------- trends

    /// Record a point on the disk-usage trend line.
    pub fn record_disk_usage(&self, disk: &DiskInfo) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO disk_snapshots (taken_at, mount_point, total, available)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    to_ts(OffsetDateTime::now_utc()),
                    disk.mount_point.to_string_lossy(),
                    disk.total.get() as i64,
                    disk.available.get() as i64,
                ],
            )?;
            Ok(())
        })
    }

    /// Disk usage over time for one mount point, oldest first.
    pub fn disk_trend(&self, mount_point: &Path, limit: usize) -> Result<Vec<DiskSnapshot>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT taken_at, total, available FROM disk_snapshots
                 WHERE mount_point = ?1 ORDER BY taken_at DESC LIMIT ?2",
            )?;
            let mut rows = stmt
                .query_map(
                    params![mount_point.to_string_lossy(), limit as i64],
                    |row| {
                        Ok(DiskSnapshot {
                            taken_at: from_ts(&row.get::<_, String>(0)?),
                            total: Bytes(row.get::<_, i64>(1)? as u64),
                            available: Bytes(row.get::<_, i64>(2)? as u64),
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.reverse();
            Ok(rows)
        })
    }

    /// Aggregate per-scanner statistics.
    pub fn scanner_stats(&self) -> Result<Vec<ScannerStats>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT scanner, runs, total_items, total_size, total_duration_ms
                 FROM scanner_stats ORDER BY total_size DESC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(ScannerStats {
                        scanner: row.get(0)?,
                        runs: row.get::<_, i64>(1)? as u64,
                        total_items: row.get::<_, i64>(2)? as u64,
                        total_size: Bytes(row.get::<_, i64>(3)? as u64),
                        total_duration_ms: row.get::<_, i64>(4)? as u64,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }
}

/// A row in the scan history list.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanHistoryEntry {
    /// Scan id.
    pub id: String,
    /// When it ran.
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// How long it took.
    pub duration_ms: u64,
    /// Results found.
    pub total_items: usize,
    /// Total size found.
    pub total_size: Bytes,
    /// Safely reclaimable size found.
    pub safe_size: Bytes,
}

/// A point on the disk usage trend line.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskSnapshot {
    /// When the sample was taken.
    #[serde(with = "time::serde::rfc3339")]
    pub taken_at: OffsetDateTime,
    /// Capacity at that time.
    pub total: Bytes,
    /// Free space at that time.
    pub available: Bytes,
}

/// Lifetime statistics for one scanner.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannerStats {
    /// Scanner id.
    pub scanner: String,
    /// How many scans it took part in.
    pub runs: u64,
    /// Total results contributed.
    pub total_items: u64,
    /// Total size contributed.
    pub total_size: Bytes,
    /// Total time spent scanning.
    pub total_duration_ms: u64,
}

fn to_ts(value: OffsetDateTime) -> String {
    value
        .format(&Rfc3339)
        .unwrap_or_else(|_| String::from("1970-01-01T00:00:00Z"))
}

fn from_ts(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &Rfc3339).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

const fn status_to_str(status: ItemStatus) -> &'static str {
    match status {
        ItemStatus::Trashed => "trashed",
        ItemStatus::Deleted => "deleted",
        ItemStatus::Planned => "planned",
        ItemStatus::Skipped => "skipped",
        ItemStatus::Failed => "failed",
    }
}

fn status_from_str(value: &str) -> ItemStatus {
    match value {
        "trashed" => ItemStatus::Trashed,
        "deleted" => ItemStatus::Deleted,
        "planned" => ItemStatus::Planned,
        "skipped" => ItemStatus::Skipped,
        _ => ItemStatus::Failed,
    }
}

/// Initial schema.
const MIGRATION_001: &str = r#"
CREATE TABLE IF NOT EXISTS scans (
    id           TEXT PRIMARY KEY,
    started_at   TEXT NOT NULL,
    duration_ms  INTEGER NOT NULL,
    cancelled    INTEGER NOT NULL DEFAULT 0,
    total_items  INTEGER NOT NULL DEFAULT 0,
    total_size   INTEGER NOT NULL DEFAULT 0,
    safe_size    INTEGER NOT NULL DEFAULT 0,
    review_size  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_scans_started_at ON scans (started_at DESC);

CREATE TABLE IF NOT EXISTS scan_items (
    scan_id   TEXT NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    item_id   TEXT NOT NULL,
    scanner   TEXT NOT NULL,
    title     TEXT NOT NULL,
    path      TEXT NOT NULL,
    size      INTEGER NOT NULL DEFAULT 0,
    category  TEXT NOT NULL,
    risk      TEXT NOT NULL,
    deletable INTEGER NOT NULL DEFAULT 0,
    score     REAL NOT NULL DEFAULT 0,
    PRIMARY KEY (scan_id, item_id)
);
CREATE INDEX IF NOT EXISTS idx_scan_items_scanner ON scan_items (scanner);

CREATE TABLE IF NOT EXISTS cleanups (
    id           TEXT PRIMARY KEY,
    performed_at TEXT NOT NULL,
    mode         TEXT NOT NULL,
    dry_run      INTEGER NOT NULL DEFAULT 0,
    freed        INTEGER NOT NULL DEFAULT 0,
    removed      INTEGER NOT NULL DEFAULT 0,
    skipped      INTEGER NOT NULL DEFAULT 0,
    failed       INTEGER NOT NULL DEFAULT 0,
    undoable     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_cleanups_performed_at ON cleanups (performed_at DESC);

CREATE TABLE IF NOT EXISTS cleanup_items (
    cleanup_id TEXT NOT NULL REFERENCES cleanups(id) ON DELETE CASCADE,
    item_id    TEXT NOT NULL,
    path       TEXT NOT NULL,
    size       INTEGER NOT NULL DEFAULT 0,
    status     TEXT NOT NULL,
    message    TEXT
);
CREATE INDEX IF NOT EXISTS idx_cleanup_items_cleanup ON cleanup_items (cleanup_id);

CREATE TABLE IF NOT EXISTS ignored_paths (
    path     TEXT PRIMARY KEY,
    added_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS scheduled_tasks (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    schedule    TEXT NOT NULL,
    scanners    TEXT NOT NULL DEFAULT '[]',
    enabled     INTEGER NOT NULL DEFAULT 1,
    last_run_at TEXT
);

CREATE TABLE IF NOT EXISTS scanner_stats (
    scanner           TEXT PRIMARY KEY,
    runs              INTEGER NOT NULL DEFAULT 0,
    total_items       INTEGER NOT NULL DEFAULT 0,
    total_size        INTEGER NOT NULL DEFAULT 0,
    total_duration_ms INTEGER NOT NULL DEFAULT 0,
    last_run_at       TEXT
);

CREATE TABLE IF NOT EXISTS disk_snapshots (
    taken_at    TEXT NOT NULL,
    mount_point TEXT NOT NULL,
    total       INTEGER NOT NULL,
    available   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_disk_snapshots ON disk_snapshots (mount_point, taken_at DESC);
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recommend;
    use crate::scan::{ScannerOutcome, ScannerStatus};
    use crate::types::{Category, RiskLevel, ScanResult};

    fn report(id: &str, results: Vec<ScanResult>) -> ScanReport {
        let summary = recommend::summarize(&results);
        ScanReport {
            id: id.into(),
            started_at: OffsetDateTime::UNIX_EPOCH,
            duration_ms: 1234,
            cancelled: false,
            scanners: vec![ScannerOutcome {
                scanner: "cache".into(),
                name: "Cache".into(),
                status: ScannerStatus::Completed,
                items: results.len(),
                size: summary.total_size,
                duration_ms: 1200,
                message: None,
            }],
            results,
            summary,
        }
    }

    fn result(path: &str, size: u64) -> ScanResult {
        ScanResult::builder("cache", path)
            .size(Bytes(size))
            .category(Category::Cache)
            .risk(RiskLevel::Safe)
            .deletable(true)
            .build()
    }

    fn cleanup(id: &str, freed: u64) -> CleanupRecord {
        CleanupRecord {
            id: id.into(),
            performed_at: OffsetDateTime::UNIX_EPOCH,
            mode: CleanupMode::Trash,
            dry_run: false,
            freed: Bytes(freed),
            removed: 1,
            skipped: 0,
            failed: 0,
            undoable: true,
            items: vec![ItemOutcome {
                id: "item".into(),
                path: PathBuf::from("/home/tester/.cache/x"),
                size: Bytes(freed),
                status: ItemStatus::Trashed,
                message: None,
            }],
        }
    }

    #[test]
    fn a_fresh_database_is_migrated() {
        let db = Database::in_memory().expect("open");
        assert_eq!(db.schema_version().expect("version"), SCHEMA_VERSION);
    }

    #[test]
    fn migrations_are_idempotent() {
        let db = Database::in_memory().expect("open");
        db.migrate().expect("second migration");
        db.migrate().expect("third migration");
        assert_eq!(db.schema_version().expect("version"), SCHEMA_VERSION);
    }

    #[test]
    fn scans_round_trip() {
        let db = Database::in_memory().expect("open");
        db.record_scan(&report(
            "scan-1",
            vec![result("/home/tester/.cache/a", 500)],
        ))
        .expect("record");

        let history = db.recent_scans(10).expect("history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, "scan-1");
        assert_eq!(history[0].total_size, Bytes(500));
        assert_eq!(history[0].safe_size, Bytes(500));
    }

    #[test]
    fn re_recording_a_scan_replaces_it() {
        let db = Database::in_memory().expect("open");
        db.record_scan(&report("scan-1", vec![result("/home/tester/a", 100)]))
            .expect("first");
        db.record_scan(&report("scan-1", vec![result("/home/tester/a", 100)]))
            .expect("second");
        assert_eq!(db.recent_scans(10).expect("history").len(), 1);
    }

    #[test]
    fn scanner_stats_accumulate_across_runs() {
        let db = Database::in_memory().expect("open");
        db.record_scan(&report("scan-1", vec![result("/home/tester/a", 100)]))
            .expect("first");
        db.record_scan(&report("scan-2", vec![result("/home/tester/b", 300)]))
            .expect("second");

        let stats = db.scanner_stats().expect("stats");
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].runs, 2);
        assert_eq!(stats[0].total_size, Bytes(400));
    }

    #[test]
    fn cleanups_round_trip_with_their_items() {
        let db = Database::in_memory().expect("open");
        db.record_cleanup(&cleanup("clean-1", 4096))
            .expect("record");

        let history = db.cleanup_history(10).expect("history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].freed, Bytes(4096));
        assert_eq!(history[0].items.len(), 1);
        assert_eq!(history[0].items[0].status, ItemStatus::Trashed);
        assert_eq!(db.total_freed().expect("total"), Bytes(4096));
    }

    #[test]
    fn dry_runs_are_excluded_from_history_and_totals() {
        let db = Database::in_memory().expect("open");
        let mut preview = cleanup("preview", 9999);
        preview.dry_run = true;
        db.record_cleanup(&preview).expect("record");

        assert!(db.cleanup_history(10).expect("history").is_empty());
        assert_eq!(db.total_freed().expect("total"), Bytes::ZERO);
    }

    #[test]
    fn ignored_paths_can_be_added_and_removed() {
        let db = Database::in_memory().expect("open");
        let path = Path::new("/home/tester/keep");
        db.add_ignored(path).expect("add");
        db.add_ignored(path).expect("add twice is fine");
        assert_eq!(
            db.ignored_paths().expect("list"),
            vec![PathBuf::from("/home/tester/keep")]
        );

        db.remove_ignored(path).expect("remove");
        assert!(db.ignored_paths().expect("list").is_empty());
    }

    #[test]
    fn settings_round_trip_and_overwrite() {
        let db = Database::in_memory().expect("open");
        assert_eq!(db.get_setting::<u32>("stale_days").expect("get"), None);

        db.set_setting("stale_days", &90u32).expect("set");
        assert_eq!(db.get_setting::<u32>("stale_days").expect("get"), Some(90));

        db.set_setting("stale_days", &30u32).expect("overwrite");
        assert_eq!(db.get_setting::<u32>("stale_days").expect("get"), Some(30));
    }

    #[test]
    fn disk_trend_is_returned_oldest_first() {
        let db = Database::in_memory().expect("open");
        let disk = DiskInfo {
            name: "disk".into(),
            mount_point: PathBuf::from("/"),
            file_system: "apfs".into(),
            total: Bytes(1000),
            available: Bytes(400),
            removable: false,
        };
        db.record_disk_usage(&disk).expect("record");
        db.record_disk_usage(&DiskInfo {
            available: Bytes(300),
            ..disk.clone()
        })
        .expect("record");

        let trend = db.disk_trend(Path::new("/"), 10).expect("trend");
        assert_eq!(trend.len(), 2);
        assert!(trend[0].taken_at <= trend[1].taken_at);
    }

    #[test]
    fn deleting_a_scan_cascades_to_its_items() {
        let db = Database::in_memory().expect("open");
        db.record_scan(&report("scan-1", vec![result("/home/tester/a", 100)]))
            .expect("record");
        db.with_conn(|conn| {
            conn.execute("DELETE FROM scans WHERE id = 'scan-1'", [])?;
            let remaining: i64 =
                conn.query_row("SELECT COUNT(*) FROM scan_items", [], |row| row.get(0))?;
            assert_eq!(remaining, 0);
            Ok(())
        })
        .expect("cascade");
    }

    #[test]
    fn opening_a_file_backed_database_creates_parent_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/deeper/spacekeeper.db");
        let db = Database::open(&path).expect("open");
        assert!(path.exists());
        assert_eq!(db.schema_version().expect("version"), SCHEMA_VERSION);
    }
}
