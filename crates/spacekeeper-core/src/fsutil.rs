//! Filesystem measurement helpers shared by every scanner.
//!
//! Scanners should never hand-roll a directory walk: doing it here once means
//! cancellation, symlink safety and timestamp handling are consistent, and a
//! bug fixed here is fixed for every scanner including third-party ones.

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use time::OffsetDateTime;
use walkdir::WalkDir;

use crate::control::Control;
use crate::error::Result;
use crate::types::Bytes;

/// Check the cancel/pause handle once every this many directory entries.
///
/// An atomic load per entry would be fine, but the pause check takes a mutex,
/// and at tens of millions of entries that starts to show up in profiles.
const CHECK_INTERVAL: u64 = 256;

/// Aggregate facts about a directory tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirStats {
    /// Total apparent size of all regular files in the tree.
    pub size: Bytes,
    /// Number of regular files.
    pub files: u64,
    /// Number of directories, excluding the root itself.
    pub dirs: u64,
    /// Most recent access time seen anywhere in the tree.
    pub last_accessed: Option<OffsetDateTime>,
    /// Most recent modification time seen anywhere in the tree.
    pub last_modified: Option<OffsetDateTime>,
    /// Whether the tree spans more than one filesystem.
    ///
    /// A folder with an external disk mounted inside it looks like an ordinary
    /// folder, and deleting it would take the other disk's contents with it.
    /// Scanners use this to refuse to offer such a folder at all.
    pub crosses_mount: bool,
}

impl DirStats {
    /// Whether the tree contains no files at all.
    pub fn is_empty(&self) -> bool {
        self.files == 0
    }

    fn observe(
        &mut self,
        size: u64,
        accessed: Option<OffsetDateTime>,
        modified: Option<OffsetDateTime>,
    ) {
        self.size = self.size.saturating_add(Bytes(size));
        self.files += 1;
        self.last_accessed = newer(self.last_accessed, accessed);
        self.last_modified = newer(self.last_modified, modified);
    }
}

fn newer(a: Option<OffsetDateTime>, b: Option<OffsetDateTime>) -> Option<OffsetDateTime> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (some, None) | (None, some) => some,
    }
}

/// The filesystem a path lives on, where the platform exposes one.
///
/// Returns `None` on platforms without a cheap device id; callers treat that
/// as "cannot tell" and fall back to their safe default.
pub fn device_of(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().as_ref().and_then(device_id)
}

#[cfg(unix)]
fn device_id(meta: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.dev())
}

#[cfg(not(unix))]
fn device_id(_meta: &std::fs::Metadata) -> Option<u64> {
    None
}

/// Whether `path` is where another filesystem is mounted.
///
/// Deleting a mount point means deleting the contents of a different disk —
/// an external drive, a network share, a container volume.
pub fn is_mount_point(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return true;
    };
    match (device_of(path), device_of(parent)) {
        (Some(here), Some(above)) => here != above,
        // Unknown on this platform: shallow-path and volume-root rules in the
        // guard still cover the common cases.
        _ => false,
    }
}

/// Access and modification timestamps of a metadata record.
///
/// Not every filesystem records access times (and many Linux setups mount with
/// `relatime` or `noatime`), so both are optional and callers must cope.
pub fn timestamps(meta: &std::fs::Metadata) -> (Option<OffsetDateTime>, Option<OffsetDateTime>) {
    let accessed = meta.accessed().ok().map(OffsetDateTime::from);
    let modified = meta.modified().ok().map(OffsetDateTime::from);
    (accessed, modified)
}

/// Recursively measure a directory tree.
///
/// Symlinks are never followed: a symlink into `/System` must not make a cache
/// folder look enormous, and it must never be traversed for deletion either.
/// Unreadable entries are skipped rather than failing the whole scan — a
/// permission error on one file is not a reason to lose the other 40 GB of
/// findings.
pub fn measure(path: &Path, control: &Control) -> Result<DirStats> {
    let mut stats = DirStats::default();
    let mut seen: u64 = 0;
    let root_device = device_of(path);

    for entry in WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        seen += 1;
        if seen % CHECK_INTERVAL == 0 {
            control.check()?;
        }

        let file_type = entry.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if entry.depth() > 0 {
                stats.dirs += 1;
            }
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            let (accessed, modified) = timestamps(&meta);
            stats.observe(meta.len(), accessed, modified);
            if let (Some(root), Some(this)) = (root_device, device_id(&meta)) {
                if root != this {
                    stats.crosses_mount = true;
                }
            }
        }
    }

    control.check()?;
    Ok(stats)
}

/// Measure many directories in parallel on the Rayon pool.
///
/// Directory sizing is the dominant cost of a scan and is almost entirely
/// I/O-latency bound, so measuring the twelve candidate cache folders
/// concurrently is close to a free twelve-fold speedup on an SSD.
pub fn measure_all(paths: &[PathBuf], control: &Control) -> Vec<(PathBuf, DirStats)> {
    paths
        .par_iter()
        .filter_map(|path| match measure(path, control) {
            Ok(stats) => Some((path.clone(), stats)),
            Err(err) => {
                if !err.is_cancelled() {
                    tracing::debug!(path = %path.display(), %err, "skipping unreadable directory");
                }
                None
            }
        })
        .collect()
}

/// Size and timestamps of a single file.
pub fn measure_file(path: &Path) -> Result<DirStats> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| crate::error::Error::io(path, e))?;
    if meta.file_type().is_symlink() {
        return Ok(DirStats::default());
    }
    let (accessed, modified) = timestamps(&meta);
    let mut stats = DirStats::default();
    stats.observe(meta.len(), accessed, modified);
    Ok(stats)
}

/// Expand a leading `~` against the given home directory.
///
/// Only a leading `~` is expanded; `~` elsewhere in a path is a legal filename
/// character and is left alone.
pub fn expand_home(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        return home.to_path_buf();
    }
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(path),
    }
}

/// Whether a directory exists and contains at least one entry.
pub fn dir_has_entries(path: &Path) -> bool {
    std::fs::read_dir(path)
        .map(|mut it| it.next().is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, bytes: usize) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, vec![b'x'; bytes]).expect("write file");
    }

    #[test]
    fn measures_nested_trees() {
        let root = tempfile::tempdir().expect("tempdir");
        write(&root.path().join("a.bin"), 100);
        write(&root.path().join("nested/b.bin"), 250);
        write(&root.path().join("nested/deep/c.bin"), 650);

        let stats = measure(root.path(), &Control::new()).expect("measure");
        assert_eq!(stats.size, Bytes(1000));
        assert_eq!(stats.files, 3);
        assert_eq!(stats.dirs, 2);
        assert!(stats.last_modified.is_some());
    }

    #[test]
    fn empty_directory_measures_to_zero() {
        let root = tempfile::tempdir().expect("tempdir");
        let stats = measure(root.path(), &Control::new()).expect("measure");
        assert_eq!(stats.size, Bytes::ZERO);
        assert!(stats.is_empty());
    }

    #[test]
    fn missing_directory_is_not_an_error() {
        let stats = measure(Path::new("/definitely/not/here"), &Control::new()).expect("measure");
        assert_eq!(stats.files, 0);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed_or_counted() {
        let root = tempfile::tempdir().expect("tempdir");
        write(&root.path().join("real/big.bin"), 500);
        let link = root.path().join("link");
        std::os::unix::fs::symlink(root.path().join("real"), &link).expect("symlink");

        let stats = measure(root.path(), &Control::new()).expect("measure");
        assert_eq!(
            stats.size,
            Bytes(500),
            "symlinked tree must not be double counted"
        );
        assert_eq!(stats.files, 1);
    }

    #[test]
    fn cancellation_stops_a_measurement() {
        let root = tempfile::tempdir().expect("tempdir");
        for i in 0..600 {
            write(&root.path().join(format!("f{i}.bin")), 1);
        }
        let control = Control::new();
        control.cancel();
        assert!(measure(root.path(), &control).is_err());
    }

    #[test]
    fn measure_all_skips_unreadable_paths() {
        let root = tempfile::tempdir().expect("tempdir");
        write(&root.path().join("a/x.bin"), 10);
        let paths = vec![root.path().join("a"), root.path().join("missing")];
        let measured = measure_all(&paths, &Control::new());
        // The missing directory yields empty stats rather than disappearing.
        assert_eq!(measured.len(), 2);
        let total: Bytes = measured.iter().map(|(_, s)| s.size).sum();
        assert_eq!(total, Bytes(10));
    }

    #[test]
    fn an_ordinary_directory_is_not_a_mount_point() {
        let root = tempfile::tempdir().expect("tempdir");
        let nested = root.path().join("nested");
        fs::create_dir_all(&nested).expect("mkdir");
        assert!(!is_mount_point(&nested));
    }

    #[test]
    fn the_filesystem_root_counts_as_a_mount_point() {
        // It has no parent to compare against, and deleting it is unthinkable,
        // so the unknown case resolves to the protective answer.
        assert!(is_mount_point(Path::new("/")));
    }

    #[test]
    fn a_single_filesystem_tree_does_not_report_crossing_a_mount() {
        let root = tempfile::tempdir().expect("tempdir");
        write(&root.path().join("a/b.bin"), 10);
        let stats = measure(root.path(), &Control::new()).expect("measure");
        assert!(!stats.crosses_mount);
    }

    #[test]
    fn expands_only_a_leading_tilde() {
        let home = Path::new("/home/tester");
        assert_eq!(expand_home("~", home), PathBuf::from("/home/tester"));
        assert_eq!(
            expand_home("~/.cache", home),
            PathBuf::from("/home/tester/.cache")
        );
        assert_eq!(expand_home("/etc/hosts", home), PathBuf::from("/etc/hosts"));
        assert_eq!(expand_home("/tmp/a~b", home), PathBuf::from("/tmp/a~b"));
    }

    #[test]
    fn measure_file_reports_size() {
        let root = tempfile::tempdir().expect("tempdir");
        let file = root.path().join("f.bin");
        write(&file, 42);
        let stats = measure_file(&file).expect("measure file");
        assert_eq!(stats.size, Bytes(42));
        assert_eq!(stats.files, 1);
    }
}
