//! Directories containing nothing at all.
//!
//! These free essentially no space, so they are reported for tidiness and rank
//! at the bottom of the list by construction (a zero-byte result scores zero).
//! They are [`RiskLevel::Review`] rather than safe because some tools do care
//! about a folder existing, and there is no upside to being aggressive about
//! something that frees nothing.

use spacekeeper_core::{
    Bytes, Category, EntryKind, Family, Result, RiskLevel, ScanContext, ScanResult, Scanner,
    ScannerMetadata,
};

use crate::rules::display_path;
use crate::walk::{walk, Descend};

/// Most empty folders to report.
const MAX_ITEMS: usize = 300;

/// Finds empty directories.
#[derive(Debug, Clone, Default)]
pub struct EmptyFolderScanner;

#[async_trait::async_trait]
impl Scanner for EmptyFolderScanner {
    fn metadata(&self) -> ScannerMetadata {
        ScannerMetadata {
            id: "empty_folders".into(),
            name: "Empty folders".into(),
            description: "Folders that contain no files at all".into(),
            explanation: "Folders with nothing inside them. Removing them frees almost no space — it is about tidiness. Some applications recreate the folders they need.".into(),
            family: Family::General,
            default_risk: RiskLevel::Review,
            platforms: vec!["macos".into(), "linux".into(), "windows".into()],
            enabled_by_default: false,
            requires_tool: None,
        }
    }

    async fn scan(&self, context: &ScanContext) -> Result<Vec<ScanResult>> {
        let roots = context.roots.clone();
        let guard = std::sync::Arc::clone(&context.guard);
        let control = context.control.clone();
        let max_depth = context.options.max_depth;
        let home = context.home.clone();

        tokio::task::spawn_blocking(move || -> Result<Vec<ScanResult>> {
            let mut found = Vec::new();
            walk(
                &roots,
                &guard,
                &control,
                max_depth,
                |_| Descend::Yes,
                |entry| {
                    if !entry.file_type().is_dir() || entry.depth() == 0 {
                        return;
                    }
                    if std::fs::read_dir(entry.path())
                        .map(|mut it| it.next().is_some())
                        .unwrap_or(true)
                    {
                        return;
                    }
                    found.push(
                        ScanResult::builder("empty_folders", entry.path())
                            .title(display_path(entry.path(), &home))
                            .description("This folder contains no files.".to_string())
                            .size(Bytes::ZERO)
                            .file_count(0)
                            .kind(EntryKind::Directory)
                            .category(Category::EmptyFolder)
                            .risk(RiskLevel::Review)
                            .deletable(true)
                            .recoverable(true)
                            .confidence(0.5)
                            .build(),
                    );
                },
            )?;

            found.truncate(MAX_ITEMS);
            Ok(found)
        })
        .await
        .map_err(|err| spacekeeper_core::Error::Other(anyhow::anyhow!(err)))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spacekeeper_core::ScanOptions;
    use std::fs;
    use std::path::Path;

    fn context(home: &Path) -> ScanContext {
        ScanContext::new(home.to_path_buf()).with_options(ScanOptions {
            min_result_size: Bytes::ZERO,
            ..ScanOptions::default()
        })
    }

    #[tokio::test]
    async fn finds_an_empty_directory() {
        let home = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(home.path().join("code/empty")).expect("mkdir");

        let results = EmptyFolderScanner
            .scan(&context(home.path()))
            .await
            .expect("scan");
        assert!(results.iter().any(|r| r.path.ends_with("empty")));
        assert!(results.iter().all(|r| r.size == Bytes::ZERO));
    }

    #[tokio::test]
    async fn a_directory_holding_a_file_is_not_empty() {
        let home = tempfile::tempdir().expect("tempdir");
        let dir = home.path().join("code/full");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join("f.txt"), b"x").expect("write");

        let results = EmptyFolderScanner
            .scan(&context(home.path()))
            .await
            .expect("scan");
        assert!(!results.iter().any(|r| r.path.ends_with("full")));
    }

    #[tokio::test]
    async fn a_directory_holding_only_a_directory_is_not_empty() {
        let home = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(home.path().join("outer/inner")).expect("mkdir");

        let results = EmptyFolderScanner
            .scan(&context(home.path()))
            .await
            .expect("scan");
        assert!(!results.iter().any(|r| r.path.ends_with("outer")));
        assert!(results.iter().any(|r| r.path.ends_with("inner")));
    }

    #[tokio::test]
    async fn the_scan_root_itself_is_never_reported() {
        let home = tempfile::tempdir().expect("tempdir");
        let results = EmptyFolderScanner
            .scan(&context(home.path()))
            .await
            .expect("scan");
        assert!(!results.iter().any(|r| r.path == home.path()));
    }

    #[test]
    fn the_scanner_is_off_by_default() {
        assert!(!EmptyFolderScanner.metadata().enabled_by_default);
    }
}
