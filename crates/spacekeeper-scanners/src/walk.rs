//! Shared traversal used by the scanners that search rather than look up a
//! known path.
//!
//! Centralising this means "never descend into a protected folder" and "check
//! for cancellation" are written once. A scanner that walks the disk by hand is
//! a scanner that can forget one of them.

use std::path::{Path, PathBuf};

use spacekeeper_core::{Control, PathGuard, Result};
use walkdir::{DirEntry, WalkDir};

/// Check the control handle every this many entries.
const CHECK_INTERVAL: u64 = 512;

/// What the walker should do with a directory it is about to descend into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Descend {
    /// Walk into it.
    Yes,
    /// Report it (if the callback wants) but do not walk inside.
    No,
}

/// Walk `roots`, calling `visit` for every entry that survives the filters.
///
/// * protected directories are never entered;
/// * hidden version-control and system directories are skipped, because
///   nothing worth cleaning lives in `.git` and walking it is expensive;
/// * `prune` lets a scanner stop at a boundary — `node_modules` is reported as
///   a unit, not as forty thousand files.
pub fn walk<F, P>(
    roots: &[PathBuf],
    guard: &PathGuard,
    control: &Control,
    max_depth: usize,
    mut prune: P,
    mut visit: F,
) -> Result<()>
where
    P: FnMut(&DirEntry) -> Descend,
    F: FnMut(&DirEntry),
{
    let mut seen: u64 = 0;

    for root in roots {
        let mut walker = WalkDir::new(root)
            .follow_links(false)
            .max_depth(max_depth)
            .into_iter();

        loop {
            let entry = match walker.next() {
                None => break,
                Some(Ok(entry)) => entry,
                // Unreadable entries are skipped: one permission error must not
                // cost the user the rest of the scan.
                Some(Err(_)) => continue,
            };

            seen += 1;
            if seen % CHECK_INTERVAL == 0 {
                control.check()?;
            }

            if entry.file_type().is_symlink() {
                continue;
            }

            if entry.file_type().is_dir() && entry.depth() > 0 {
                if guard.is_protected(entry.path()) || is_uninteresting_dir(entry.path()) {
                    walker.skip_current_dir();
                    continue;
                }
                if prune(&entry) == Descend::No {
                    visit(&entry);
                    walker.skip_current_dir();
                    continue;
                }
            }

            visit(&entry);
        }
    }

    control.check()
}

/// Directories that never contain anything worth reporting, and that are
/// expensive to walk.
fn is_uninteresting_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some(".git" | ".hg" | ".svn" | ".Trash" | "System Volume Information" | "$RECYCLE.BIN")
    )
}

/// Whether a name starts with a dot.
pub fn is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .is_some_and(|name| name.starts_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("tempdir");
        for path in [
            "code/app/src/main.rs",
            "code/app/node_modules/left-pad/index.js",
            "code/app/.git/objects/ab/cdef",
            ".ssh/id_rsa",
            "Downloads/big.zip",
        ] {
            let full = root.path().join(path);
            fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
            fs::write(&full, b"x").expect("write");
        }
        root
    }

    fn collect(root: &Path, prune: impl FnMut(&DirEntry) -> Descend) -> Vec<PathBuf> {
        let guard = PathGuard::new(root);
        let mut found = Vec::new();
        walk(
            &[root.to_path_buf()],
            &guard,
            &Control::new(),
            20,
            prune,
            |entry| found.push(entry.path().to_path_buf()),
        )
        .expect("walk");
        found
    }

    #[test]
    fn visits_ordinary_files() {
        let root = setup();
        let found = collect(root.path(), |_| Descend::Yes);
        assert!(found.iter().any(|p| p.ends_with("code/app/src/main.rs")));
    }

    #[test]
    fn never_enters_protected_directories() {
        let root = setup();
        let found = collect(root.path(), |_| Descend::Yes);
        assert!(
            !found.iter().any(|p| p.to_string_lossy().contains(".ssh")),
            "the walker must not read protected folders at all"
        );
    }

    #[test]
    fn skips_version_control_directories() {
        let root = setup();
        let found = collect(root.path(), |_| Descend::Yes);
        assert!(!found.iter().any(|p| p.to_string_lossy().contains(".git")));
    }

    #[test]
    fn pruning_reports_the_boundary_without_descending() {
        let root = setup();
        let found = collect(root.path(), |entry| {
            if entry.file_name() == "node_modules" {
                Descend::No
            } else {
                Descend::Yes
            }
        });
        assert!(found.iter().any(|p| p.ends_with("node_modules")));
        assert!(
            !found
                .iter()
                .any(|p| p.to_string_lossy().contains("left-pad")),
            "pruned directories must not be walked"
        );
    }

    #[test]
    fn cancellation_stops_the_walk() {
        let root = tempfile::tempdir().expect("tempdir");
        for i in 0..2000 {
            let path = root.path().join(format!("dir{}/f.bin", i % 50));
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            fs::write(&path, b"x").expect("write");
        }
        let control = Control::new();
        control.cancel();
        let outcome = walk(
            &[root.path().to_path_buf()],
            &PathGuard::new(root.path()),
            &control,
            20,
            |_| Descend::Yes,
            |_| {},
        );
        assert!(outcome.is_err());
    }

    #[test]
    fn max_depth_is_respected() {
        let root = setup();
        let guard = PathGuard::new(root.path());
        let mut found = Vec::new();
        walk(
            &[root.path().to_path_buf()],
            &guard,
            &Control::new(),
            1,
            |_| Descend::Yes,
            |entry| found.push(entry.path().to_path_buf()),
        )
        .expect("walk");
        assert!(!found.iter().any(|p| p.ends_with("main.rs")));
    }

    #[test]
    fn a_missing_root_is_not_an_error() {
        let guard = PathGuard::new(Path::new("/home/tester"));
        let outcome = walk(
            &[PathBuf::from("/definitely/not/here")],
            &guard,
            &Control::new(),
            5,
            |_| Descend::Yes,
            |_| {},
        );
        assert!(outcome.is_ok());
    }
}
