//! Byte-identical duplicate files.
//!
//! Hashing every file in a home directory would take minutes and gain nothing,
//! so the work is staged:
//!
//! 1. group candidate files by exact size — two files of different sizes can
//!    never be identical, and this pass is metadata-only;
//! 2. hash only the groups with more than one member, in parallel, with BLAKE3;
//! 3. within a hash group, keep the oldest file and report the rest — skipping
//!    any copy that shares storage with one already kept.
//!
//! On a typical home directory that hashes a few percent of the data.
//!
//! "Keep the oldest" is deliberate: the first copy is usually the one other
//! things reference, and the later copies are the `report (1).pdf` accidents.
//!
//! Files that share storage are excluded, because deleting them frees nothing.
//! Two paths pointing at the same inode — a hard link, or an APFS clone made by
//! duplicating a file in Finder — are byte-identical by definition and would
//! otherwise be reported as an easy win that never materialises.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use rayon::prelude::*;
use spacekeeper_core::{
    fsutil, Bytes, Category, EntryKind, Family, Result, RiskLevel, ScanContext, ScanResult,
    Scanner, ScannerMetadata,
};
use time::OffsetDateTime;

use crate::rules::display_path;
use crate::walk::{walk, Descend};

/// Most duplicates to report.
const MAX_ITEMS: usize = 500;

/// Finds byte-identical copies of the same file.
///
/// Files are hashed in full. A prefix hash would be faster but could call two
/// different files identical, and this tool deletes things.
#[derive(Debug, Clone, Default)]
pub struct DuplicateScanner;

#[async_trait::async_trait]
impl Scanner for DuplicateScanner {
    fn metadata(&self) -> ScannerMetadata {
        ScannerMetadata {
            id: "duplicates".into(),
            name: "Duplicate files".into(),
            description: "Files whose contents are exactly identical".into(),
            explanation: "Files with byte-for-byte identical contents. SpaceKeeper keeps the oldest copy of each and offers the later ones for removal. Check the paths first — some programs keep intentional copies.".into(),
            family: Family::General,
            default_risk: RiskLevel::Review,
            platforms: vec!["macos".into(), "linux".into(), "windows".into()],
            enabled_by_default: true,
            requires_tool: None,
        }
    }

    async fn scan(&self, context: &ScanContext) -> Result<Vec<ScanResult>> {
        let progress = context.progress_for("duplicates");
        let roots = context.roots.clone();
        let guard = std::sync::Arc::clone(&context.guard);
        let control = context.control.clone();
        let max_depth = context.options.max_depth;
        let min_size = context.options.duplicate_min_size;
        let home = context.home.clone();

        let results = tokio::task::spawn_blocking(move || -> Result<Vec<ScanResult>> {
            // Stage 1: group by size, metadata only.
            let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
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
                    if meta.len() < min_size.get() {
                        return;
                    }
                    by_size
                        .entry(meta.len())
                        .or_default()
                        .push(entry.path().to_path_buf());
                },
            )?;

            let candidates: Vec<(u64, Vec<PathBuf>)> = by_size
                .into_iter()
                .filter(|(_, paths)| paths.len() > 1)
                .collect();
            control.check()?;

            // Stage 2: hash the survivors in parallel.
            let hashed: Vec<(blake3::Hash, PathBuf, u64)> = candidates
                .par_iter()
                .flat_map(|(size, paths)| {
                    paths
                        .par_iter()
                        .filter(|_| !control.is_cancelled())
                        .filter_map(|path| hash_file(path).map(|hash| (hash, path.clone(), *size)))
                        .collect::<Vec<_>>()
                })
                .collect();
            control.check()?;

            // Stage 3: within each hash group keep the oldest, report the rest.
            let mut groups: HashMap<blake3::Hash, Vec<(PathBuf, u64)>> = HashMap::new();
            for (hash, path, size) in hashed {
                groups.entry(hash).or_default().push((path, size));
            }

            let mut found = Vec::new();
            for (hash, mut members) in groups {
                if members.len() < 2 {
                    continue;
                }
                members.sort_by_key(|(path, _)| {
                    std::fs::metadata(path)
                        .and_then(|m| m.modified())
                        .map(OffsetDateTime::from)
                        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
                });
                let (original, _) = members[0].clone();

                // Anything sharing storage with a copy we are keeping frees
                // nothing when deleted, so it is not a duplicate worth showing.
                let mut kept_storage: HashSet<FileIdentity> = HashSet::new();
                if let Some(id) = file_identity(&original) {
                    kept_storage.insert(id);
                }

                for (path, size) in members.into_iter().skip(1) {
                    if let Some(id) = file_identity(&path) {
                        if !kept_storage.insert(id) {
                            continue;
                        }
                    }
                    let stats = fsutil::measure_file(&path).unwrap_or_default();
                    found.push(
                        ScanResult::builder("duplicates", &path)
                            .title(display_path(&path, &home))
                            .description(format!(
                                "Identical to {}. That copy is kept; this one can go.",
                                display_path(&original, &home)
                            ))
                            .size(Bytes(size))
                            .file_count(1)
                            .kind(EntryKind::File)
                            .category(Category::Duplicate)
                            .risk(RiskLevel::Review)
                            .deletable(true)
                            .recoverable(true)
                            .last_accessed(stats.last_accessed)
                            .last_modified(stats.last_modified)
                            .confidence(0.9)
                            .detail(serde_json::json!({
                                "group": hash.to_hex()[..16].to_string(),
                                "original": original.to_string_lossy(),
                            }))
                            .build(),
                    );
                }
            }

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

/// Identifies the storage behind a path: the same value means the same bytes
/// on disk, so removing one path frees nothing.
type FileIdentity = (u64, u64);

#[cfg(unix)]
fn file_identity(path: &std::path::Path) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

/// Windows exposes a file index, but only through an open handle; without a
/// cheap equivalent the check is skipped and duplicates are reported as before.
#[cfg(not(unix))]
fn file_identity(_path: &std::path::Path) -> Option<FileIdentity> {
    None
}

/// Hash a file's full contents, returning `None` if it cannot be read.
fn hash_file(path: &std::path::Path) -> Option<blake3::Hash> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_mmap(path).ok()?;
    Some(hasher.finalize())
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
            duplicate_min_size: Bytes(1),
            ..ScanOptions::default()
        })
    }

    fn write(path: &Path, contents: &[u8]) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, contents).expect("write");
    }

    #[tokio::test]
    async fn finds_identical_files() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join("a/report.pdf"), b"same contents here");
        write(
            &home.path().join("b/report copy.pdf"),
            b"same contents here",
        );

        let results = DuplicateScanner
            .scan(&context(home.path()))
            .await
            .expect("scan");
        assert_eq!(results.len(), 1, "one copy is kept, the other reported");
        assert_eq!(results[0].category, Category::Duplicate);
        assert_eq!(results[0].risk, RiskLevel::Review);
    }

    #[tokio::test]
    async fn files_of_the_same_size_but_different_contents_are_not_duplicates() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join("a.bin"), b"aaaaaaaa");
        write(&home.path().join("b.bin"), b"bbbbbbbb");

        let results = DuplicateScanner
            .scan(&context(home.path()))
            .await
            .expect("scan");
        assert!(results.is_empty(), "same size is not the same contents");
    }

    #[tokio::test]
    async fn one_copy_is_always_kept() {
        let home = tempfile::tempdir().expect("tempdir");
        for name in ["a.bin", "b.bin", "c.bin", "d.bin"] {
            write(&home.path().join(name), b"identical");
        }

        let results = DuplicateScanner
            .scan(&context(home.path()))
            .await
            .expect("scan");
        assert_eq!(results.len(), 3, "four copies means three removable");
    }

    #[tokio::test]
    async fn the_kept_copy_is_named_in_the_description() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join("original.bin"), b"identical");
        write(&home.path().join("copy.bin"), b"identical");

        let results = DuplicateScanner
            .scan(&context(home.path()))
            .await
            .expect("scan");
        assert!(results[0].description.starts_with("Identical to ~/"));
        assert!(
            results[0].detail.is_some(),
            "duplicates carry their group hash"
        );
    }

    #[tokio::test]
    async fn files_below_the_minimum_size_are_not_hashed() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join("a.bin"), b"xy");
        write(&home.path().join("b.bin"), b"xy");

        let context = ScanContext::new(home.path().to_path_buf()).with_options(ScanOptions {
            min_result_size: Bytes::ZERO,
            duplicate_min_size: Bytes(1000),
            ..ScanOptions::default()
        });
        assert!(DuplicateScanner
            .scan(&context)
            .await
            .expect("scan")
            .is_empty());
    }

    #[tokio::test]
    async fn protected_folders_are_not_searched() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join("Documents/a.bin"), b"identical");
        write(&home.path().join("Documents/b.bin"), b"identical");

        assert!(DuplicateScanner
            .scan(&context(home.path()))
            .await
            .expect("scan")
            .is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hard_links_are_not_reported_as_duplicates() {
        // Two names for the same bytes. Deleting one frees nothing, so
        // offering it would be a promise the cleanup cannot keep.
        let home = tempfile::tempdir().expect("tempdir");
        let original = home.path().join("original.bin");
        write(&original, b"identical contents here");
        std::fs::hard_link(&original, home.path().join("linked.bin")).expect("hard link");

        let results = DuplicateScanner
            .scan(&context(home.path()))
            .await
            .expect("scan");
        assert!(
            results.is_empty(),
            "a hard link frees no space, so it is not a duplicate worth reporting"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_real_copy_alongside_a_hard_link_still_reports_the_copy() {
        let home = tempfile::tempdir().expect("tempdir");
        let original = home.path().join("original.bin");
        write(&original, b"identical contents here");
        std::fs::hard_link(&original, home.path().join("linked.bin")).expect("hard link");
        write(&home.path().join("realcopy.bin"), b"identical contents here");

        let results = DuplicateScanner
            .scan(&context(home.path()))
            .await
            .expect("scan");
        assert_eq!(results.len(), 1, "only the independent copy frees space");
        assert!(results[0].path.ends_with("realcopy.bin"));
    }

    #[tokio::test]
    async fn a_home_with_no_duplicates_yields_nothing() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join("a.bin"), b"one");
        write(&home.path().join("b.bin"), b"two different");

        assert!(DuplicateScanner
            .scan(&context(home.path()))
            .await
            .expect("scan")
            .is_empty());
    }

    #[test]
    fn hashing_a_missing_file_returns_none() {
        assert!(hash_file(Path::new("/definitely/not/here")).is_none());
    }
}
