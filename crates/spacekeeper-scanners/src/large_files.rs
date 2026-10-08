//! Individual files big enough to be worth a decision.
//!
//! This scanner never claims a file is disposable — it cannot know. It reports
//! size and age and lets the user decide, which is why everything it produces
//! is [`RiskLevel::Review`] with modest confidence.

use spacekeeper_core::{
    fsutil, Bytes, Category, EntryKind, Family, Result, RiskLevel, ScanContext, ScanResult,
    Scanner, ScannerMetadata,
};

use crate::rules::display_path;
use crate::walk::{walk, Descend};

/// Most files to report.
const MAX_ITEMS: usize = 300;

/// Finds unusually large individual files.
#[derive(Debug, Clone, Default)]
pub struct LargeFileScanner;

#[async_trait::async_trait]
impl Scanner for LargeFileScanner {
    fn metadata(&self) -> ScannerMetadata {
        ScannerMetadata {
            id: "large_files".into(),
            name: "Large files".into(),
            description: "Individual files taking up a lot of space".into(),
            explanation: "The biggest single files in your home folder, with the last time each was opened. SpaceKeeper does not know whether you still need them, so nothing here is removed without your review.".into(),
            family: Family::General,
            default_risk: RiskLevel::Review,
            platforms: vec!["macos".into(), "linux".into(), "windows".into()],
            enabled_by_default: true,
            requires_tool: None,
        }
    }

    async fn scan(&self, context: &ScanContext) -> Result<Vec<ScanResult>> {
        let progress = context.progress_for("large_files");
        let roots = context.roots.clone();
        let guard = std::sync::Arc::clone(&context.guard);
        let control = context.control.clone();
        let max_depth = context.options.max_depth;
        let threshold = context.options.large_file_threshold;
        let home = context.home.clone();

        let results = tokio::task::spawn_blocking(move || -> Result<Vec<ScanResult>> {
            let mut found: Vec<ScanResult> = Vec::new();

            walk(
                &roots,
                &guard,
                &control,
                max_depth,
                |_| Descend::Yes,
                |entry| {
                    if !entry.file_type().is_file() {
                        return;
                    }
                    let Ok(meta) = entry.metadata() else { return };
                    if meta.len() < threshold.get() {
                        return;
                    }
                    let (accessed, modified) = fsutil::timestamps(&meta);
                    found.push(
                        ScanResult::builder("large_files", entry.path())
                            .title(display_path(entry.path(), &home))
                            .description(
                                "A large file. Check whether you still need it before removing it."
                                    .to_string(),
                            )
                            .size(Bytes(meta.len()))
                            .file_count(1)
                            .kind(EntryKind::File)
                            .category(Category::LargeFile)
                            .risk(RiskLevel::Review)
                            .deletable(true)
                            .recoverable(true)
                            .last_accessed(accessed)
                            .last_modified(modified)
                            .confidence(0.4)
                            .build(),
                    );
                },
            )?;

            found.sort_by(|a, b| b.size.cmp(&a.size));
            found.truncate(MAX_ITEMS);
            Ok(found)
        })
        .await
        .map_err(|err| spacekeeper_core::Error::Other(anyhow::anyhow!(err)))??;

        let total: Bytes = results.iter().map(|r| r.size).sum();
        progress.scanning(results.len() as u64, total, None);
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spacekeeper_core::{Control, ScanOptions};
    use std::fs;
    use std::path::Path;

    fn context(home: &Path, threshold: u64) -> ScanContext {
        ScanContext::new(home.to_path_buf()).with_options(ScanOptions {
            min_result_size: Bytes::ZERO,
            large_file_threshold: Bytes(threshold),
            ..ScanOptions::default()
        })
    }

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, vec![b'x'; bytes]).expect("write");
    }

    #[tokio::test]
    async fn reports_files_at_or_above_the_threshold() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join("videos/movie.mp4"), 5000);
        write(&home.path().join("videos/note.txt"), 10);

        let results = LargeFileScanner
            .scan(&context(home.path(), 1000))
            .await
            .expect("scan");
        assert_eq!(results.len(), 1);
        assert!(results[0].path.ends_with("movie.mp4"));
        assert_eq!(results[0].kind, EntryKind::File);
        assert_eq!(results[0].risk, RiskLevel::Review);
    }

    #[tokio::test]
    async fn never_reports_directories() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join("bulk/a.bin"), 600);
        write(&home.path().join("bulk/b.bin"), 600);

        let results = LargeFileScanner
            .scan(&context(home.path(), 1000))
            .await
            .expect("scan");
        assert!(results.is_empty(), "a directory is not a large file");
    }

    #[tokio::test]
    async fn skips_protected_folders() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join("Documents/thesis.pdf"), 9000);

        let results = LargeFileScanner
            .scan(&context(home.path(), 1000))
            .await
            .expect("scan");
        assert!(results.is_empty(), "protected folders are not even read");
    }

    #[tokio::test]
    async fn results_are_ordered_largest_first() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join("a.bin"), 2000);
        write(&home.path().join("b.bin"), 8000);

        let results = LargeFileScanner
            .scan(&context(home.path(), 1000))
            .await
            .expect("scan");
        assert_eq!(results[0].size, Bytes(8000));
        assert_eq!(results[1].size, Bytes(2000));
    }

    #[tokio::test]
    async fn cancellation_is_reported_as_an_error() {
        let home = tempfile::tempdir().expect("tempdir");
        for i in 0..1500 {
            write(&home.path().join(format!("d{}/f{i}.bin", i % 20)), 1);
        }
        let control = Control::new();
        control.cancel();
        let context = context(home.path(), 1).with_control(control);
        assert!(LargeFileScanner.scan(&context).await.is_err());
    }

    #[tokio::test]
    async fn an_empty_home_yields_nothing() {
        let home = tempfile::tempdir().expect("tempdir");
        assert!(LargeFileScanner
            .scan(&context(home.path(), 1000))
            .await
            .expect("scan")
            .is_empty());
    }
}
