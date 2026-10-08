//! The list of things SpaceKeeper will never delete, and the things it will
//! only delete if you insist.
//!
//! Every deletion goes through [`PathGuard`], including deletions requested by
//! third-party scanners and by the file explorer. A scanner cannot opt out:
//! the guard is applied by the cleanup engine, not by whatever produced the
//! path.
//!
//! # Three tiers
//!
//! [`Protection::Refused`] is absolute — system files, credentials, installed
//! applications, volume roots. Nothing in the product can delete these.
//!
//! [`Protection::Caution`] is for irreplaceable *personal* data: your
//! documents, photos, iCloud Drive, a repository's `.git`. Automated cleanup
//! never touches it, but the file explorer may, because a user looking at a
//! 40 GB video they recorded should be able to delete it. It costs a second,
//! explicit confirmation.
//!
//! [`Protection::Allowed`] is everything else.
//!
//! The split exists because a single boolean forced a bad trade: refuse
//! `~/Documents` outright and the explorer is useless there; allow it and a
//! scanner bug can eat someone's thesis. Tiering lets automation stay
//! paranoid while the explorer stays useful.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// How protected a path is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Protection {
    /// Ordinary path. Normal rules apply.
    Allowed,
    /// Irreplaceable personal data. Automated cleanup refuses it; the explorer
    /// allows it after an explicit confirmation.
    #[serde(rename_all = "camelCase")]
    Caution {
        /// Plain-language reason, shown in the UI.
        reason: String,
    },
    /// Never deletable by any part of SpaceKeeper.
    #[serde(rename_all = "camelCase")]
    Refused {
        /// Plain-language reason, shown in the UI.
        reason: String,
    },
}

impl Protection {
    /// Whether normal, automated cleanup may remove this.
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// Whether *any* part of SpaceKeeper may remove this.
    pub fn is_refused(&self) -> bool {
        matches!(self, Self::Refused { .. })
    }

    /// Whether the explorer may remove it after explicit confirmation.
    pub fn needs_confirmation(&self) -> bool {
        matches!(self, Self::Caution { .. })
    }

    /// The reason, when there is one.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Allowed => None,
            Self::Caution { reason } | Self::Refused { reason } => Some(reason),
        }
    }

    fn refused(reason: impl Into<String>) -> Self {
        Self::Refused { reason: reason.into() }
    }

    fn caution(reason: impl Into<String>) -> Self {
        Self::Caution { reason: reason.into() }
    }
}

/// A protected location and the reason it is protected.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedEntry {
    /// The path.
    pub path: PathBuf,
    /// Why it is protected, in plain language.
    pub reason: String,
    /// Whether it is absolutely refused or merely cautioned.
    pub refused: bool,
}

/// Decides whether a path may be deleted.
#[derive(Debug, Clone)]
pub struct PathGuard {
    /// Subtrees that are absolutely off limits.
    refused: Vec<(PathBuf, &'static str)>,
    /// Subtrees holding irreplaceable personal data.
    caution: Vec<(PathBuf, &'static str)>,
    /// Paths protected only as themselves, not as subtrees. The home directory
    /// belongs here: `~` must never be deleted, but almost everything we clean
    /// lives inside it.
    refused_exact: Vec<PathBuf>,
    /// Folders the user asked SpaceKeeper to leave alone.
    user_ignored: Vec<PathBuf>,
}

impl PathGuard {
    /// Build a guard for the given home directory.
    pub fn new(home: &Path) -> Self {
        Self {
            refused: platform_refused()
                .into_iter()
                .chain(home_refused(home))
                .collect(),
            caution: home_caution(home),
            refused_exact: vec![home.to_path_buf()],
            user_ignored: Vec::new(),
        }
    }

    /// Build a guard for the current user's home directory.
    pub fn for_current_user() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        Self::new(&home)
    }

    /// Add folders the user asked SpaceKeeper to leave alone.
    pub fn with_user_ignored(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.user_ignored.extend(paths);
        self
    }

    /// Everything protected, for the settings screen.
    pub fn protected_entries(&self) -> Vec<ProtectedEntry> {
        let mut entries: Vec<ProtectedEntry> = self
            .refused
            .iter()
            .map(|(path, reason)| ProtectedEntry {
                path: path.clone(),
                reason: (*reason).to_string(),
                refused: true,
            })
            .chain(self.caution.iter().map(|(path, reason)| ProtectedEntry {
                path: path.clone(),
                reason: (*reason).to_string(),
                refused: false,
            }))
            .collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries
    }

    /// Just the paths, for callers that do not need the reasons.
    pub fn protected_paths(&self) -> Vec<PathBuf> {
        self.protected_entries().into_iter().map(|entry| entry.path).collect()
    }

    /// Classify a path.
    ///
    /// The order matters: structural refusals (traversal, shallowness, volume
    /// roots) come before list lookups, because a path containing `..` could
    /// otherwise slip past every prefix comparison below it.
    pub fn classify(&self, path: &Path) -> Protection {
        if !path.is_absolute() {
            return Protection::refused("only absolute paths can be removed");
        }
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Protection::refused("paths containing `..` are never followed");
        }
        if Self::is_too_shallow(path) {
            return Protection::refused("this is a top-level system location");
        }
        if Self::is_volume_root(path) {
            return Protection::refused("this is the root of a disk, not a folder on it");
        }
        // A folder with another disk mounted inside it looks completely
        // ordinary. Deleting it would empty that disk.
        if crate::fsutil::is_mount_point(path) {
            return Protection::refused("another disk is mounted here");
        }
        if Self::is_application_bundle(path) {
            return Protection::refused("this is an installed application");
        }
        if self.user_ignored.iter().any(|root| path.starts_with(root)) {
            return Protection::refused("you asked SpaceKeeper to ignore this folder");
        }
        if self.refused_exact.iter().any(|root| root == path) {
            return Protection::refused("this is your home folder");
        }
        if let Some((_, reason)) = self.refused.iter().find(|(root, _)| path.starts_with(root)) {
            return Protection::refused(*reason);
        }
        // Deleting an ancestor would take a protected location with it.
        if let Some((root, _)) = self
            .refused
            .iter()
            .map(|(root, reason)| (root, reason))
            .chain(self.refused_exact.iter().map(|root| (root, &"")))
            .find(|(root, _)| root.starts_with(path) && *root != path)
        {
            return Protection::refused(format!(
                "removing this would also remove {}",
                root.display()
            ));
        }
        if let Some((_, reason)) = self.caution.iter().find(|(root, _)| path.starts_with(root)) {
            return Protection::caution(*reason);
        }
        if Self::is_repository_metadata(path) {
            return Protection::caution(
                "this holds a project's entire version history, which cannot be recovered",
            );
        }
        Protection::Allowed
    }

    /// Whether `path` is protected at all — either tier.
    pub fn is_protected(&self, path: &Path) -> bool {
        !self.classify(path).is_allowed()
    }

    /// Whether deleting `path` would take a refused location down with it.
    pub fn contains_protected(&self, path: &Path) -> bool {
        self.refused
            .iter()
            .map(|(root, _)| root)
            .chain(self.caution.iter().map(|(root, _)| root))
            .chain(self.refused_exact.iter())
            .any(|root| root.starts_with(path) && root != path)
    }

    /// Whether the path is dangerously shallow — a filesystem root, a drive
    /// letter, or a direct child of the root such as `/usr`.
    pub fn is_too_shallow(path: &Path) -> bool {
        path.components().filter(|c| matches!(c, Component::Normal(_))).count() < 2
    }

    /// Whether the path is the mount point of a volume rather than a folder on
    /// one. Deleting `/Volumes/Backup` means deleting an entire disk's contents.
    pub fn is_volume_root(path: &Path) -> bool {
        for parent in ["/Volumes", "/media", "/mnt", "/run/media"] {
            if path.parent() == Some(Path::new(parent)) {
                return true;
            }
        }
        false
    }

    /// Whether the path is a macOS bundle that represents installed software.
    pub fn is_application_bundle(path: &Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("app" | "framework" | "kext" | "bundle" | "xpc")
        )
    }

    /// Whether the path is a version-control directory.
    pub fn is_repository_metadata(path: &Path) -> bool {
        matches!(path.file_name().and_then(|n| n.to_str()), Some(".git" | ".hg" | ".svn"))
    }

    /// The check used before any *automated* deletion.
    ///
    /// Fails for both protected tiers: automation never touches personal data,
    /// even the kind the explorer would allow after a confirmation.
    pub fn check(&self, path: &Path) -> Result<()> {
        match self.classify(path) {
            Protection::Allowed => Ok(()),
            _ => Err(Error::ProtectedPath(path.to_path_buf())),
        }
    }

    /// The check used by the file explorer, where the user is looking directly
    /// at the thing and has confirmed it.
    ///
    /// Still refuses everything in [`Protection::Refused`].
    pub fn check_confirmed(&self, path: &Path) -> Result<()> {
        match self.classify(path) {
            Protection::Refused { .. } => Err(Error::ProtectedPath(path.to_path_buf())),
            _ => Ok(()),
        }
    }
}

/// System locations that are off limits, with the reason shown to the user.
fn platform_refused() -> Vec<(PathBuf, &'static str)> {
    let raw: &[(&str, &str)] = if cfg!(target_os = "macos") {
        &[
            ("/System", "part of macOS"),
            ("/Library", "shared system files used by every account"),
            ("/Applications", "installed applications"),
            ("/usr", "part of the operating system"),
            ("/bin", "part of the operating system"),
            ("/sbin", "part of the operating system"),
            ("/etc", "system configuration"),
            ("/private/etc", "system configuration"),
            ("/private/var/db", "system databases"),
            ("/cores", "system crash data"),
            ("/Network", "network mounts, not local files"),
            ("/opt/homebrew/Cellar", "installed Homebrew software"),
        ]
    } else if cfg!(target_os = "windows") {
        &[
            (r"C:\Windows", "part of Windows"),
            (r"C:\Program Files", "installed applications"),
            (r"C:\Program Files (x86)", "installed applications"),
            (r"C:\ProgramData\Microsoft", "system data"),
            (r"C:\Recovery", "Windows recovery files"),
            (r"C:\System Volume Information", "system restore data"),
            (r"C:\Users\Default", "the template for new user accounts"),
        ]
    } else {
        &[
            ("/bin", "part of the operating system"),
            ("/boot", "needed to start the computer"),
            ("/dev", "device files, not real files"),
            ("/efi", "needed to start the computer"),
            ("/etc", "system configuration"),
            ("/lib", "part of the operating system"),
            ("/lib64", "part of the operating system"),
            ("/opt", "installed software"),
            ("/proc", "kernel data, not real files"),
            ("/root", "the administrator's home folder"),
            ("/run", "runtime system state"),
            ("/sbin", "part of the operating system"),
            ("/snap", "installed Snap applications"),
            ("/srv", "server data"),
            ("/sys", "kernel data, not real files"),
            ("/usr", "part of the operating system"),
            ("/var/lib", "system and application state"),
        ]
    };
    raw.iter().map(|(path, reason)| (PathBuf::from(path), *reason)).collect()
}

/// Locations inside the home directory that are absolutely off limits.
fn home_refused(home: &Path) -> Vec<(PathBuf, &'static str)> {
    const ENTRIES: &[(&str, &str)] = &[
        (".ssh", "the keys you use to sign in to other machines"),
        (".gnupg", "your encryption keys"),
        (".aws", "cloud credentials"),
        (".kube", "cluster credentials"),
        (".docker/config.json", "registry credentials"),
        (".password-store", "your password store"),
        (".config", "settings for the applications you use"),
        (".local/share/keyrings", "saved passwords"),
        ("Library/Keychains", "your saved passwords"),
        ("Library/Preferences", "settings for the applications you use"),
        ("Library/Safari", "your Safari bookmarks and history"),
        ("Library/Application Support/MobileSync", "backups of your iPhone or iPad"),
        ("Applications", "installed applications"),
        ("AppData/Roaming", "settings and data for your applications"),
        ("AppData/Local/Microsoft/Crypto", "encryption keys"),
    ];
    ENTRIES.iter().map(|(rel, reason)| (home.join(rel), *reason)).collect()
}

/// Personal data: never touched by automation, removable from the explorer
/// after an explicit confirmation.
fn home_caution(home: &Path) -> Vec<(PathBuf, &'static str)> {
    const ENTRIES: &[(&str, &str)] = &[
        ("Documents", "your documents"),
        ("Pictures", "your photos"),
        ("Movies", "your videos"),
        ("Videos", "your videos"),
        ("Music", "your music"),
        ("Desktop", "files you keep on your desktop"),
        ("Library/Mobile Documents", "your iCloud Drive"),
        ("Library/CloudStorage", "files synced from cloud storage"),
        ("Library/Messages", "your message history"),
        ("Library/Mail", "your email"),
        ("OneDrive", "files synced with OneDrive"),
        ("Dropbox", "files synced with Dropbox"),
        ("Google Drive", "files synced with Google Drive"),
    ];
    ENTRIES.iter().map(|(rel, reason)| (home.join(rel), *reason)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> PathGuard {
        PathGuard::new(Path::new("/home/tester"))
    }

    #[test]
    fn home_itself_is_refused() {
        assert!(guard().classify(Path::new("/home/tester")).is_refused());
    }

    #[test]
    fn credentials_are_refused_outright() {
        let guard = guard();
        for path in ["/home/tester/.ssh", "/home/tester/.ssh/id_ed25519", "/home/tester/.aws/credentials"] {
            let protection = guard.classify(Path::new(path));
            assert!(protection.is_refused(), "{path} was not refused");
            assert!(protection.reason().is_some(), "{path} has no explanation");
        }
    }

    #[test]
    fn credentials_stay_refused_even_when_confirmed() {
        assert!(guard().check_confirmed(Path::new("/home/tester/.ssh/id_ed25519")).is_err());
    }

    #[test]
    fn documents_are_caution_not_refusal() {
        let protection = guard().classify(Path::new("/home/tester/Documents/taxes.pdf"));
        assert!(protection.needs_confirmation(), "got {protection:?}");
        assert_eq!(protection.reason(), Some("your documents"));
    }

    #[test]
    fn automation_refuses_personal_data_but_the_explorer_may_remove_it() {
        let guard = guard();
        let photo = Path::new("/home/tester/Pictures/2019/holiday.mov");
        assert!(guard.check(photo).is_err(), "automated cleanup must not touch photos");
        assert!(guard.check_confirmed(photo).is_ok(), "the explorer may, once confirmed");
    }

    #[test]
    fn ordinary_cache_paths_are_allowed() {
        let guard = guard();
        assert!(guard.check(Path::new("/home/tester/.cache/pip")).is_ok());
        assert!(guard.check(Path::new("/home/tester/code/api/node_modules")).is_ok());
    }

    #[test]
    fn deleting_an_ancestor_of_a_protected_path_is_refused() {
        let protection = guard().classify(Path::new("/home"));
        assert!(protection.is_refused());
        assert!(guard().contains_protected(Path::new("/home")));
    }

    #[test]
    fn shallow_paths_are_refused() {
        assert!(PathGuard::is_too_shallow(Path::new("/")));
        assert!(PathGuard::is_too_shallow(Path::new("/usr")));
        assert!(!PathGuard::is_too_shallow(Path::new("/usr/local")));
    }

    #[test]
    fn volume_roots_are_refused() {
        assert!(PathGuard::is_volume_root(Path::new("/Volumes/Backup")));
        assert!(PathGuard::is_volume_root(Path::new("/media/usb")));
        assert!(!PathGuard::is_volume_root(Path::new("/Volumes/Backup/old-photos")));
        assert!(guard().classify(Path::new("/Volumes/Backup")).is_refused());
        // A folder *on* an external disk is fair game.
        assert!(guard().check(Path::new("/Volumes/Backup/caches")).is_ok());
    }

    #[test]
    fn application_bundles_are_refused() {
        assert!(guard().classify(Path::new("/home/tester/Downloads/Thing.app")).is_refused());
        assert!(guard().classify(Path::new("/home/tester/code/My.framework")).is_refused());
        assert!(guard().check(Path::new("/home/tester/code/notes.appendix")).is_ok());
    }

    #[test]
    fn git_directories_need_confirmation() {
        let protection = guard().classify(Path::new("/home/tester/code/api/.git"));
        assert!(protection.needs_confirmation(), "got {protection:?}");
        assert!(guard().check(Path::new("/home/tester/code/api/.git")).is_err());
    }

    #[test]
    fn relative_and_traversing_paths_are_refused() {
        let guard = guard();
        assert!(guard.classify(Path::new("relative/path")).is_refused());
        assert!(guard
            .classify(Path::new("/home/tester/code/../../etc/passwd"))
            .is_refused());
    }

    #[test]
    fn user_ignored_folders_are_refused_with_a_clear_reason() {
        let guard = guard().with_user_ignored([PathBuf::from("/home/tester/code/keepme")]);
        let protection = guard.classify(Path::new("/home/tester/code/keepme/node_modules"));
        assert!(protection.is_refused());
        assert_eq!(protection.reason(), Some("you asked SpaceKeeper to ignore this folder"));
        assert!(guard.check(Path::new("/home/tester/code/other/node_modules")).is_ok());
    }

    #[test]
    fn an_ignored_folder_is_refused_even_when_confirmed() {
        let guard = guard().with_user_ignored([PathBuf::from("/home/tester/keep")]);
        assert!(guard.check_confirmed(Path::new("/home/tester/keep/x")).is_err());
    }

    #[test]
    fn system_directories_are_refused() {
        let guard = guard();
        #[cfg(not(target_os = "windows"))]
        assert!(guard.classify(Path::new("/etc/passwd")).is_refused());
        #[cfg(target_os = "windows")]
        assert!(guard.classify(Path::new(r"C:\Windows\System32\cmd.exe")).is_refused());
    }

    #[test]
    fn every_protected_entry_explains_itself() {
        for entry in guard().protected_entries() {
            assert!(
                entry.reason.len() > 8,
                "{} has no usable explanation",
                entry.path.display()
            );
        }
    }

    #[test]
    fn protected_entries_cover_both_tiers() {
        let entries = guard().protected_entries();
        assert!(entries.iter().any(|e| e.refused));
        assert!(entries.iter().any(|e| !e.refused));
    }
}
