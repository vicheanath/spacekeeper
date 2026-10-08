//! End-to-end tests over a synthetic home directory.
//!
//! These exercise the whole pipeline — registry, engine, ranking, policy,
//! cleanup and history — against real files on disk. The unit tests prove each
//! piece behaves; these prove the pieces are wired together, which is the part
//! that a UI screenshot would otherwise be the only evidence for.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use spacekeeper_core::cleanup::{
    Authority, CleanupEngine, CleanupItem, CleanupMode, CleanupRequest,
};
use spacekeeper_core::db::Database;
use spacekeeper_core::explain::explain;
use spacekeeper_core::{
    Bytes, PathGuard, RiskLevel, ScanContext, ScanEngine, ScanOptions, ScanReport, ScannerRegistry,
};

/// Build a home directory that looks like a developer's machine.
fn fake_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    let root = home.path();

    // A large, safe application cache.
    write(&root.join(".cache/some-app/blob.bin"), 3_000_000);
    // npm's cache.
    write(&root.join(".npm/_cacache/content/aa/bb.bin"), 2_500_000);
    // An abandoned JavaScript project.
    write(&root.join("code/old-api/package.json"), 400);
    write(
        &root.join("code/old-api/node_modules/left-pad/index.js"),
        4_000_000,
    );
    age(&root.join("code/old-api/package.json"), 500);
    // A stale download.
    write(&root.join("Downloads/installer.dmg"), 5_000_000);
    age(&root.join("Downloads/installer.dmg"), 400);
    // Two byte-identical copies of the same file.
    write_exact(
        &root.join("code/report.pdf"),
        b"identical contents",
        2_000_000,
    );
    write_exact(
        &root.join("Downloads/report copy.pdf"),
        b"identical contents",
        2_000_000,
    );
    // Things SpaceKeeper must never touch.
    write(&root.join(".ssh/id_ed25519"), 4_000);
    write(&root.join("Documents/thesis.pdf"), 9_000_000);

    home
}

fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, vec![b'x'; bytes]).expect("write");
}

/// Write `filler` repeated to exactly `bytes`, so two files are byte-identical.
fn write_exact(path: &Path, filler: &[u8], bytes: usize) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    let mut contents = Vec::with_capacity(bytes);
    while contents.len() < bytes {
        contents.extend_from_slice(filler);
    }
    contents.truncate(bytes);
    fs::write(path, contents).expect("write");
}

fn age(path: &Path, days: u64) {
    let when = SystemTime::now() - Duration::from_secs(days * 86_400);
    let file = fs::File::options().write(true).open(path).expect("open");
    file.set_times(fs::FileTimes::new().set_accessed(when).set_modified(when))
        .expect("set times");
}

fn context(home: &Path) -> ScanContext {
    ScanContext::new(home.to_path_buf()).with_options(ScanOptions {
        min_result_size: Bytes(100_000),
        duplicate_min_size: Bytes(1_000_000),
        stale_after_days: 90,
        ..ScanOptions::default()
    })
}

async fn scan(home: &Path) -> ScanReport {
    let mut registry = ScannerRegistry::new();
    registry.register_all(spacekeeper_scanners::builtin());
    ScanEngine::new(registry).run(&context(home), None).await
}

#[tokio::test]
async fn a_full_scan_finds_the_obvious_wins() {
    let home = fake_home();
    let report = scan(home.path()).await;

    assert!(
        !report.results.is_empty(),
        "a realistic home must produce findings"
    );
    assert!(!report.cancelled);

    let scanners: Vec<&str> = report.results.iter().map(|r| r.scanner.as_str()).collect();
    assert!(scanners.contains(&"node_modules"), "found: {scanners:?}");
    assert!(scanners.contains(&"downloads"), "found: {scanners:?}");
    assert!(scanners.contains(&"duplicates"), "found: {scanners:?}");
}

#[tokio::test]
async fn nothing_protected_is_ever_reported_as_deletable() {
    let home = fake_home();
    let report = scan(home.path()).await;

    for result in &report.results {
        let protected = result.path.starts_with(home.path().join(".ssh"))
            || result.path.starts_with(home.path().join("Documents"));
        assert!(
            !(protected && result.deletable),
            "{} was offered for deletion",
            result.path.display()
        );
    }
}

#[tokio::test]
async fn results_are_ranked_and_scores_are_in_range() {
    let home = fake_home();
    let report = scan(home.path()).await;

    assert!(
        report
            .results
            .windows(2)
            .all(|pair| pair[0].score >= pair[1].score),
        "results must arrive best-first"
    );
    assert!(report
        .results
        .iter()
        .all(|r| (0.0..=100.0).contains(&r.score)));
}

#[tokio::test]
async fn the_summary_agrees_with_the_results() {
    let home = fake_home();
    let report = scan(home.path()).await;

    let total: Bytes = report.results.iter().map(|r| r.size).sum();
    assert_eq!(total, report.summary.total_size);

    let safe: Bytes = report
        .results
        .iter()
        .filter(|r| r.risk == RiskLevel::Safe && r.deletable)
        .map(|r| r.size)
        .sum();
    assert_eq!(safe, report.summary.safe_size);
}

#[tokio::test]
async fn every_result_explains_itself_in_plain_language() {
    let home = fake_home();
    let report = scan(home.path()).await;

    for result in &report.results {
        assert!(
            !result.title.is_empty(),
            "{} has no title",
            result.path.display()
        );
        assert!(
            result.description.len() > 20,
            "{} has no usable explanation: {:?}",
            result.path.display(),
            result.description
        );
    }
}

#[tokio::test]
async fn the_explanation_quotes_only_numbers_from_the_report() {
    let home = fake_home();
    let report = scan(home.path()).await;
    let explanation = explain(&report, None);

    assert_eq!(explanation.safe_total, report.summary.safe_size);
    assert_eq!(explanation.review_total, report.summary.review_size);
    assert!(!explanation.next_steps.is_empty());
    for finding in &explanation.findings {
        assert!(
            report
                .summary
                .by_scanner
                .iter()
                .any(|g| g.key == finding.scanner),
            "explanation mentioned a scanner that is not in the report"
        );
    }
}

#[tokio::test]
async fn preview_then_clean_frees_exactly_what_was_promised() {
    let home = fake_home();
    let report = scan(home.path()).await;
    let engine = CleanupEngine::new(Arc::new(PathGuard::new(home.path())));

    let items: Vec<CleanupItem> = report
        .results
        .iter()
        .filter(|r| r.risk == RiskLevel::Safe && r.deletable)
        .map(Into::into)
        .collect();
    assert!(!items.is_empty(), "expected some safe items to clean");

    let request = CleanupRequest {
        items,
        mode: CleanupMode::Permanent,
        dry_run: false,
        authority: Authority::Automated,
    };

    let preview = engine.execute(&request.as_dry_run());
    assert!(preview.freed > Bytes::ZERO);
    assert!(!preview.undoable);

    let done = engine.execute(&request);
    assert_eq!(
        done.freed, preview.freed,
        "the preview must not over-promise"
    );
    assert_eq!(done.removed, preview.removed);
    assert_eq!(done.failed, 0);

    for item in &done.items {
        if item.status == spacekeeper_core::cleanup::ItemStatus::Deleted {
            assert!(
                !item.path.exists(),
                "{} survived deletion",
                item.path.display()
            );
        }
    }
}

#[tokio::test]
async fn a_cleanup_cannot_be_talked_into_touching_protected_files() {
    let home = fake_home();
    let engine = CleanupEngine::new(Arc::new(PathGuard::new(home.path())));

    // Simulate a compromised frontend asking for exactly the wrong things.
    let request = CleanupRequest {
        authority: Authority::Automated,
        items: vec![
            CleanupItem {
                id: "evil-1".into(),
                path: home.path().join(".ssh/id_ed25519"),
                size: Bytes(4_000),
                risk: RiskLevel::Safe,
            },
            CleanupItem {
                id: "evil-2".into(),
                path: home.path().join("Documents/thesis.pdf"),
                size: Bytes(9_000_000),
                risk: RiskLevel::Safe,
            },
            CleanupItem {
                id: "evil-3".into(),
                path: home.path().to_path_buf(),
                size: Bytes(0),
                risk: RiskLevel::Safe,
            },
        ],
        mode: CleanupMode::Permanent,
        dry_run: false,
    };

    let record = engine.execute(&request);

    assert_eq!(record.removed, 0);
    assert_eq!(record.skipped, 3);
    assert_eq!(record.freed, Bytes::ZERO);
    assert!(home.path().join(".ssh/id_ed25519").exists());
    assert!(home.path().join("Documents/thesis.pdf").exists());
    assert!(home.path().exists());
}

#[tokio::test]
async fn history_survives_a_round_trip_through_the_database() {
    let home = fake_home();
    let report = scan(home.path()).await;
    let db = Database::in_memory().expect("open database");

    db.record_scan(&report).expect("record scan");

    let history = db.recent_scans(10).expect("read history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, report.id);
    assert_eq!(history[0].total_size, report.summary.total_size);
    assert_eq!(history[0].total_items, report.summary.total_items);

    let stats = db.scanner_stats().expect("read stats");
    assert!(!stats.is_empty());
    assert!(stats.iter().all(|s| s.runs == 1));
}

#[tokio::test]
async fn ignoring_a_folder_removes_it_from_the_next_scan() {
    let home = fake_home();

    let before = scan(home.path()).await;
    assert!(before
        .results
        .iter()
        .any(|r| r.path.starts_with(home.path().join("code"))));

    let context = ScanContext::new(home.path().to_path_buf()).with_options(ScanOptions {
        min_result_size: Bytes(100_000),
        duplicate_min_size: Bytes(1_000_000),
        ignored_paths: vec![home.path().join("code")],
        ..ScanOptions::default()
    });

    let mut registry = ScannerRegistry::new();
    registry.register_all(spacekeeper_scanners::builtin());
    let after = ScanEngine::new(registry).run(&context, None).await;

    assert!(
        !after
            .results
            .iter()
            .any(|r| r.path.starts_with(home.path().join("code")) && r.deletable),
        "an ignored folder must not come back as deletable"
    );
}

#[tokio::test]
async fn a_scan_of_an_untouched_home_is_quiet_rather_than_wrong() {
    let home = tempfile::tempdir().expect("tempdir");
    let report = scan(home.path()).await;

    assert!(report.results.is_empty());
    assert_eq!(report.summary.safe_size, Bytes::ZERO);
    assert!(
        report
            .scanners
            .iter()
            .all(|outcome| outcome.status != spacekeeper_core::ScannerStatus::Failed),
        "an empty home must not make any scanner fail"
    );

    let explanation = explain(&report, None);
    assert!(explanation.findings.is_empty());
    assert!(explanation.next_steps[0].contains("Nothing needs your attention"));
}
