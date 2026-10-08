//! Runs scanners and turns their raw output into a ranked report.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::sync::Semaphore;

use crate::progress::Phase;
use crate::recommend::{self, ResultsSummary};
use crate::scanner::{ScanContext, Scanner, ScannerRegistry};
use crate::types::{Bytes, ScanResult};

/// How a single scanner finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScannerStatus {
    /// Ran to completion.
    Completed,
    /// Not applicable on this machine (tool missing, directory absent).
    Skipped,
    /// Raised an error. The rest of the scan continued.
    Failed,
    /// Cancelled before it finished.
    Cancelled,
}

/// Per-scanner outcome, surfaced in the UI's diagnostics panel so a scan that
/// silently found nothing is distinguishable from one that quietly failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannerOutcome {
    /// Scanner id.
    pub scanner: String,
    /// Display name.
    pub name: String,
    /// How it ended.
    pub status: ScannerStatus,
    /// Number of results it contributed.
    pub items: usize,
    /// Total size it found.
    pub size: Bytes,
    /// Wall clock duration.
    pub duration_ms: u64,
    /// Reason, when skipped or failed.
    pub message: Option<String>,
}

/// The complete result of one scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    /// Unique id for this run, used as the database key.
    pub id: String,
    /// When the scan started.
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// Wall clock duration of the whole run.
    pub duration_ms: u64,
    /// Whether the user stopped the scan early. Results found before the stop
    /// are still returned — throwing away 30 seconds of work because someone
    /// pressed cancel would be hostile.
    pub cancelled: bool,
    /// Ranked results, best recommendation first.
    pub results: Vec<ScanResult>,
    /// Aggregates for the dashboard.
    pub summary: ResultsSummary,
    /// One entry per scanner that was selected to run.
    pub scanners: Vec<ScannerOutcome>,
}

impl ScanReport {
    /// The top `n` recommendations.
    pub fn top(&self, n: usize) -> &[ScanResult] {
        &self.results[..self.results.len().min(n)]
    }
}

/// Runs a registry's scanners against a context.
#[derive(Debug, Clone)]
pub struct ScanEngine {
    registry: ScannerRegistry,
    concurrency: usize,
}

impl ScanEngine {
    /// Build an engine with a concurrency limit derived from the machine.
    ///
    /// Scanners are I/O bound, so oversubscribing cores helps, but running all
    /// thirty at once on a spinning disk turns sequential reads into thrash.
    pub fn new(registry: ScannerRegistry) -> Self {
        let concurrency = (num_cpus::get() * 2).clamp(2, 16);
        Self {
            registry,
            concurrency,
        }
    }

    /// Override the number of scanners allowed to run concurrently.
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency.max(1);
        self
    }

    /// The registry this engine runs.
    pub fn registry(&self) -> &ScannerRegistry {
        &self.registry
    }

    /// Run every selected scanner and return a ranked report.
    ///
    /// Never returns `Err` for a scanner failure: one broken scanner must not
    /// cost the user the other twenty results. Failures are recorded in
    /// [`ScanReport::scanners`].
    pub async fn run(&self, context: &ScanContext, only: Option<&HashSet<String>>) -> ScanReport {
        let started_at = OffsetDateTime::now_utc();
        let clock = Instant::now();

        let selected = self.registry.selected(context, only);
        let mut context = context.clone();
        context.scanner_count = selected.len();

        let semaphore = Arc::new(Semaphore::new(self.concurrency));
        let mut tasks = tokio::task::JoinSet::new();

        for scanner in selected {
            let context = context.clone();
            let semaphore = Arc::clone(&semaphore);
            tasks.spawn(async move { run_one(scanner, context, semaphore).await });
        }

        let mut results = Vec::new();
        let mut outcomes = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((outcome, mut found)) => {
                    results.append(&mut found);
                    outcomes.push(outcome);
                }
                Err(err) => {
                    // A scanner panicked. Record it rather than propagating,
                    // and keep everything the other scanners found.
                    tracing::error!(%err, "scanner task panicked");
                    outcomes.push(ScannerOutcome {
                        scanner: "unknown".into(),
                        name: "unknown".into(),
                        status: ScannerStatus::Failed,
                        items: 0,
                        size: Bytes::ZERO,
                        duration_ms: 0,
                        message: Some(format!("scanner panicked: {err}")),
                    });
                }
            }
        }

        // The minimum-size threshold exists to keep a thousand tiny caches out
        // of the list. A zero-byte result was clearly not reported for its
        // size (empty folders, for instance), so the threshold does not apply.
        results
            .retain(|result| result.size == Bytes::ZERO || context.is_worth_reporting(result.size));
        // Two scanners can find the same bytes (the generic cache sweep and a
        // browser-specific one, say). Collapsing those before ranking is what
        // keeps the headline "reclaimable" figure from over-promising.
        let merged = recommend::dedupe_overlapping(&mut results);
        if merged > 0 {
            tracing::debug!(merged, "merged overlapping results");
        }
        recommend::rank(
            &mut results,
            &context.guard,
            OffsetDateTime::now_utc(),
            context.options.stale_after_days,
        );
        let summary = recommend::summarize(&results);
        outcomes.sort_by(|a, b| b.size.cmp(&a.size));

        ScanReport {
            id: uuid::Uuid::new_v4().to_string(),
            started_at,
            duration_ms: clock.elapsed().as_millis() as u64,
            cancelled: context.control.is_cancelled(),
            results,
            summary,
            scanners: outcomes,
        }
    }
}

/// Run one scanner, converting every failure mode into an outcome record.
async fn run_one(
    scanner: Arc<dyn Scanner>,
    context: ScanContext,
    semaphore: Arc<Semaphore>,
) -> (ScannerOutcome, Vec<ScanResult>) {
    let meta = scanner.metadata();
    let progress = context.progress_for(&meta.id);
    let clock = Instant::now();

    let outcome = |status, items, size, message, duration_ms| ScannerOutcome {
        scanner: meta.id.clone(),
        name: meta.name.clone(),
        status,
        items,
        size,
        duration_ms,
        message,
    };

    // `acquire_owned` on a semaphore that is never closed cannot fail; if it
    // somehow does, treat the scanner as skipped rather than panicking.
    let Ok(_permit) = semaphore.acquire_owned().await else {
        return (
            outcome(
                ScannerStatus::Skipped,
                0,
                Bytes::ZERO,
                Some("could not schedule".into()),
                0,
            ),
            Vec::new(),
        );
    };

    if context.control.is_cancelled() {
        return (
            outcome(ScannerStatus::Cancelled, 0, Bytes::ZERO, None, 0),
            Vec::new(),
        );
    }

    if !scanner.is_available(&context) {
        let message = "not available on this machine".to_string();
        progress.emit_message(Phase::Skipped, message.clone());
        return (
            outcome(ScannerStatus::Skipped, 0, Bytes::ZERO, Some(message), 0),
            Vec::new(),
        );
    }

    progress.emit(Phase::Started, 0, Bytes::ZERO, None, None);

    match scanner.scan(&context).await {
        Ok(results) => {
            let size: Bytes = results.iter().map(|r| r.size).sum();
            let duration_ms = clock.elapsed().as_millis() as u64;
            progress.emit(Phase::Finished, 0, size, None, None);
            (
                outcome(
                    ScannerStatus::Completed,
                    results.len(),
                    size,
                    None,
                    duration_ms,
                ),
                results,
            )
        }
        Err(err) if err.is_cancelled() => {
            let duration_ms = clock.elapsed().as_millis() as u64;
            (
                outcome(ScannerStatus::Cancelled, 0, Bytes::ZERO, None, duration_ms),
                Vec::new(),
            )
        }
        // A scanner may only discover mid-run that it has nothing to work with
        // — Docker is installed but its daemon is stopped, for instance. That
        // is a skip, not a failure, and must not show up as a red error.
        Err(err) if err.is_unavailable() => {
            let message = err.to_string();
            let duration_ms = clock.elapsed().as_millis() as u64;
            progress.emit_message(Phase::Skipped, message.clone());
            (
                outcome(
                    ScannerStatus::Skipped,
                    0,
                    Bytes::ZERO,
                    Some(message),
                    duration_ms,
                ),
                Vec::new(),
            )
        }
        Err(err) => {
            let message = err.to_string();
            let duration_ms = clock.elapsed().as_millis() as u64;
            tracing::warn!(scanner = %meta.id, %err, "scanner failed");
            progress.emit_message(Phase::Failed, message.clone());
            (
                outcome(
                    ScannerStatus::Failed,
                    0,
                    Bytes::ZERO,
                    Some(message),
                    duration_ms,
                ),
                Vec::new(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Control;
    use crate::error::{Error, Result};
    use crate::types::{Category, Family, RiskLevel, ScannerMetadata};
    use std::path::PathBuf;

    enum Behaviour {
        Findings(Vec<(&'static str, u64)>),
        Fail,
        Unavailable,
        UnavailableMidRun,
        Panic,
    }

    struct Fake {
        id: &'static str,
        behaviour: Behaviour,
    }

    #[async_trait::async_trait]
    impl Scanner for Fake {
        fn metadata(&self) -> ScannerMetadata {
            ScannerMetadata {
                id: self.id.to_string(),
                name: self.id.to_string(),
                description: String::new(),
                explanation: String::new(),
                family: Family::General,
                default_risk: RiskLevel::Safe,
                platforms: vec![std::env::consts::OS.to_string()],
                enabled_by_default: true,
                requires_tool: None,
            }
        }

        fn is_available(&self, _context: &ScanContext) -> bool {
            !matches!(self.behaviour, Behaviour::Unavailable)
        }

        async fn scan(&self, _context: &ScanContext) -> Result<Vec<ScanResult>> {
            match &self.behaviour {
                Behaviour::Findings(items) => Ok(items
                    .iter()
                    .map(|(path, size)| {
                        ScanResult::builder(self.id, *path)
                            .size(Bytes(*size))
                            .category(Category::Cache)
                            .risk(RiskLevel::Safe)
                            .deletable(true)
                            .confidence(1.0)
                            .build()
                    })
                    .collect()),
                Behaviour::Fail => Err(Error::Other(anyhow::anyhow!("boom"))),
                Behaviour::Unavailable => Ok(Vec::new()),
                Behaviour::UnavailableMidRun => {
                    Err(Error::unavailable(self.id, "the daemon is not running"))
                }
                Behaviour::Panic => panic!("scanner exploded"),
            }
        }
    }

    fn engine(scanners: Vec<Fake>) -> ScanEngine {
        let mut registry = ScannerRegistry::new();
        for scanner in scanners {
            registry.register(Arc::new(scanner));
        }
        ScanEngine::new(registry)
    }

    fn context() -> ScanContext {
        let mut context = ScanContext::new(PathBuf::from("/home/tester"));
        context.options.min_result_size = Bytes::ZERO;
        context
    }

    #[tokio::test]
    async fn collects_results_from_every_scanner() {
        let engine = engine(vec![
            Fake {
                id: "a",
                behaviour: Behaviour::Findings(vec![("/home/tester/a", 100)]),
            },
            Fake {
                id: "b",
                behaviour: Behaviour::Findings(vec![("/home/tester/b", 200)]),
            },
        ]);
        let report = engine.run(&context(), None).await;
        assert_eq!(report.results.len(), 2);
        assert_eq!(report.summary.total_size, Bytes(300));
        assert_eq!(report.scanners.len(), 2);
    }

    #[tokio::test]
    async fn a_failing_scanner_does_not_lose_other_results() {
        let engine = engine(vec![
            Fake {
                id: "good",
                behaviour: Behaviour::Findings(vec![("/home/tester/a", 100)]),
            },
            Fake {
                id: "bad",
                behaviour: Behaviour::Fail,
            },
        ]);
        let report = engine.run(&context(), None).await;
        assert_eq!(report.results.len(), 1);
        let bad = report
            .scanners
            .iter()
            .find(|o| o.scanner == "bad")
            .expect("outcome recorded");
        assert_eq!(bad.status, ScannerStatus::Failed);
        assert_eq!(bad.message.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn a_panicking_scanner_does_not_bring_down_the_scan() {
        let engine = engine(vec![
            Fake {
                id: "good",
                behaviour: Behaviour::Findings(vec![("/home/tester/a", 100)]),
            },
            Fake {
                id: "panics",
                behaviour: Behaviour::Panic,
            },
        ]);
        let report = engine.run(&context(), None).await;
        assert_eq!(report.results.len(), 1);
        assert!(report
            .scanners
            .iter()
            .any(|o| o.status == ScannerStatus::Failed));
    }

    #[tokio::test]
    async fn unavailable_scanners_are_skipped_not_failed() {
        let engine = engine(vec![Fake {
            id: "docker",
            behaviour: Behaviour::Unavailable,
        }]);
        let report = engine.run(&context(), None).await;
        assert_eq!(report.scanners[0].status, ScannerStatus::Skipped);
        assert!(report.results.is_empty());
    }

    #[tokio::test]
    async fn a_scanner_that_discovers_it_is_unavailable_mid_run_is_skipped() {
        let engine = engine(vec![Fake {
            id: "docker",
            behaviour: Behaviour::UnavailableMidRun,
        }]);
        let report = engine.run(&context(), None).await;
        assert_eq!(
            report.scanners[0].status,
            ScannerStatus::Skipped,
            "a missing daemon is not an error the user should see as red"
        );
        assert!(report.scanners[0]
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("daemon"));
    }

    #[tokio::test]
    async fn results_below_the_minimum_size_are_dropped() {
        let mut context = context();
        context.options.min_result_size = Bytes(150);
        let engine = engine(vec![Fake {
            id: "a",
            behaviour: Behaviour::Findings(vec![
                ("/home/tester/small", 100),
                ("/home/tester/big", 200),
            ]),
        }]);
        let report = engine.run(&context, None).await;
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].size, Bytes(200));
    }

    #[tokio::test]
    async fn the_engine_never_double_counts_overlapping_findings() {
        // Exactly the real-world shape: a generic cache sweep and a
        // browser-specific scanner reporting the same bytes.
        let engine = engine(vec![
            Fake {
                id: "app_cache",
                behaviour: Behaviour::Findings(vec![("/home/tester/Library/Caches", 4_000)]),
            },
            Fake {
                id: "chrome_cache",
                behaviour: Behaviour::Findings(vec![(
                    "/home/tester/Library/Caches/Google/Chrome",
                    2_000,
                )]),
            },
        ]);
        let report = engine.run(&context(), None).await;

        assert_eq!(report.results.len(), 1);
        assert_eq!(
            report.summary.total_size,
            Bytes(4_000),
            "the dashboard must not promise space that does not exist"
        );
    }

    #[tokio::test]
    async fn results_are_ranked_best_first() {
        let engine = engine(vec![Fake {
            id: "a",
            behaviour: Behaviour::Findings(vec![
                ("/home/tester/small", 1_000),
                ("/home/tester/huge", 10_000_000_000),
            ]),
        }]);
        let report = engine.run(&context(), None).await;
        assert_eq!(report.results[0].path, PathBuf::from("/home/tester/huge"));
        assert!(report.results[0].score > report.results[1].score);
    }

    #[tokio::test]
    async fn cancelling_before_the_run_yields_an_empty_cancelled_report() {
        let control = Control::new();
        control.cancel();
        let context = context().with_control(control);
        let engine = engine(vec![Fake {
            id: "a",
            behaviour: Behaviour::Findings(vec![("/home/tester/a", 100)]),
        }]);
        let report = engine.run(&context, None).await;
        assert!(report.cancelled);
        assert!(report.results.is_empty());
    }

    #[tokio::test]
    async fn only_filter_restricts_which_scanners_run() {
        let engine = engine(vec![
            Fake {
                id: "a",
                behaviour: Behaviour::Findings(vec![("/home/tester/a", 100)]),
            },
            Fake {
                id: "b",
                behaviour: Behaviour::Findings(vec![("/home/tester/b", 100)]),
            },
        ]);
        let only = HashSet::from(["b".to_string()]);
        let report = engine.run(&context(), Some(&only)).await;
        assert_eq!(report.scanners.len(), 1);
        assert_eq!(report.results[0].scanner, "b");
    }

    #[tokio::test]
    async fn progress_events_are_emitted() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let context = context().with_progress(crate::progress::ProgressReporter::channel(tx));
        let engine = engine(vec![Fake {
            id: "a",
            behaviour: Behaviour::Findings(vec![("/home/tester/a", 100)]),
        }]);
        let report = engine.run(&context, None).await;
        drop(report);

        let mut phases = Vec::new();
        while let Ok(event) = rx.try_recv() {
            phases.push(event.phase);
        }
        assert!(phases.contains(&Phase::Started));
        assert!(phases.contains(&Phase::Finished));
    }

    #[tokio::test]
    async fn top_returns_at_most_n_results() {
        let engine = engine(vec![Fake {
            id: "a",
            behaviour: Behaviour::Findings(vec![("/home/tester/a", 100), ("/home/tester/b", 200)]),
        }]);
        let report = engine.run(&context(), None).await;
        assert_eq!(report.top(1).len(), 1);
        assert_eq!(report.top(50).len(), 2);
    }
}
