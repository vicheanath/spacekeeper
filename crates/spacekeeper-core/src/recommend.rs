//! Ranking and safety policy applied to raw scanner output.
//!
//! Scanners describe what they found; this module decides what SpaceKeeper is
//! willing to say about it. Two things happen here:
//!
//! * **policy** — the protected-path rules are applied on top of whatever risk
//!   a scanner proposed. Risk can only ever be raised, never lowered, so a
//!   buggy or hostile plugin cannot mark `~/.ssh` as safe;
//! * **ranking** — a score that answers "what should I delete first?" better
//!   than sorting by size does.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::protect::PathGuard;
use crate::types::{Bytes, Category, RiskLevel, ScanResult};

/// Reference point for the size term of the score.
///
/// Sizes are compressed logarithmically: without it a single 200 GB item would
/// flatten every other recommendation to zero, and users overwhelmingly have
/// many medium-sized wins rather than one huge one.
const SIZE_REFERENCE: f64 = 50.0 * 1024.0 * 1024.0 * 1024.0;

/// Floor for the staleness term, so a large, safe, freshly-written cache is
/// still recommended — just below an equally large stale one.
const STALENESS_FLOOR: f64 = 0.3;

/// Apply the protected-path policy to a scanner's result.
///
/// This is deliberately the only way risk gets finalised, and it is applied by
/// the engine to *every* result, whatever produced it.
pub fn apply_policy(result: &mut ScanResult, guard: &PathGuard) {
    if guard.check(&result.path).is_err() {
        result.risk = result.risk.max(RiskLevel::Dangerous);
        result.deletable = false;
    }
    if result.risk == RiskLevel::Dangerous {
        // Nothing dangerous is ever offered for one-click cleanup.
        result.deletable = false;
    }
}

/// Score a result in `0..=100`.
///
/// `size × staleness × safety × confidence`, each term normalised so the
/// product stays comparable across scanners.
pub fn score(result: &ScanResult, now: OffsetDateTime, stale_after_days: u32) -> f64 {
    if result.size == Bytes::ZERO {
        return 0.0;
    }
    let size_term = ((result.size.get() as f64).ln_1p() / SIZE_REFERENCE.ln_1p()).clamp(0.0, 1.0);

    let stale_days = result.unused_days(now);
    let horizon = f64::from(stale_after_days.max(1));
    let staleness_term =
        STALENESS_FLOOR + (1.0 - STALENESS_FLOOR) * (stale_days / horizon).clamp(0.0, 1.0);

    let safety_term = result.risk.weight();
    let confidence_term = result.confidence.clamp(0.0, 1.0);

    // Being able to undo a deletion genuinely lowers the stakes, so nudge
    // recoverable items up rather than treating them identically.
    let recoverability = if result.recoverable { 1.0 } else { 0.85 };

    (100.0 * size_term * staleness_term * safety_term * confidence_term * recoverability)
        .clamp(0.0, 100.0)
}

/// Remove results whose bytes are already accounted for by another result.
///
/// Two scanners can legitimately find the same disk space. `app_cache` reports
/// all of `~/Library/Caches`; `chrome_cache` reports a folder inside it. Both
/// findings are true, but they are the *same bytes* — summing them tells the
/// user they can reclaim more than exists, which is the one number the whole
/// dashboard is built on.
///
/// Two cases are collapsed:
///
/// * **the same path twice** — the higher-confidence result wins, so a scanner
///   written specifically for Chrome beats the generic cache sweep and the
///   user gets the better explanation;
/// * **a path inside another** — the descendant is dropped, because deleting
///   the ancestor removes it anyway and the ancestor's size already includes
///   it. An ancestor only covers its children if it is itself deletable: when
///   the parent is protected, the child is the only actionable item and must
///   survive.
///
/// Returns how many results were merged away.
pub fn dedupe_overlapping(results: &mut Vec<ScanResult>) -> usize {
    let before = results.len();

    // Highest confidence first so that, among identical paths, the first one
    // seen is the one worth keeping.
    results.sort_by(|a, b| {
        a.path.cmp(&b.path).then_with(|| {
            b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    results.dedup_by(|later, kept| later.path == kept.path);

    // Paths are sorted, so an ancestor always appears before its descendants.
    let mut covering: Option<std::path::PathBuf> = None;
    results.retain(|result| {
        if let Some(ancestor) = &covering {
            if result.path.starts_with(ancestor) {
                return false;
            }
        }
        if result.deletable {
            covering = Some(result.path.clone());
        }
        true
    });

    before - results.len()
}

/// Apply policy and scoring to a batch, then sort best-first.
pub fn rank(
    results: &mut [ScanResult],
    guard: &PathGuard,
    now: OffsetDateTime,
    stale_after_days: u32,
) {
    for result in results.iter_mut() {
        apply_policy(result, guard);
        result.score = score(result, now, stale_after_days);
    }
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            // Stable tiebreak keeps the list from reshuffling between scans.
            .then_with(|| b.size.cmp(&a.size))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Aggregate figures for one grouping key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSummary {
    /// Grouping key: category id, scanner id, or risk level.
    pub key: String,
    /// Human readable label.
    pub label: String,
    /// Number of results in the group.
    pub items: usize,
    /// Total size of the group.
    pub size: Bytes,
}

/// Everything the dashboard needs about a set of results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultsSummary {
    /// Number of results.
    pub total_items: usize,
    /// Total size of all results.
    pub total_size: Bytes,
    /// Total size of results that are safe and deletable — the headline
    /// "recoverable space" figure.
    pub safe_size: Bytes,
    /// Total size of results needing review before deletion.
    pub review_size: Bytes,
    /// Breakdown by category, largest first.
    pub by_category: Vec<GroupSummary>,
    /// Breakdown by scanner, largest first.
    pub by_scanner: Vec<GroupSummary>,
}

/// Summarise a ranked result set.
pub fn summarize(results: &[ScanResult]) -> ResultsSummary {
    let mut by_category: BTreeMap<Category, (usize, Bytes)> = BTreeMap::new();
    let mut by_scanner: BTreeMap<String, (usize, Bytes)> = BTreeMap::new();
    let mut total_size = Bytes::ZERO;
    let mut safe_size = Bytes::ZERO;
    let mut review_size = Bytes::ZERO;

    for result in results {
        total_size = total_size.saturating_add(result.size);
        match result.risk {
            RiskLevel::Safe if result.deletable => {
                safe_size = safe_size.saturating_add(result.size)
            }
            RiskLevel::Review if result.deletable => {
                review_size = review_size.saturating_add(result.size);
            }
            _ => {}
        }

        let category = by_category
            .entry(result.category)
            .or_insert((0, Bytes::ZERO));
        category.0 += 1;
        category.1 = category.1.saturating_add(result.size);

        let scanner = by_scanner
            .entry(result.scanner.clone())
            .or_insert((0, Bytes::ZERO));
        scanner.0 += 1;
        scanner.1 = scanner.1.saturating_add(result.size);
    }

    let mut categories: Vec<_> = by_category
        .into_iter()
        .map(|(key, (items, size))| GroupSummary {
            key: key.as_str().to_string(),
            label: key.label().to_string(),
            items,
            size,
        })
        .collect();
    categories.sort_by(|a, b| b.size.cmp(&a.size));

    let mut scanners: Vec<_> = by_scanner
        .into_iter()
        .map(|(key, (items, size))| GroupSummary {
            label: key.clone(),
            key,
            items,
            size,
        })
        .collect();
    scanners.sort_by(|a, b| b.size.cmp(&a.size));

    ResultsSummary {
        total_items: results.len(),
        total_size,
        safe_size,
        review_size,
        by_category: categories,
        by_scanner: scanners,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ScanResult;
    use std::path::{Path, PathBuf};

    fn now() -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH + time::Duration::days(1000)
    }

    fn result(path: &str, size: u64, risk: RiskLevel, days_idle: i64) -> ScanResult {
        let mut r = ScanResult::builder("test", path)
            .size(Bytes(size))
            .risk(risk)
            .deletable(true)
            .confidence(1.0)
            .last_accessed(Some(now() - time::Duration::days(days_idle)))
            .build();
        r.score = score(&r, now(), 90);
        r
    }

    fn guard() -> PathGuard {
        PathGuard::new(Path::new("/home/tester"))
    }

    #[test]
    fn bigger_items_score_higher_all_else_equal() {
        let small = result("/home/tester/a", 100_000_000, RiskLevel::Safe, 10);
        let big = result("/home/tester/b", 10_000_000_000, RiskLevel::Safe, 10);
        assert!(big.score > small.score);
    }

    #[test]
    fn staler_items_score_higher_all_else_equal() {
        let fresh = result("/home/tester/a", 1_000_000_000, RiskLevel::Safe, 0);
        let stale = result("/home/tester/b", 1_000_000_000, RiskLevel::Safe, 365);
        assert!(stale.score > fresh.score);
    }

    #[test]
    fn safer_items_score_higher_all_else_equal() {
        let safe = result("/home/tester/a", 1_000_000_000, RiskLevel::Safe, 30);
        let review = result("/home/tester/b", 1_000_000_000, RiskLevel::Review, 30);
        assert!(safe.score > review.score);
    }

    #[test]
    fn a_small_stale_safe_item_can_beat_a_huge_risky_one() {
        // The whole point of scoring instead of sorting by size.
        let small_safe = result("/home/tester/a", 2_000_000_000, RiskLevel::Safe, 400);
        let huge_risky = result("/home/tester/b", 20_000_000_000, RiskLevel::Review, 0);
        assert!(small_safe.score > huge_risky.score);
    }

    #[test]
    fn empty_items_score_zero() {
        let empty = result("/home/tester/a", 0, RiskLevel::Safe, 400);
        assert_eq!(empty.score, 0.0);
    }

    #[test]
    fn scores_stay_within_bounds() {
        let enormous = result("/home/tester/a", u64::MAX, RiskLevel::Safe, 100_000);
        assert!((0.0..=100.0).contains(&enormous.score));
    }

    #[test]
    fn policy_forces_protected_paths_to_dangerous_and_undeletable() {
        let mut sneaky = ScanResult::builder("evil", "/home/tester/.ssh")
            .size(Bytes(1000))
            .risk(RiskLevel::Safe)
            .deletable(true)
            .build();
        apply_policy(&mut sneaky, &guard());
        assert_eq!(sneaky.risk, RiskLevel::Dangerous);
        assert!(
            !sneaky.deletable,
            "a plugin must not be able to delete credentials"
        );
    }

    #[test]
    fn policy_never_lowers_risk() {
        let mut cautious = ScanResult::builder("test", "/home/tester/.cache/x")
            .risk(RiskLevel::Review)
            .deletable(true)
            .build();
        apply_policy(&mut cautious, &guard());
        assert_eq!(cautious.risk, RiskLevel::Review);
        assert!(cautious.deletable);
    }

    #[test]
    fn dangerous_items_are_never_deletable() {
        let mut item = ScanResult::builder("test", "/home/tester/.cache/x")
            .risk(RiskLevel::Dangerous)
            .deletable(true)
            .build();
        apply_policy(&mut item, &guard());
        assert!(!item.deletable);
    }

    #[test]
    fn ranking_sorts_best_first_and_is_deterministic() {
        let mut results = vec![
            result("/home/tester/small", 10_000_000, RiskLevel::Safe, 1),
            result("/home/tester/big", 5_000_000_000, RiskLevel::Safe, 300),
            result("/home/tester/mid", 500_000_000, RiskLevel::Safe, 100),
        ];
        rank(&mut results, &guard(), now(), 90);
        let paths: Vec<_> = results.iter().map(|r| r.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/tester/big"),
                PathBuf::from("/home/tester/mid"),
                PathBuf::from("/home/tester/small"),
            ]
        );
        assert!(results.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn overlapping_results_are_not_counted_twice() {
        // `app_cache` reports all of ~/Library/Caches; `chrome_cache` reports a
        // folder inside it. Both are real findings, but their bytes are the
        // same bytes — summing them inflates the headline figure.
        let mut results = vec![
            result("/home/tester/Library/Caches", 4_000, RiskLevel::Safe, 10),
            result("/home/tester/Library/Caches/Google/Chrome", 2_000, RiskLevel::Safe, 10),
        ];
        assert_eq!(dedupe_overlapping(&mut results), 1);
        rank(&mut results, &guard(), now(), 90);

        let summary = summarize(&results);
        assert_eq!(
            summary.total_size,
            Bytes(4_000),
            "the child's bytes are already inside the parent's total"
        );
    }

    #[test]
    fn exact_duplicate_paths_keep_the_more_confident_scanner() {
        let mut generic = result("/home/tester/Library/Caches/Homebrew", 4_000, RiskLevel::Safe, 10);
        generic.scanner = "app_cache".into();
        generic.confidence = 0.9;
        let mut specific =
            result("/home/tester/Library/Caches/Homebrew", 4_000, RiskLevel::Safe, 10);
        specific.scanner = "homebrew_cache".into();
        specific.confidence = 0.95;

        let mut results = vec![generic, specific];
        assert_eq!(dedupe_overlapping(&mut results), 1);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].scanner, "homebrew_cache",
            "the scanner that knows what it is found should win"
        );
    }

    #[test]
    fn a_protected_parent_does_not_swallow_an_actionable_child() {
        // The parent cannot be deleted, so dropping the child would leave the
        // user with nothing to act on.
        let mut parent = result("/home/tester/thing", 9_000, RiskLevel::Safe, 10);
        parent.deletable = false;
        let child = result("/home/tester/thing/cache", 4_000, RiskLevel::Safe, 10);

        let mut results = vec![parent, child];
        assert_eq!(dedupe_overlapping(&mut results), 0);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn unrelated_siblings_are_both_kept() {
        let mut results = vec![
            result("/home/tester/a/cache", 4_000, RiskLevel::Safe, 10),
            result("/home/tester/b/cache", 4_000, RiskLevel::Safe, 10),
            result("/home/tester/ab/cache", 4_000, RiskLevel::Safe, 10),
        ];
        assert_eq!(dedupe_overlapping(&mut results), 0, "sibling prefixes are not containment");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn deeply_nested_results_collapse_to_the_outermost() {
        let mut results = vec![
            result("/home/tester/x", 9_000, RiskLevel::Safe, 10),
            result("/home/tester/x/y", 5_000, RiskLevel::Safe, 10),
            result("/home/tester/x/y/z", 2_000, RiskLevel::Safe, 10),
        ];
        assert_eq!(dedupe_overlapping(&mut results), 2);
        assert_eq!(results[0].path, PathBuf::from("/home/tester/x"));
    }

    #[test]
    fn summary_splits_safe_from_review() {
        let results = vec![
            result("/home/tester/a", 1_000, RiskLevel::Safe, 10),
            result("/home/tester/b", 2_000, RiskLevel::Review, 10),
            result("/home/tester/c", 4_000, RiskLevel::Safe, 10),
        ];
        let summary = summarize(&results);
        assert_eq!(summary.total_items, 3);
        assert_eq!(summary.total_size, Bytes(7_000));
        assert_eq!(summary.safe_size, Bytes(5_000));
        assert_eq!(summary.review_size, Bytes(2_000));
    }

    #[test]
    fn summary_excludes_undeletable_items_from_recoverable_space() {
        let mut protected = result("/home/tester/.ssh", 9_000, RiskLevel::Safe, 10);
        apply_policy(&mut protected, &guard());
        let summary = summarize(&[protected]);
        assert_eq!(summary.safe_size, Bytes::ZERO);
        assert_eq!(summary.total_size, Bytes(9_000));
    }

    #[test]
    fn summary_groups_are_sorted_by_size() {
        let results = vec![
            result("/home/tester/a", 1_000, RiskLevel::Safe, 1),
            result("/home/tester/b", 9_000, RiskLevel::Safe, 1),
        ];
        let summary = summarize(&results);
        assert_eq!(summary.by_category.len(), 1);
        assert_eq!(summary.by_scanner[0].size, Bytes(10_000));
    }
}
