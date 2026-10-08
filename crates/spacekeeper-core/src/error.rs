//! Error type shared by the whole core crate.

use std::path::PathBuf;

/// Everything that can go wrong inside SpaceKeeper.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The user cancelled the running operation.
    #[error("operation was cancelled")]
    Cancelled,

    /// A scanner could not run because a tool or directory it needs is absent.
    /// This is not a failure: the scanner is simply skipped.
    #[error("scanner `{scanner}` is unavailable: {reason}")]
    ScannerUnavailable {
        /// Identifier of the scanner that was skipped.
        scanner: String,
        /// Human readable reason, shown in the UI diagnostics panel.
        reason: String,
    },

    /// A path was refused because it is protected or outside the allowed roots.
    #[error("refusing to touch protected path: {0}")]
    ProtectedPath(PathBuf),

    /// The caller tried to delete something that is not marked deletable.
    #[error("item is not deletable: {0}")]
    NotDeletable(PathBuf),

    /// A dangerous item was submitted without an explicit confirmation.
    #[error("dangerous item requires explicit confirmation: {0}")]
    ConfirmationRequired(PathBuf),

    /// Filesystem failure, carrying the path for a useful message.
    #[error("io error at {path}: {source}")]
    Io {
        /// Path being operated on when the failure happened.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Plain I/O failure with no meaningful path.
    #[error(transparent)]
    RawIo(#[from] std::io::Error),

    /// Storage failure.
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// Serialisation failure when reading or writing stored settings.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// Moving a file to the OS trash failed.
    #[error("trash error: {0}")]
    Trash(#[from] trash::Error),

    /// Catch-all for scanner implementations.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    /// Attach a path to an [`std::io::Error`].
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Build an [`Error::ScannerUnavailable`].
    pub fn unavailable(scanner: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::ScannerUnavailable {
            scanner: scanner.into(),
            reason: reason.into(),
        }
    }

    /// `true` when the error means "user pressed stop", which callers treat as
    /// a normal outcome rather than a failure.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    /// `true` when a scanner reported that it has nothing to work with on this
    /// machine. Not a failure: a laptop without Docker installed should not
    /// show a red error, it should show nothing.
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::ScannerUnavailable { .. })
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
