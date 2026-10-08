//! # SpaceKeeper scanners
//!
//! Every scanner SpaceKeeper ships with, and the pieces third-party scanners
//! are built from.
//!
//! Scanners come in two shapes:
//!
//! * **declarative** — a [`rules::PathRule`] entry in [`catalog::CATALOG`].
//!   Use this whenever the answer is "look in this well-known folder". Adding
//!   one is a data change: no new code, and the traversal, measurement,
//!   cancellation and safety behaviour comes for free.
//! * **bespoke** — a type implementing [`spacekeeper_core::Scanner`], for the
//!   cases that need real logic: finding duplicates, dating a project by its
//!   manifest, asking the Docker daemon.
//!
//! ```no_run
//! use spacekeeper_core::{ScanContext, ScanEngine, ScannerRegistry};
//!
//! # async fn example() {
//! let mut registry = ScannerRegistry::new();
//! registry.register_all(spacekeeper_scanners::builtin());
//!
//! let report = ScanEngine::new(registry).run(&ScanContext::for_current_user(), None).await;
//! # }
//! ```

#![warn(missing_docs)]

pub mod catalog;
pub mod docker;
pub mod downloads;
pub mod duplicates;
pub mod empty_dirs;
pub mod large_files;
pub mod node_modules;
pub mod rules;
pub mod walk;

use std::sync::Arc;

use spacekeeper_core::Scanner;

/// Every scanner SpaceKeeper ships with.
///
/// This is the composition root for the built-ins. The application calls it
/// once at startup and can register anything else afterwards; nothing here is
/// privileged over a scanner a plugin registers.
pub fn builtin() -> Vec<Arc<dyn Scanner>> {
    let mut scanners: Vec<Arc<dyn Scanner>> = catalog::CATALOG
        .iter()
        .map(|rule| Arc::new(rule.scanner()) as Arc<dyn Scanner>)
        .collect();

    scanners.extend([
        Arc::new(downloads::StaleFolderScanner::downloads()) as Arc<dyn Scanner>,
        Arc::new(large_files::LargeFileScanner) as Arc<dyn Scanner>,
        Arc::new(node_modules::NodeModulesScanner),
        Arc::new(duplicates::DuplicateScanner),
        Arc::new(empty_dirs::EmptyFolderScanner),
        Arc::new(docker::DockerScanner),
    ]);

    scanners
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_builtin_scanner_has_a_unique_id() {
        let mut seen = HashSet::new();
        for scanner in builtin() {
            let id = scanner.metadata().id;
            assert!(seen.insert(id.clone()), "duplicate scanner id: {id}");
        }
    }

    #[test]
    fn the_catalog_and_bespoke_scanners_are_all_registered() {
        assert_eq!(builtin().len(), catalog::CATALOG.len() + 6);
    }

    #[test]
    fn every_scanner_describes_itself_for_a_non_technical_user() {
        for scanner in builtin() {
            let meta = scanner.metadata();
            assert!(!meta.name.is_empty(), "{} has no name", meta.id);
            assert!(
                !meta.description.is_empty(),
                "{} has no description",
                meta.id
            );
            assert!(
                meta.explanation.len() > 40,
                "{} must explain what its findings actually are",
                meta.id
            );
        }
    }

    #[test]
    fn every_scanner_supports_at_least_one_platform() {
        for scanner in builtin() {
            let meta = scanner.metadata();
            assert!(
                !meta.platforms.is_empty(),
                "{} supports no platforms",
                meta.id
            );
        }
    }

    #[test]
    fn the_registry_accepts_every_builtin() {
        let mut registry = spacekeeper_core::ScannerRegistry::new();
        registry.register_all(builtin());
        assert_eq!(registry.len(), builtin().len());
    }

    #[tokio::test]
    async fn a_full_scan_of_an_empty_home_finds_nothing_and_fails_nothing() {
        let home = tempfile::tempdir().expect("tempdir");
        let mut registry = spacekeeper_core::ScannerRegistry::new();
        registry.register_all(builtin());

        let context = spacekeeper_core::ScanContext::new(home.path().to_path_buf());
        let report = spacekeeper_core::ScanEngine::new(registry)
            .run(&context, None)
            .await;

        assert!(report.results.is_empty());
        assert!(
            report
                .scanners
                .iter()
                .all(|outcome| outcome.status != spacekeeper_core::ScannerStatus::Failed),
            "no scanner may fail on an empty home directory"
        );
    }
}
