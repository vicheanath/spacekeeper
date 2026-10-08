//! Turns a scan report into plain language.
//!
//! This is SpaceKeeper's answer to "why is my drive full?". It is a template
//! engine over the scan report, not a model: it runs offline, produces the same
//! answer for the same input, and cannot invent a number that is not in the
//! report. A local LLM can be layered on later to reword this output, but the
//! facts will still come from here.

use serde::{Deserialize, Serialize};

use crate::disk::DiskInfo;
use crate::scan::ScanReport;
use crate::types::{Bytes, RiskLevel};

/// One sentence of the explanation, with the number behind it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// Scanner id, so the UI can link to the detail view.
    pub scanner: String,
    /// The sentence itself, e.g. "Docker is using 45 GB".
    pub sentence: String,
    /// Why this is safe (or not) to remove.
    pub reason: String,
    /// Size involved.
    pub size: Bytes,
    /// Risk of acting on it.
    pub risk: RiskLevel,
}

/// The full answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Explanation {
    /// One-line summary shown at the top of the dashboard.
    pub headline: String,
    /// Ordered findings, largest first.
    pub findings: Vec<Finding>,
    /// Space that can be reclaimed with no review needed.
    pub safe_total: Bytes,
    /// Space that could be reclaimed after the user reviews it.
    pub review_total: Bytes,
    /// What to do next, in order.
    pub next_steps: Vec<String>,
}

/// Build an explanation from a scan report and, optionally, the disk it ran on.
pub fn explain(report: &ScanReport, disk: Option<&DiskInfo>) -> Explanation {
    let safe_total = report.summary.safe_size;
    let review_total = report.summary.review_size;

    let headline = match disk {
        Some(disk) if safe_total > Bytes::ZERO => format!(
            "{} of {} is in use. SpaceKeeper found {} it can free safely.",
            disk.used(),
            disk.total,
            safe_total
        ),
        Some(disk) => format!(
            "{} of {} is in use. Nothing obvious to clean up right now.",
            disk.used(),
            disk.total
        ),
        None if safe_total > Bytes::ZERO => {
            format!("SpaceKeeper found {safe_total} it can free safely.")
        }
        None => "Nothing obvious to clean up right now.".to_string(),
    };

    let findings = build_findings(report);
    let next_steps = build_next_steps(&findings, safe_total, review_total);

    Explanation {
        headline,
        findings,
        safe_total,
        review_total,
        next_steps,
    }
}

fn build_findings(report: &ScanReport) -> Vec<Finding> {
    // Index results by scanner so each finding can quote a real risk level and
    // the scanner's own explanation text.
    report
        .summary
        .by_scanner
        .iter()
        .filter(|group| group.size > Bytes::ZERO)
        .take(6)
        .map(|group| {
            let sample = report.results.iter().find(|r| r.scanner == group.key);
            let name = report
                .scanners
                .iter()
                .find(|o| o.scanner == group.key)
                .map(|o| o.name.clone())
                .unwrap_or_else(|| group.key.clone());
            let risk = sample.map_or(RiskLevel::Review, |r| r.risk);

            Finding {
                scanner: group.key.clone(),
                sentence: format!("{name} is using {}.", group.size),
                reason: match risk {
                    RiskLevel::Safe => {
                        "These files are regenerated automatically when they are needed again."
                            .to_string()
                    }
                    RiskLevel::Review => {
                        "Worth a look before deleting — some of this may still be wanted."
                            .to_string()
                    }
                    RiskLevel::Dangerous => {
                        "Shown for information only. SpaceKeeper will not delete these.".to_string()
                    }
                },
                size: group.size,
                risk,
            }
        })
        .collect()
}

fn build_next_steps(findings: &[Finding], safe: Bytes, review: Bytes) -> Vec<String> {
    let mut steps = Vec::new();

    if safe > Bytes::ZERO {
        let biggest = findings
            .iter()
            .find(|f| f.risk == RiskLevel::Safe)
            .map(|f| f.scanner.clone());
        match biggest {
            Some(scanner) => steps.push(format!(
                "Clean the safe items to reclaim {safe} — start with {scanner}."
            )),
            None => steps.push(format!("Clean the safe items to reclaim {safe}.")),
        }
    }

    if review > Bytes::ZERO {
        steps.push(format!(
            "Review a further {review} of downloads and duplicates; these are kept until you decide."
        ));
    }

    if steps.is_empty() {
        steps.push("Nothing needs your attention. Run a scan again in a few weeks.".to_string());
    } else {
        steps.push("Everything is moved to the Trash by default, so you can put it back.".into());
    }

    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recommend;
    use crate::scan::{ScanReport, ScannerOutcome, ScannerStatus};
    use crate::types::{Category, ScanResult};
    use std::path::PathBuf;
    use time::OffsetDateTime;

    fn report(results: Vec<ScanResult>) -> ScanReport {
        let scanners = results
            .iter()
            .map(|r| ScannerOutcome {
                scanner: r.scanner.clone(),
                name: format!("{} scanner", r.scanner),
                status: ScannerStatus::Completed,
                items: 1,
                size: r.size,
                duration_ms: 1,
                message: None,
            })
            .collect();
        let summary = recommend::summarize(&results);
        ScanReport {
            id: "test".into(),
            started_at: OffsetDateTime::UNIX_EPOCH,
            duration_ms: 10,
            cancelled: false,
            results,
            summary,
            scanners,
        }
    }

    fn result(scanner: &str, size: u64, risk: RiskLevel) -> ScanResult {
        ScanResult::builder(scanner, format!("/home/tester/{scanner}"))
            .size(Bytes(size))
            .risk(risk)
            .category(Category::Cache)
            .deletable(true)
            .build()
    }

    fn disk(total: u64, available: u64) -> DiskInfo {
        DiskInfo {
            name: "Macintosh HD".into(),
            mount_point: PathBuf::from("/"),
            file_system: "apfs".into(),
            total: Bytes(total),
            available: Bytes(available),
            removable: false,
        }
    }

    #[test]
    fn headline_quotes_real_numbers() {
        let report = report(vec![result("docker", 45_000_000_000, RiskLevel::Safe)]);
        let explanation = explain(&report, Some(&disk(500_000_000_000, 100_000_000_000)));
        assert!(
            explanation.headline.contains("400 GB"),
            "{}",
            explanation.headline
        );
        assert!(
            explanation.headline.contains("45 GB"),
            "{}",
            explanation.headline
        );
    }

    #[test]
    fn findings_are_ordered_largest_first() {
        let report = report(vec![
            result("node", 18_000_000_000, RiskLevel::Safe),
            result("docker", 45_000_000_000, RiskLevel::Safe),
        ]);
        let explanation = explain(&report, None);
        assert_eq!(explanation.findings[0].scanner, "docker");
        assert_eq!(explanation.findings[1].scanner, "node");
    }

    #[test]
    fn safe_and_review_totals_are_separated() {
        let report = report(vec![
            result("cache", 10_000, RiskLevel::Safe),
            result("downloads", 4_000, RiskLevel::Review),
        ]);
        let explanation = explain(&report, None);
        assert_eq!(explanation.safe_total, Bytes(10_000));
        assert_eq!(explanation.review_total, Bytes(4_000));
        assert!(explanation
            .next_steps
            .iter()
            .any(|s| s.contains("Review a further")));
    }

    #[test]
    fn an_empty_report_says_so_without_inventing_work() {
        let explanation = explain(&report(Vec::new()), None);
        assert!(explanation.findings.is_empty());
        assert_eq!(explanation.safe_total, Bytes::ZERO);
        assert_eq!(explanation.next_steps.len(), 1);
        assert!(explanation.next_steps[0].contains("Nothing needs your attention"));
    }

    #[test]
    fn dangerous_findings_are_described_as_informational() {
        let report = report(vec![result("system", 9_000, RiskLevel::Dangerous)]);
        let explanation = explain(&report, None);
        assert!(explanation.findings[0].reason.contains("will not delete"));
        assert_eq!(explanation.safe_total, Bytes::ZERO);
    }

    #[test]
    fn undo_is_always_mentioned_when_there_is_work_to_do() {
        let report = report(vec![result("cache", 10_000, RiskLevel::Safe)]);
        let explanation = explain(&report, None);
        assert!(explanation.next_steps.iter().any(|s| s.contains("Trash")));
    }
}
