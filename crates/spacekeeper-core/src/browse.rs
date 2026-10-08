//! Browsing the filesystem by size.
//!
//! The scanners answer "what is safe to delete?". This answers the other
//! question users have: "what is actually *in* there?" — walking down from the
//! home directory, seeing where the weight is, and removing something specific.
//!
//! The expensive part is that a directory's size is only known by walking all
//! of it, so listing one folder means measuring every child recursively. On a
//! home directory that is minutes of work, and blocking the whole listing on it
//! would leave the user staring at a spinner with no idea what is in there.
//!
//! So the listing is split in two:
//!
//! * [`list_directory`] returns **immediately** with names, kinds, timestamps,
//!   protection, and the sizes of plain files — everything that costs a single
//!   `stat`. Directories start unmeasured;
//! * [`measure_entries`] then walks the directories in parallel on the Rayon
//!   pool and hands each size back through a callback as it lands, so rows fill
//!   in progressively and the user can navigate before it finishes.
//!
//! Results are memoised in a [`SizeCache`], so stepping back up a level — which
//! is most of what browsing *is* — needs no measuring at all.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::control::Control;
use crate::error::{Error, Result};
use crate::fsutil::{self, DirStats};
use crate::protect::{PathGuard, Protection};
use crate::types::{Bytes, EntryKind};

/// Most entries returned for one directory.
///
/// A folder with 50 000 files is real (`node_modules`, a Maildir). Sending all
/// of them would stall the UI for no benefit, so the largest are kept and the
/// listing is marked truncated.
const MAX_ENTRIES: usize = 1_000;

/// One row in a directory listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseEntry {
    /// File or folder name.
    pub name: String,
    /// Absolute path.
    pub path: PathBuf,
    /// File or directory.
    pub kind: EntryKind,
    /// Size, recursive for directories.
    ///
    /// Zero and `measured == false` means "not known yet", not "empty".
    pub size: Bytes,
    /// Whether [`Self::size`] is final.
    ///
    /// Files and cached directories arrive measured; everything else is filled
    /// in by the background pass.
    pub measured: bool,
    /// Files contained, recursive. 1 for a plain file.
    pub file_count: u64,
    /// Fraction of the parent directory's total size, in `0.0..=1.0`.
    pub share: f64,
    /// Last access time, when the filesystem records one.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_accessed: Option<OffsetDateTime>,
    /// Last modification time.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_modified: Option<OffsetDateTime>,
    /// How protected this entry is, and why.
    pub protection: Protection,
    /// Symlinks are shown so the user understands the folder, but are never
    /// measured through and never deleted.
    pub is_symlink: bool,
}

impl BrowseEntry {
    /// Whether the explorer offers a delete control for this entry.
    pub fn is_deletable(&self) -> bool {
        !self.is_symlink && !self.protection.is_refused()
    }
}

/// One step in the path bar.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crumb {
    /// Display name for this step.
    pub name: String,
    /// Path to navigate to.
    pub path: PathBuf,
}

/// The contents of one directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryListing {
    /// The directory being shown.
    pub path: PathBuf,
    /// Its parent, unless navigating up would leave the allowed roots.
    pub parent: Option<PathBuf>,
    /// Path bar, root first.
    pub breadcrumbs: Vec<Crumb>,
    /// Total size of everything listed.
    pub total_size: Bytes,
    /// Entries, largest first.
    pub entries: Vec<BrowseEntry>,
    /// Entries dropped because the directory was enormous.
    pub hidden_entries: usize,
    /// Protection of the directory itself, so the UI can warn on arrival.
    pub protection: Protection,
}

/// How to order a listing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortBy {
    /// Largest first. The only ordering that answers "where did my disk go?".
    #[default]
    Size,
    /// Alphabetical.
    Name,
    /// Most recently changed first.
    Modified,
    /// Least recently used first.
    Oldest,
}

/// Memoised directory sizes.
///
/// Keyed by path. Entries are dropped when something inside them is deleted,
/// which is the only mutation SpaceKeeper itself performs; changes made by
/// other applications are picked up on the next explicit refresh.
#[derive(Debug, Default)]
pub struct SizeCache {
    entries: Mutex<HashMap<PathBuf, DirStats>>,
}

impl SizeCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    fn get(&self, path: &Path) -> Option<DirStats> {
        self.lock().get(path).copied()
    }

    fn put(&self, path: PathBuf, stats: DirStats) {
        self.lock().insert(path, stats);
    }

    /// Forget `path` and every ancestor of it, whose totals just changed.
    ///
    /// Called after a deletion. Forgetting ancestors matters more than
    /// forgetting the path itself: the parent folder's size is what the user
    /// is looking at.
    pub fn invalidate(&self, path: &Path) {
        let mut entries = self.lock();
        entries.remove(path);
        entries.retain(|cached, _| !cached.starts_with(path));
        for ancestor in path.ancestors() {
            entries.remove(ancestor);
        }
    }

    /// Drop everything.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// How many directories are memoised.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, DirStats>> {
        self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A directory size that has finished being measured.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measured {
    /// Which entry this is about.
    pub path: PathBuf,
    /// Its recursive size.
    pub size: Bytes,
    /// How many files it contains.
    pub file_count: u64,
    /// Most recent access time inside it.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_accessed: Option<OffsetDateTime>,
    /// Most recent modification time inside it.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_modified: Option<OffsetDateTime>,
    /// Set when the folder spans filesystems, which makes it undeletable.
    pub crosses_mount: bool,
}

/// List a directory without measuring anything that would need a walk.
///
/// Returns as fast as one `read_dir` plus a `stat` per child, so the UI has
/// something real to show immediately. Directory sizes are zero and
/// `measured` is false unless the size cache already knew them; feed the
/// entries to [`measure_entries`] to fill the rest in.
///
/// `roots` bounds navigation: the user can move around inside them but the
/// breadcrumb bar will not offer a way above them.
pub fn list_directory(
    path: &Path,
    roots: &[PathBuf],
    guard: &PathGuard,
    cache: &SizeCache,
) -> Result<DirectoryListing> {
    let entries = collect_entries(path, guard, cache)?;
    Ok(finish_listing(path, roots, guard, entries, SortBy::Size))
}

/// Every child of `path`, unsorted and untruncated.
///
/// Kept separate from [`finish_listing`] so that truncation happens exactly
/// once: the streaming path trims to the visible rows *before* measuring, so
/// it never walks 50 000 folders nobody will see, while the blocking path
/// measures everything first so its total covers what it trimmed.
fn collect_entries(
    path: &Path,
    guard: &PathGuard,
    cache: &SizeCache,
) -> Result<Vec<BrowseEntry>> {
    if !path.is_dir() {
        return Err(Error::io(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a directory"),
        ));
    }

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| Error::io(path, e))? {
        let Ok(entry) = entry else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let child = entry.path();

        let is_symlink = std::fs::symlink_metadata(&child)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        let is_dir = meta.file_type().is_dir();

        // A symlink is never traversed, so it is measured the moment it is
        // seen: its size is zero and that is final.
        let (stats, measured) = if is_symlink {
            (DirStats::default(), true)
        } else if is_dir {
            match cache.get(&child) {
                Some(cached) => (cached, true),
                None => (DirStats::default(), false),
            }
        } else {
            let (accessed, modified) = fsutil::timestamps(&meta);
            (
                DirStats {
                    size: Bytes(meta.len()),
                    files: 1,
                    dirs: 0,
                    last_accessed: accessed,
                    last_modified: modified,
                    crosses_mount: false,
                },
                true,
            )
        };

        entries.push(entry_from(&child, is_dir, is_symlink, &stats, measured, guard));
    }

    Ok(entries)
}

/// Measure the directories in `entries` that are not yet measured.
///
/// Calls `on_measured` for each one as it completes, so a caller can push the
/// result to the UI rather than waiting for the whole folder. Honours the
/// control handle, which is how switching folders abandons the old work.
pub fn measure_entries(
    entries: &[BrowseEntry],
    control: &Control,
    cache: &SizeCache,
    on_measured: impl Fn(Measured) + Sync + Send,
) {
    let pending: Vec<&BrowseEntry> = entries
        .iter()
        .filter(|entry| !entry.measured && !entry.is_symlink)
        .collect();

    // Largest-looking work last would be ideal, but nothing is known about
    // size yet, so simply fan out and let results arrive as they will.
    pending.par_iter().for_each(|entry| {
        if control.is_cancelled() {
            return;
        }
        let Ok(stats) = fsutil::measure(&entry.path, control) else {
            return;
        };
        cache.put(entry.path.clone(), stats);
        on_measured(Measured {
            path: entry.path.clone(),
            size: stats.size,
            file_count: stats.files,
            last_accessed: stats.last_accessed,
            last_modified: stats.last_modified,
            crosses_mount: stats.crosses_mount,
        });
    });
}

fn entry_from(
    child: &Path,
    is_dir: bool,
    is_symlink: bool,
    stats: &DirStats,
    measured: bool,
    guard: &PathGuard,
) -> BrowseEntry {
    // Deleting a folder that spans filesystems would reach onto another disk.
    // The guard catches a folder that *is* a mount point; this catches one
    // that merely contains one.
    let protection = if stats.crosses_mount {
        Protection::Refused {
            reason: "another disk is mounted inside this folder".into(),
        }
    } else {
        guard.classify(child)
    };

    BrowseEntry {
        name: child
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: child.to_path_buf(),
        kind: if is_dir { EntryKind::Directory } else { EntryKind::File },
        size: stats.size,
        measured,
        file_count: stats.files,
        share: 0.0,
        last_accessed: stats.last_accessed,
        last_modified: stats.last_modified,
        protection,
        is_symlink,
    }
}

fn finish_listing(
    path: &Path,
    roots: &[PathBuf],
    guard: &PathGuard,
    mut entries: Vec<BrowseEntry>,
    sort: SortBy,
) -> DirectoryListing {
    let total_size: Bytes = entries.iter().map(|entry| entry.size).sum();
    sort_entries(&mut entries, sort);

    let hidden_entries = entries.len().saturating_sub(MAX_ENTRIES);
    entries.truncate(MAX_ENTRIES);

    if total_size.get() > 0 {
        for entry in &mut entries {
            entry.share = entry.size.get() as f64 / total_size.get() as f64;
        }
    }

    DirectoryListing {
        parent: parent_within(path, roots),
        breadcrumbs: breadcrumbs(path, roots),
        total_size,
        entries,
        hidden_entries,
        protection: guard.classify(path),
        path: path.to_path_buf(),
    }
}

/// List a directory, measuring every child before returning.
///
/// The blocking counterpart to [`list_directory`] + [`measure_entries`], kept
/// for callers that genuinely want the finished answer in one call.
pub fn browse(
    path: &Path,
    roots: &[PathBuf],
    guard: &PathGuard,
    control: &Control,
    cache: &SizeCache,
    sort: SortBy,
) -> Result<DirectoryListing> {
    let mut entries = collect_entries(path, guard, cache)?;
    control.check()?;

    let measured: Mutex<HashMap<PathBuf, Measured>> = Mutex::new(HashMap::new());
    measure_entries(&entries, control, cache, |result| {
        measured
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(result.path.clone(), result);
    });
    control.check()?;

    let measured = measured.into_inner().unwrap_or_else(|poisoned| poisoned.into_inner());
    for entry in &mut entries {
        let Some(result) = measured.get(&entry.path) else { continue };
        entry.size = result.size;
        entry.file_count = result.file_count;
        entry.last_accessed = result.last_accessed;
        entry.last_modified = result.last_modified;
        entry.measured = true;
        if result.crosses_mount {
            entry.protection = Protection::Refused {
                reason: "another disk is mounted inside this folder".into(),
            };
        }
    }

    Ok(finish_listing(path, roots, guard, entries, sort))
}

fn sort_entries(entries: &mut [BrowseEntry], sort: SortBy) {
    match sort {
        SortBy::Size => entries.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))),
        SortBy::Name => entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        SortBy::Modified => entries.sort_by(|a, b| b.last_modified.cmp(&a.last_modified)),
        SortBy::Oldest => entries.sort_by(|a, b| {
            let key = |e: &BrowseEntry| e.last_accessed.or(e.last_modified);
            key(a).cmp(&key(b))
        }),
    }
}

/// The parent directory, unless that would navigate above every allowed root.
///
/// The parent must itself lie inside a root. Testing the other direction as
/// well ("the root is inside the parent") looks symmetric but is wrong: it is
/// true for every root's own parent, which would hand back a way to walk
/// straight out of the sandbox.
fn parent_within(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let parent = path.parent()?;
    roots
        .iter()
        .any(|root| parent.starts_with(root))
        .then(|| parent.to_path_buf())
}

/// Build the path bar, starting at whichever root contains `path`.
fn breadcrumbs(path: &Path, roots: &[PathBuf]) -> Vec<Crumb> {
    let root = roots
        .iter()
        .filter(|root| path.starts_with(root))
        // The most specific matching root, if they nest.
        .max_by_key(|root| root.components().count());

    let (base, rest) = match root {
        Some(root) => (root.clone(), path.strip_prefix(root).ok()),
        None => (PathBuf::from("/"), path.strip_prefix("/").ok()),
    };

    let mut crumbs = vec![Crumb {
        name: base
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| base.to_string_lossy().into_owned()),
        path: base.clone(),
    }];

    let mut current = base;
    for component in rest.into_iter().flatten() {
        current = current.join(component);
        crumbs.push(Crumb {
            name: component.to_string_lossy().into_owned(),
            path: current.clone(),
        });
    }
    crumbs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, vec![b'x'; bytes]).expect("write");
    }

    struct Fixture {
        home: tempfile::TempDir,
        cache: SizeCache,
    }

    impl Fixture {
        fn new() -> Self {
            let home = tempfile::tempdir().expect("tempdir");
            write(&home.path().join("code/big/a.bin"), 8_000);
            write(&home.path().join("code/big/nested/b.bin"), 2_000);
            write(&home.path().join("code/small/c.bin"), 500);
            write(&home.path().join("notes.txt"), 100);
            write(&home.path().join(".ssh/id_rsa"), 50);
            write(&home.path().join("Documents/thesis.pdf"), 3_000);
            Self { home, cache: SizeCache::new() }
        }

        fn list(&self, relative: &str) -> DirectoryListing {
            let path = if relative.is_empty() {
                self.home.path().to_path_buf()
            } else {
                self.home.path().join(relative)
            };
            browse(
                &path,
                &[self.home.path().to_path_buf()],
                &PathGuard::new(self.home.path()),
                &Control::new(),
                &self.cache,
                SortBy::Size,
            )
            .expect("browse")
        }

        fn entry<'a>(&self, listing: &'a DirectoryListing, name: &str) -> &'a BrowseEntry {
            listing
                .entries
                .iter()
                .find(|entry| entry.name == name)
                .unwrap_or_else(|| panic!("no entry named {name}"))
        }
    }

    #[test]
    fn directories_report_their_recursive_size() {
        let fixture = Fixture::new();
        let listing = fixture.list("code");
        assert_eq!(fixture.entry(&listing, "big").size, Bytes(10_000));
        assert_eq!(fixture.entry(&listing, "big").file_count, 2);
    }

    #[test]
    fn entries_are_largest_first_by_default() {
        let fixture = Fixture::new();
        let listing = fixture.list("code");
        assert_eq!(listing.entries[0].name, "big");
        assert_eq!(listing.entries[1].name, "small");
    }

    #[test]
    fn shares_add_up_to_the_whole_directory() {
        let fixture = Fixture::new();
        let listing = fixture.list("code");
        let total: f64 = listing.entries.iter().map(|entry| entry.share).sum();
        assert!((total - 1.0).abs() < 0.0001, "shares summed to {total}");
        assert_eq!(listing.total_size, Bytes(10_500));
    }

    #[test]
    fn protection_is_attached_to_every_entry() {
        let fixture = Fixture::new();
        let listing = fixture.list("");

        assert!(fixture.entry(&listing, ".ssh").protection.is_refused());
        assert!(!fixture.entry(&listing, ".ssh").is_deletable());

        assert!(fixture.entry(&listing, "Documents").protection.needs_confirmation());
        assert!(
            fixture.entry(&listing, "Documents").is_deletable(),
            "personal data is deletable from the explorer, with confirmation"
        );

        assert!(fixture.entry(&listing, "code").protection.is_allowed());
    }

    #[test]
    fn listing_returns_immediately_with_directories_unmeasured() {
        let fixture = Fixture::new();
        let listing = list_directory(
            &fixture.home.path().join("code"),
            &[fixture.home.path().to_path_buf()],
            &PathGuard::new(fixture.home.path()),
            &fixture.cache,
        )
        .expect("list");

        let big = fixture.entry(&listing, "big");
        assert!(!big.measured, "a directory must not block the listing on a walk");
        assert_eq!(big.size, Bytes::ZERO, "unmeasured means unknown, not empty");
    }

    #[test]
    fn plain_files_are_measured_immediately() {
        let fixture = Fixture::new();
        let listing = list_directory(
            fixture.home.path(),
            &[fixture.home.path().to_path_buf()],
            &PathGuard::new(fixture.home.path()),
            &fixture.cache,
        )
        .expect("list");

        let notes = fixture.entry(&listing, "notes.txt");
        assert!(notes.measured, "a file's size is one stat away");
        assert_eq!(notes.size, Bytes(100));
    }

    #[test]
    fn measuring_reports_every_pending_directory_exactly_once() {
        let fixture = Fixture::new();
        let listing = list_directory(
            &fixture.home.path().join("code"),
            &[fixture.home.path().to_path_buf()],
            &PathGuard::new(fixture.home.path()),
            &fixture.cache,
        )
        .expect("list");

        let seen = Mutex::new(Vec::new());
        measure_entries(&listing.entries, &Control::new(), &fixture.cache, |result| {
            seen.lock().expect("lock").push(result);
        });

        let mut seen = seen.into_inner().expect("lock");
        seen.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(seen.len(), 2, "both directories, and nothing twice");
        assert_eq!(seen[0].size, Bytes(10_000));
        assert_eq!(seen[1].size, Bytes(500));
    }

    #[test]
    fn a_cached_directory_needs_no_second_measurement() {
        let fixture = Fixture::new();
        let roots = [fixture.home.path().to_path_buf()];
        let guard = PathGuard::new(fixture.home.path());
        let code = fixture.home.path().join("code");

        // Warm the cache.
        browse(&code, &roots, &guard, &Control::new(), &fixture.cache, SortBy::Size)
            .expect("browse");

        let listing = list_directory(&code, &roots, &guard, &fixture.cache).expect("list");
        assert!(
            listing.entries.iter().all(|entry| entry.measured),
            "revisiting a folder should need no walking at all"
        );

        let seen = Mutex::new(0);
        measure_entries(&listing.entries, &Control::new(), &fixture.cache, |_| {
            *seen.lock().expect("lock") += 1;
        });
        assert_eq!(*seen.lock().expect("lock"), 0);
    }

    #[test]
    fn cancelling_abandons_the_measurement_pass() {
        let fixture = Fixture::new();
        let listing = list_directory(
            &fixture.home.path().join("code"),
            &[fixture.home.path().to_path_buf()],
            &PathGuard::new(fixture.home.path()),
            &fixture.cache,
        )
        .expect("list");

        let control = Control::new();
        control.cancel();

        let seen = Mutex::new(0);
        measure_entries(&listing.entries, &control, &fixture.cache, |_| {
            *seen.lock().expect("lock") += 1;
        });
        assert_eq!(
            *seen.lock().expect("lock"),
            0,
            "navigating away must not keep the old folder's work running"
        );
    }

    #[test]
    fn breadcrumbs_start_at_the_root_and_end_at_the_directory() {
        let fixture = Fixture::new();
        let listing = fixture.list("code/big");
        assert_eq!(listing.breadcrumbs.len(), 3);
        assert_eq!(listing.breadcrumbs[1].name, "code");
        assert_eq!(listing.breadcrumbs[2].name, "big");
        assert_eq!(listing.breadcrumbs[2].path, fixture.home.path().join("code/big"));
    }

    #[test]
    fn navigation_cannot_escape_the_roots() {
        let fixture = Fixture::new();
        assert!(fixture.list("").parent.is_none(), "the root has nowhere to go up to");
        assert_eq!(fixture.list("code").parent.as_deref(), Some(fixture.home.path()));
    }

    #[test]
    fn sorting_by_name_is_case_insensitive() {
        let home = tempfile::tempdir().expect("tempdir");
        write(&home.path().join("Zebra.txt"), 10);
        write(&home.path().join("apple.txt"), 9_000);

        let listing = browse(
            home.path(),
            &[home.path().to_path_buf()],
            &PathGuard::new(home.path()),
            &Control::new(),
            &SizeCache::new(),
            SortBy::Name,
        )
        .expect("browse");
        assert_eq!(listing.entries[0].name, "apple.txt");
    }

    #[test]
    fn the_cache_is_populated_and_reused() {
        let fixture = Fixture::new();
        assert!(fixture.cache.is_empty());
        fixture.list("code");
        assert!(!fixture.cache.is_empty());

        // Change a file behind the cache's back; the cached size is kept,
        // which is the documented behaviour until something is invalidated.
        write(&fixture.home.path().join("code/big/extra.bin"), 50_000);
        assert_eq!(fixture.entry(&fixture.list("code"), "big").size, Bytes(10_000));
    }

    #[test]
    fn invalidating_forgets_the_path_and_its_ancestors() {
        let fixture = Fixture::new();
        fixture.list("code");
        write(&fixture.home.path().join("code/big/extra.bin"), 50_000);

        fixture.cache.invalidate(&fixture.home.path().join("code/big"));
        assert_eq!(fixture.entry(&fixture.list("code"), "big").size, Bytes(60_000));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_listed_but_never_measured_or_deletable() {
        let fixture = Fixture::new();
        std::os::unix::fs::symlink(
            fixture.home.path().join("code"),
            fixture.home.path().join("shortcut"),
        )
        .expect("symlink");

        let listing = fixture.list("");
        let link = fixture.entry(&listing, "shortcut");
        assert!(link.is_symlink);
        assert_eq!(link.size, Bytes::ZERO, "a link must not inherit its target's size");
        assert!(!link.is_deletable());
    }

    #[test]
    fn browsing_a_file_is_an_error_rather_than_an_empty_listing() {
        let fixture = Fixture::new();
        let result = browse(
            &fixture.home.path().join("notes.txt"),
            &[fixture.home.path().to_path_buf()],
            &PathGuard::new(fixture.home.path()),
            &Control::new(),
            &fixture.cache,
            SortBy::Size,
        );
        assert!(result.is_err());
    }

    #[test]
    fn cancellation_stops_a_listing() {
        let fixture = Fixture::new();
        let control = Control::new();
        control.cancel();
        let result = browse(
            fixture.home.path(),
            &[fixture.home.path().to_path_buf()],
            &PathGuard::new(fixture.home.path()),
            &control,
            &fixture.cache,
            SortBy::Size,
        );
        assert!(result.is_err());
    }

    #[test]
    fn an_empty_directory_lists_cleanly() {
        let home = tempfile::tempdir().expect("tempdir");
        let listing = browse(
            home.path(),
            &[home.path().to_path_buf()],
            &PathGuard::new(home.path()),
            &Control::new(),
            &SizeCache::new(),
            SortBy::Size,
        )
        .expect("browse");
        assert!(listing.entries.is_empty());
        assert_eq!(listing.total_size, Bytes::ZERO);
        assert_eq!(listing.hidden_entries, 0);
    }

    #[test]
    fn enormous_directories_are_truncated_and_say_so() {
        let home = tempfile::tempdir().expect("tempdir");
        for i in 0..(MAX_ENTRIES + 25) {
            write(&home.path().join(format!("f{i:05}.bin")), i + 1);
        }
        let listing = browse(
            home.path(),
            &[home.path().to_path_buf()],
            &PathGuard::new(home.path()),
            &Control::new(),
            &SizeCache::new(),
            SortBy::Size,
        )
        .expect("browse");

        assert_eq!(listing.entries.len(), MAX_ENTRIES);
        assert_eq!(listing.hidden_entries, 25);
        // The total still counts everything, including what was dropped.
        assert!(listing.total_size.get() > listing.entries.iter().map(|e| e.size.get()).sum::<u64>());
    }
}
