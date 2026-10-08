//! Downloads: files the user acquired and then forgot.
//!
//! `~/Desktop` deliberately has no equivalent scanner. Desktop files are
//! things the user made, not things that arrived, so they are classified as
//! personal data and automated cleanup never offers them. The file explorer
//! handles that folder instead, with the user looking straight at it.
//!
//! Unlike a cache, nothing here regenerates. These are always
//! [`RiskLevel::Review`] and are reported per item with their age, so the user
//! is deciding about "the 4 GB installer you last opened 14 months ago" rather
//! than about "Downloads".

use std::path::PathBuf;

use spacekeeper_core::{
    fsutil, Bytes, Category, EntryKind, Family, Result, RiskLevel, ScanContext, ScanResult,
    Scanner, ScannerMetadata,
};
use time::OffsetDateTime;

use crate::rules::display_path;

/// Most items to report from one folder.
///
/// A Downloads folder with 3000 files is a real thing; showing all of them is
/// not useful and makes the UI crawl. The largest, stalest ones are the ones
/// worth deciding about.
const MAX_ITEMS: usize = 200;

/// Scans a single user folder for stale items.
#[derive(Debug, Clone)]
pub struct StaleFolderScanner {
    id: &'static str,
    name: &'static str,
    folder: &'static str,
    category: Category,
    explanation: &'static str,
}

impl StaleFolderScanner {
    /// The Downloads folder.
    pub const fn downloads() -> Self {
        Self {
            id: "downloads",
            name: "Old downloads",
            folder: "~/Downloads",
            category: Category::Downloads,
            explanation: "Files you downloaded and have not opened in a long time. Nothing here is recreated automatically, so check the list before removing anything.",
        }
    }

}

#[async_trait::async_trait]
impl Scanner for StaleFolderScanner {
    fn metadata(&self) -> ScannerMetadata {
        ScannerMetadata {
            id: self.id.to_string(),
            name: self.name.to_string(),
            description: format!("Items in {} you have not opened recently", self.folder),
            explanation: self.explanation.to_string(),
            family: Family::General,
            default_risk: RiskLevel::Review,
            platforms: vec!["macos".into(), "linux".into(), "windows".into()],
            enabled_by_default: true,
            requires_tool: None,
        }
    }

    fn is_available(&self, context: &ScanContext) -> bool {
        context.resolve(self.folder).is_dir()
    }

    async fn scan(&self, context: &ScanContext) -> Result<Vec<ScanResult>> {
        let folder = context.resolve(self.folder);
        let progress = context.progress_for(self.id);
        let control = context.control.clone();
        let guard = std::sync::Arc::clone(&context.guard);
        let min_size = context.options.min_result_size;
        let stale_days = f64::from(context.options.stale_after_days);
        let (id, name, category, explanation) =
            (self.id, self.name, self.category, self.explanation);
        let home = context.home.clone();

        let results = tokio::task::spawn_blocking(move || -> Result<Vec<ScanResult>> {
            let now = OffsetDateTime::now_utc();
            let mut entries: Vec<PathBuf> = Vec::new();
            for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                control.check()?;
                let path = entry.path();
                if guard.check(&path).is_ok() {
                    entries.push(path);
                }
            }

            let mut found: Vec<ScanResult> = fsutil::measure_all(&entries, &control)
                .into_iter()
                .filter(|(_, stats)| stats.size >= min_size)
                .map(|(path, stats)| {
                    let is_dir = path.is_dir();
                    ScanResult::builder(id, &path)
                        .title(display_path(&path, &home))
                        .description(explanation.to_string())
                        .size(stats.size)
                        .file_count(stats.files.max(1))
                        .kind(if is_dir {
                            EntryKind::Directory
                        } else {
                            EntryKind::File
                        })
                        .category(category)
                        .risk(RiskLevel::Review)
                        .deletable(true)
                        .recoverable(true)
                        .last_accessed(stats.last_accessed)
                        .last_modified(stats.last_modified)
                        .confidence(0.6)
                        .build()
                })
                // Something opened this week is not clutter, whatever its size.
                .filter(|result| result.unused_days(now) >= stale_days)
                .collect();

            found.sort_by(|a, b| b.size.cmp(&a.size));
            found.truncate(MAX_ITEMS);
            Ok(found)
        })
        .await
        .map_err(|err| spacekeeper_core::Error::Other(anyhow::anyhow!(err)))??;

        let total: Bytes = results.iter().map(|r| r.size).sum();
        progress.scanning(results.len() as u64, total, Some(name.to_string()));
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spacekeeper_core::ScanOptions;
    use std::fs;
    use std::path::Path;

    fn context(home: &Path, stale_after_days: u32) -> ScanContext {
        ScanContext::new(home.to_path_buf()).with_options(ScanOptions {
            min_result_size: Bytes::ZERO,
            stale_after_days,
            ..ScanOptions::default()
        })
    }

    fn write_old(path: &Path, bytes: usize, days_old: u64) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, vec![b'x'; bytes]).expect("write");
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(days_old * 86_400);
        let times = fs::FileTimes::new().set_accessed(when).set_modified(when);
        let file = fs::File::options().write(true).open(path).expect("open");
        file.set_times(times).expect("set times");
    }

    #[tokio::test]
    async fn reports_stale_downloads() {
        let home = tempfile::tempdir().expect("tempdir");
        write_old(&home.path().join("Downloads/installer.dmg"), 4096, 400);

        let results = StaleFolderScanner::downloads()
            .scan(&context(home.path(), 90))
            .await
            .expect("scan");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].size, Bytes(4096));
        assert_eq!(
            results[0].risk,
            RiskLevel::Review,
            "downloads are never one-click"
        );
        assert_eq!(results[0].kind, EntryKind::File);
        assert!(results[0].deletable);
    }

    #[tokio::test]
    async fn ignores_recently_used_files() {
        let home = tempfile::tempdir().expect("tempdir");
        write_old(&home.path().join("Downloads/today.zip"), 4096, 0);

        let results = StaleFolderScanner::downloads()
            .scan(&context(home.path(), 90))
            .await
            .expect("scan");
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn measures_directories_recursively() {
        let home = tempfile::tempdir().expect("tempdir");
        write_old(&home.path().join("Downloads/archive/a.bin"), 1000, 400);
        write_old(&home.path().join("Downloads/archive/b.bin"), 2000, 400);

        let results = StaleFolderScanner::downloads()
            .scan(&context(home.path(), 90))
            .await
            .expect("scan");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].size, Bytes(3000));
        assert_eq!(results[0].kind, EntryKind::Directory);
        assert_eq!(results[0].file_count, 2);
    }

    #[tokio::test]
    async fn results_are_ordered_largest_first_and_capped() {
        let home = tempfile::tempdir().expect("tempdir");
        for i in 0..(MAX_ITEMS + 20) {
            write_old(&home.path().join(format!("Downloads/f{i}.bin")), i + 1, 400);
        }

        let results = StaleFolderScanner::downloads()
            .scan(&context(home.path(), 90))
            .await
            .expect("scan");

        assert_eq!(results.len(), MAX_ITEMS);
        assert!(results.windows(2).all(|w| w[0].size >= w[1].size));
    }

    #[tokio::test]
    async fn a_missing_folder_yields_nothing() {
        let home = tempfile::tempdir().expect("tempdir");
        let scanner = StaleFolderScanner::downloads();
        assert!(!scanner.is_available(&context(home.path(), 90)));
        assert!(scanner
            .scan(&context(home.path(), 90))
            .await
            .expect("scan")
            .is_empty());
    }


    #[tokio::test]
    async fn small_items_are_filtered_by_the_size_threshold() {
        let home = tempfile::tempdir().expect("tempdir");
        write_old(&home.path().join("Downloads/tiny.txt"), 10, 400);

        let context = ScanContext::new(home.path().to_path_buf()).with_options(ScanOptions {
            min_result_size: Bytes(1000),
            stale_after_days: 90,
            ..ScanOptions::default()
        });
        assert!(StaleFolderScanner::downloads()
            .scan(&context)
            .await
            .expect("scan")
            .is_empty());
    }
}
