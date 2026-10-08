//! Progress reporting from a running scan to the UI.
//!
//! Events are fire-and-forget. Losing one must never affect the scan, so the
//! channel is unbounded and send failures are ignored — the alternative is a
//! scan that stalls because a UI window stopped draining events.

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::types::Bytes;

/// Which stage of the pipeline a progress event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// A scanner has been picked up and is about to run.
    Started,
    /// A scanner is walking the filesystem.
    Scanning,
    /// A scanner finished successfully.
    Finished,
    /// A scanner was skipped because it is unavailable on this machine.
    Skipped,
    /// A scanner failed. The rest of the scan continues.
    Failed,
}

impl Phase {
    /// Whether the scanner will emit no further events.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Finished | Self::Skipped | Self::Failed)
    }
}

/// One progress update. Emitted frequently, so it stays small and `Clone`.
///
/// The number of *completed* scanners is deliberately not carried here: the
/// consumer counts terminal events itself, which keeps a single source of
/// truth and stops the progress bar from jumping backwards when events from
/// concurrent scanners interleave.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    /// Id of the scanner this update is about.
    pub scanner: String,
    /// Stage the scanner is in.
    pub phase: Phase,
    /// Files inspected so far by this scanner.
    pub files_scanned: u64,
    /// Bytes of cleanup opportunity found so far by this scanner.
    pub bytes_found: Bytes,
    /// Path currently being inspected, for the "scanning …" line.
    pub current_path: Option<String>,
    /// Total scanners in this run.
    pub scanners_total: usize,
    /// Explanation when the phase is `Skipped` or `Failed`.
    pub message: Option<String>,
}

/// Where progress events go.
///
/// Defaults to a sink that discards everything, so tests and CLI callers do not
/// have to wire up a channel.
#[derive(Debug, Clone, Default)]
pub struct ProgressReporter {
    sender: Option<UnboundedSender<ScanProgress>>,
}

impl ProgressReporter {
    /// A reporter that drops every event.
    pub fn noop() -> Self {
        Self::default()
    }

    /// A reporter that forwards events to `sender`.
    pub fn channel(sender: UnboundedSender<ScanProgress>) -> Self {
        Self {
            sender: Some(sender),
        }
    }

    /// Send an event, ignoring a dropped receiver.
    pub fn send(&self, progress: ScanProgress) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(progress);
        }
    }
}

/// Per-scanner emitter, so scanners report progress without boilerplate.
///
/// A scanner obtains one from [`crate::scanner::ScanContext::progress_for`].
#[derive(Debug, Clone)]
pub struct ScannerProgress {
    reporter: ProgressReporter,
    scanner: String,
    total: usize,
}

impl ScannerProgress {
    /// Create an emitter for `scanner` in a run of `total` scanners.
    pub fn new(reporter: ProgressReporter, scanner: impl Into<String>, total: usize) -> Self {
        Self {
            reporter,
            scanner: scanner.into(),
            total,
        }
    }

    /// Report that work is ongoing.
    pub fn scanning(&self, files: u64, bytes: Bytes, path: Option<String>) {
        self.emit(Phase::Scanning, files, bytes, path, None);
    }

    /// Emit an event for an arbitrary phase.
    pub fn emit(
        &self,
        phase: Phase,
        files: u64,
        bytes: Bytes,
        path: Option<String>,
        message: Option<String>,
    ) {
        self.reporter.send(ScanProgress {
            scanner: self.scanner.clone(),
            phase,
            files_scanned: files,
            bytes_found: bytes,
            current_path: path,
            scanners_total: self.total,
            message,
        });
    }

    /// Emit a terminal event carrying an explanation.
    pub fn emit_message(&self, phase: Phase, message: impl Into<String>) {
        self.emit(phase, 0, Bytes::ZERO, None, Some(message.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> ScanProgress {
        ScanProgress {
            scanner: "cache".into(),
            phase: Phase::Scanning,
            files_scanned: 0,
            bytes_found: Bytes::ZERO,
            current_path: None,
            scanners_total: 2,
            message: None,
        }
    }

    #[test]
    fn terminal_phases_are_identified() {
        assert!(Phase::Finished.is_terminal());
        assert!(Phase::Skipped.is_terminal());
        assert!(Phase::Failed.is_terminal());
        assert!(!Phase::Started.is_terminal());
        assert!(!Phase::Scanning.is_terminal());
    }

    #[test]
    fn noop_reporter_swallows_events() {
        ProgressReporter::noop().send(event());
    }

    #[test]
    fn channel_reporter_forwards_events() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ScannerProgress::new(ProgressReporter::channel(tx), "cache", 2);
        progress.scanning(10, Bytes(5), Some("/tmp/x".into()));

        let received = rx.try_recv().expect("event should have been forwarded");
        assert_eq!(received.scanner, "cache");
        assert_eq!(received.phase, Phase::Scanning);
        assert_eq!(received.files_scanned, 10);
        assert_eq!(received.scanners_total, 2);
    }

    #[test]
    fn messages_ride_along_with_terminal_events() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ScannerProgress::new(ProgressReporter::channel(tx), "docker", 1);
        progress.emit_message(Phase::Skipped, "docker is not installed");

        let received = rx.try_recv().expect("event should have been forwarded");
        assert_eq!(received.phase, Phase::Skipped);
        assert_eq!(received.message.as_deref(), Some("docker is not installed"));
    }

    #[test]
    fn sending_after_the_receiver_is_gone_does_not_panic() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        ProgressReporter::channel(tx).send(event());
    }
}
