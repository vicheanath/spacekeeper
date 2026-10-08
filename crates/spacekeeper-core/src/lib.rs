//! # SpaceKeeper core
//!
//! Types and engines shared by the desktop app, the CLI and any plugin.
//!
//! The crate is organised around one flow:
//!
//! ```text
//!   ScannerRegistry ──▶ ScanEngine ──▶ ScanReport ──▶ CleanupEngine ──▶ CleanupRecord
//!                            │                              │
//!                       recommend::rank              protect::PathGuard
//! ```
//!
//! Two rules hold everything together:
//!
//! * a [`scanner::Scanner`] *proposes*; [`recommend::apply_policy`] and
//!   [`protect::PathGuard`] *decide*. A scanner — including a third-party one —
//!   can never widen what SpaceKeeper is willing to delete;
//! * nothing is deleted without an explicit request naming the exact paths.
//!   There is no "clean everything" path through the code.
//!
//! ## Example
//!
//! ```no_run
//! # async fn example() -> spacekeeper_core::Result<()> {
//! use spacekeeper_core::{ScanContext, ScanEngine, ScannerRegistry};
//!
//! let registry = ScannerRegistry::new(); // register scanners here
//! let engine = ScanEngine::new(registry);
//! let report = engine.run(&ScanContext::for_current_user(), None).await;
//!
//! println!("recoverable: {}", report.summary.safe_size);
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

pub mod browse;
pub mod cleanup;
pub mod control;
pub mod db;
pub mod disk;
pub mod error;
pub mod explain;
pub mod fsutil;
pub mod progress;
pub mod protect;
pub mod recommend;
pub mod scan;
pub mod scanner;
pub mod types;

pub use control::Control;
pub use error::{Error, Result};
pub use progress::{Phase, ProgressReporter, ScanProgress, ScannerProgress};
pub use browse::{BrowseEntry, DirectoryListing, SizeCache, SortBy};
pub use protect::{PathGuard, Protection};
pub use scan::{ScanEngine, ScanReport, ScannerOutcome, ScannerStatus};
pub use scanner::{ScanContext, ScanOptions, Scanner, ScannerRegistry};
pub use types::{
    Bytes, Category, EntryKind, Family, RiskLevel, ScanResult, ScanResultBuilder, ScannerMetadata,
};
