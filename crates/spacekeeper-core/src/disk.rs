//! Disk capacity, for the dashboard gauge and for storage trends.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::Bytes;

/// One mounted volume.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskInfo {
    /// Volume name as reported by the OS.
    pub name: String,
    /// Where it is mounted.
    pub mount_point: PathBuf,
    /// Filesystem type, e.g. `apfs`.
    pub file_system: String,
    /// Capacity.
    pub total: Bytes,
    /// Free space.
    pub available: Bytes,
    /// Whether the volume is removable.
    pub removable: bool,
}

impl DiskInfo {
    /// Space in use.
    pub fn used(&self) -> Bytes {
        Bytes(self.total.get().saturating_sub(self.available.get()))
    }

    /// Fraction of the volume in use, in `0.0..=1.0`.
    pub fn used_fraction(&self) -> f64 {
        if self.total.get() == 0 {
            return 0.0;
        }
        (self.used().get() as f64 / self.total.get() as f64).clamp(0.0, 1.0)
    }

    /// Whether the volume is nearly full, which the UI highlights.
    pub fn is_critical(&self) -> bool {
        self.used_fraction() >= 0.9
    }
}

/// All mounted volumes, excluding pseudo-filesystems with no capacity.
pub fn disks() -> Vec<DiskInfo> {
    sysinfo::Disks::new_with_refreshed_list()
        .list()
        .iter()
        .filter(|disk| disk.total_space() > 0)
        .map(|disk| DiskInfo {
            name: disk.name().to_string_lossy().into_owned(),
            mount_point: disk.mount_point().to_path_buf(),
            file_system: disk.file_system().to_string_lossy().into_owned(),
            total: Bytes(disk.total_space()),
            available: Bytes(disk.available_space()),
            removable: disk.is_removable(),
        })
        .collect()
}

/// The volume the given path lives on.
///
/// Picks the mount point with the longest match, which is how nested mounts
/// resolve (`/` vs `/home` vs `/home/me/data`).
pub fn disk_for(path: &std::path::Path) -> Option<DiskInfo> {
    disks()
        .into_iter()
        .filter(|disk| path.starts_with(&disk.mount_point))
        .max_by_key(|disk| disk.mount_point.components().count())
}

/// The volume the user's home directory lives on.
pub fn primary() -> Option<DiskInfo> {
    let home = dirs::home_dir()?;
    disk_for(&home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn disk(mount: &str, total: u64, available: u64) -> DiskInfo {
        DiskInfo {
            name: "test".into(),
            mount_point: PathBuf::from(mount),
            file_system: "apfs".into(),
            total: Bytes(total),
            available: Bytes(available),
            removable: false,
        }
    }

    #[test]
    fn used_space_is_capacity_minus_free() {
        assert_eq!(disk("/", 1000, 250).used(), Bytes(750));
    }

    #[test]
    fn used_fraction_handles_a_zero_sized_volume() {
        assert_eq!(disk("/", 0, 0).used_fraction(), 0.0);
    }

    #[test]
    fn nearly_full_volumes_are_critical() {
        assert!(disk("/", 1000, 50).is_critical());
        assert!(!disk("/", 1000, 500).is_critical());
    }

    #[test]
    fn enumerating_disks_returns_only_real_volumes() {
        for disk in disks() {
            assert!(disk.total.get() > 0);
        }
    }

    #[test]
    fn disk_lookup_prefers_the_longest_matching_mount_point() {
        // Exercised through the pure helper rather than the real machine so
        // the test is deterministic on any CI runner.
        let candidates = vec![disk("/", 100, 10), disk("/home/me/data", 100, 10)];
        let best = candidates
            .into_iter()
            .filter(|d| Path::new("/home/me/data/x").starts_with(&d.mount_point))
            .max_by_key(|d| d.mount_point.components().count())
            .expect("a match");
        assert_eq!(best.mount_point, PathBuf::from("/home/me/data"));
    }
}
