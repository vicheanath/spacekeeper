//! Docker disk usage.
//!
//! Docker is the one place where the honest answer is "SpaceKeeper will not do
//! this for you". Its images, volumes and build cache are not plain files we
//! can move to the Trash — they live inside a virtual disk, removal is not
//! reversible, and deleting a volume can destroy a database someone needs.
//!
//! So this scanner reports what Docker is using and tells the user the exact
//! command to run. The results are marked not deletable, which means the
//! cleanup engine will refuse them even if the UI asked. Actionable Docker
//! cleanup, run through the Docker CLI with per-item confirmation, is planned
//! for a later release.

use std::process::Stdio;
use std::time::Duration;

use spacekeeper_core::{
    Bytes, Category, EntryKind, Family, Result, RiskLevel, ScanContext, ScanResult, Scanner,
    ScannerMetadata,
};

/// Give up if the Docker daemon does not answer. A stopped Docker Desktop can
/// leave the CLI waiting a long time, and a scan must not hang on it.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Reports Docker's disk usage.
#[derive(Debug, Clone, Default)]
pub struct DockerScanner;

#[async_trait::async_trait]
impl Scanner for DockerScanner {
    fn metadata(&self) -> ScannerMetadata {
        ScannerMetadata {
            id: "docker".into(),
            name: "Docker".into(),
            description: "Images, containers, volumes and build cache".into(),
            explanation: "Space used by Docker. SpaceKeeper reports it but does not remove it: Docker data is not stored as ordinary files and removing a volume can destroy data a project depends on. Use the suggested command when you are ready.".into(),
            family: Family::Developer,
            default_risk: RiskLevel::Review,
            platforms: vec!["macos".into(), "linux".into(), "windows".into()],
            enabled_by_default: true,
            requires_tool: Some("docker".into()),
        }
    }

    async fn scan(&self, context: &ScanContext) -> Result<Vec<ScanResult>> {
        let progress = context.progress_for("docker");
        context.control.check_async().await?;

        let output = tokio::time::timeout(
            TIMEOUT,
            tokio::process::Command::new("docker")
                .args([
                    "system",
                    "df",
                    "--format",
                    "{{.Type}}|{{.Size}}|{{.Reclaimable}}",
                ])
                .stdin(Stdio::null())
                .output(),
        )
        .await;

        let output = match output {
            Ok(Ok(output)) if output.status.success() => output,
            Ok(Ok(_)) => {
                return Err(spacekeeper_core::Error::unavailable(
                    "docker",
                    "the Docker daemon is not running",
                ))
            }
            Ok(Err(err)) => {
                return Err(spacekeeper_core::Error::unavailable(
                    "docker",
                    err.to_string(),
                ))
            }
            Err(_) => {
                return Err(spacekeeper_core::Error::unavailable(
                    "docker",
                    "the Docker daemon did not respond",
                ))
            }
        };

        let results = parse_system_df(&String::from_utf8_lossy(&output.stdout));
        let total: Bytes = results.iter().map(|r| r.size).sum();
        progress.scanning(results.len() as u64, total, None);
        Ok(results)
    }
}

/// Parse the output of `docker system df` in our pipe-separated format.
fn parse_system_df(stdout: &str) -> Vec<ScanResult> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('|');
            let kind = parts.next()?.trim();
            let size = parse_size(parts.next()?.trim())?;
            let reclaimable = parts.next().and_then(|value| parse_size(value.trim()));
            if size == Bytes::ZERO {
                return None;
            }
            Some(build(kind, size, reclaimable))
        })
        .collect()
}

fn build(kind: &str, size: Bytes, reclaimable: Option<Bytes>) -> ScanResult {
    let (what, command) = match kind {
        "Images" => (
            "Container images you have pulled or built",
            "docker image prune -a",
        ),
        "Containers" => (
            "Stopped containers and their writable layers",
            "docker container prune",
        ),
        "Local Volumes" => (
            "Volumes holding container data — these may contain databases you still need",
            "docker volume prune",
        ),
        "Build Cache" => (
            "Intermediate layers kept to make rebuilds faster",
            "docker builder prune",
        ),
        _ => ("Docker data", "docker system df"),
    };

    let reclaimable_text = reclaimable
        .filter(|value| *value > Bytes::ZERO)
        .map(|value| format!(" Docker reports {value} of this as reclaimable."))
        .unwrap_or_default();

    // The path is synthetic: nothing here is a file SpaceKeeper can delete, and
    // `deletable(false)` means the cleanup engine will refuse it regardless.
    ScanResult::builder(
        "docker",
        format!("/docker/{}", kind.to_lowercase().replace(' ', "-")),
    )
    .title(format!("Docker — {kind}"))
    .description(format!(
        "{what}.{reclaimable_text} Run `{command}` to reclaim it."
    ))
    .size(size)
    .kind(EntryKind::Directory)
    .category(Category::Container)
    .risk(RiskLevel::Review)
    .deletable(false)
    .recoverable(false)
    .confidence(0.9)
    .detail(serde_json::json!({ "command": command, "kind": kind }))
    .build()
}

/// Parse a Docker size string such as `1.234GB`, `512MB` or `0B`.
///
/// Docker prints decimal units, and appends a percentage to the reclaimable
/// column (`12.3GB (45%)`), which is ignored.
fn parse_size(value: &str) -> Option<Bytes> {
    let value = value.split('(').next()?.trim();
    if value.is_empty() || value == "N/A" {
        return None;
    }

    let split = value.find(|c: char| c.is_ascii_alphabetic())?;
    let (number, unit) = value.split_at(split);
    let number: f64 = number.trim().parse().ok()?;

    let multiplier = match unit.trim().to_ascii_uppercase().as_str() {
        "B" => 1.0,
        "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };

    Some(Bytes((number * multiplier) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_decimal_sizes() {
        assert_eq!(parse_size("0B"), Some(Bytes(0)));
        assert_eq!(parse_size("512MB"), Some(Bytes(512_000_000)));
        assert_eq!(parse_size("1.5GB"), Some(Bytes(1_500_000_000)));
        assert_eq!(parse_size("2TB"), Some(Bytes(2_000_000_000_000)));
    }

    #[test]
    fn ignores_the_reclaimable_percentage() {
        assert_eq!(parse_size("12.3GB (45%)"), Some(Bytes(12_300_000_000)));
    }

    #[test]
    fn rejects_unparseable_sizes() {
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("N/A"), None);
        assert_eq!(parse_size("lots"), None);
        assert_eq!(parse_size("5PB"), None);
    }

    #[test]
    fn parses_a_full_system_df_report() {
        let stdout = "Images|45.2GB|30.1GB (66%)\n\
                      Containers|1.2GB|0B (0%)\n\
                      Local Volumes|8GB|8GB (100%)\n\
                      Build Cache|12GB|12GB\n";
        let results = parse_system_df(stdout);

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].size, Bytes(45_200_000_000));
        assert_eq!(results[0].category, Category::Container);
    }

    #[test]
    fn zero_sized_rows_are_dropped() {
        let results = parse_system_df("Images|0B|0B\nContainers|1GB|0B\n");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Docker — Containers");
    }

    #[test]
    fn malformed_lines_are_skipped_rather_than_failing() {
        let results = parse_system_df("garbage\nImages|1GB|0B\n\n|");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn docker_results_are_never_deletable() {
        let results = parse_system_df("Local Volumes|8GB|8GB (100%)\n");
        assert!(
            !results[0].deletable,
            "SpaceKeeper must never delete a Docker volume itself"
        );
        assert_eq!(results[0].risk, RiskLevel::Review);
    }

    #[test]
    fn each_row_names_the_command_that_would_reclaim_it() {
        let results = parse_system_df("Build Cache|12GB|12GB\n");
        assert!(results[0].description.contains("docker builder prune"));
        assert!(results[0].detail.is_some());
    }

    #[test]
    fn volumes_are_described_as_possibly_holding_real_data() {
        let results = parse_system_df("Local Volumes|8GB|8GB\n");
        assert!(results[0].description.contains("still need"));
    }
}
