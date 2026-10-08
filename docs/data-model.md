# Data model

## Core types

All of these cross the Tauri boundary as camelCase JSON and are mirrored in
[`apps/desktop/src/lib/types.ts`](../apps/desktop/src/lib/types.ts).

### `ScanResult`

One cleanup opportunity. The unit the UI renders, the recommendation engine
ranks, and the cleanup engine acts on.

| Field | Type | Notes |
|---|---|---|
| `id` | `String` | BLAKE3 of `scanner\0path`, 16 hex chars. Stable across scans, so a selection survives a rescan |
| `scanner` | `String` | id of the producing scanner |
| `title` | `String` | headline, e.g. `Xcode DerivedData — ~/Library/…` |
| `description` | `String` | plain-language explanation, shown to the user |
| `path` | `PathBuf` | absolute |
| `size` | `Bytes` | newtype over `u64` |
| `kind` | `EntryKind` | `File` or `Directory` |
| `fileCount` | `u64` | files covered; 1 for a plain file |
| `category` | `Category` | grouping for the dashboard |
| `risk` | `RiskLevel` | `Safe` \| `Review` \| `Dangerous` |
| `deletable` | `bool` | whether SpaceKeeper is willing to remove it at all |
| `recoverable` | `bool` | whether removal can be undone |
| `lastAccessed` / `lastModified` | `Option<OffsetDateTime>` | RFC 3339; absent on filesystems that do not record atime |
| `score` | `f64` | 0–100, from the recommendation engine |
| `confidence` | `f64` | 0–1, how sure the scanner is |
| `detail` | `Option<Value>` | scanner-specific extras (duplicate group hash, Docker command) |

`Bytes` is a newtype rather than a bare `u64` so a byte count can never be
swapped with a file count, a score or a day count — all of which are also
integers moving through the same pipeline.

`RiskLevel` derives `Ord` on purpose: policy combines a scanner's proposal with
the protected-path rules using `max`, so risk can only ever be raised.

### `ScanReport`

`id`, `startedAt`, `durationMs`, `cancelled`, `results`, `summary`, and one
`ScannerOutcome` per scanner that ran (`completed` / `skipped` / `failed` /
`cancelled`, with a message). That last field is why a scan that quietly found
nothing is distinguishable from one that quietly broke.

### `CleanupRecord`

The audit record: `mode`, `dryRun`, `freed`, `removed`, `skipped`, `failed`,
`undoable`, and an `ItemOutcome` per item. A dry run produces the same structure
with `status: "planned"`, which is what the confirmation dialog displays — so
the numbers a user confirms come from the code that does the work.

### `Protection`

Attached to every explorer row and returned by `protected_paths`.

```rust
enum Protection {
    Allowed,
    Caution { reason: String },   // personal data — explorer only, with consent
    Refused { reason: String },   // nothing in SpaceKeeper can remove it
}
```

The `reason` is written for the user ("your saved passwords"), not for a
developer, because it is rendered directly.

### `BrowseEntry` / `DirectoryListing`

One row of the size explorer, and the folder containing them:

| Field | Notes |
|---|---|
| `size` | recursive for directories |
| `share` | fraction of the folder's total, `0.0..=1.0`, for the bar |
| `protection` | the tier above, with its reason |
| `isSymlink` | listed for context; never measured through, never deletable |

`DirectoryListing` adds `breadcrumbs`, `parent` (absent at a root, so navigation
cannot escape), `totalSize`, and `hiddenEntries` — the count dropped when a
folder exceeds 1 000 entries. The total still counts everything that was
dropped, so the percentages do not silently rescale.

## SQLite schema

One file in the platform data directory. Deleting it loses history and settings
and breaks nothing. Migrations are applied through the `user_version` pragma;
each is append-only and idempotent.

```sql
scans(id PK, started_at, duration_ms, cancelled,
      total_items, total_size, safe_size, review_size)

scan_items(scan_id FK→scans ON DELETE CASCADE, item_id, scanner, title,
           path, size, category, risk, deletable, score,
           PRIMARY KEY (scan_id, item_id))

cleanups(id PK, performed_at, mode, dry_run,
         freed, removed, skipped, failed, undoable)

cleanup_items(cleanup_id FK→cleanups ON DELETE CASCADE,
              item_id, path, size, status, message)

ignored_paths(path PK, added_at)
settings(key PK, value)                    -- JSON-encoded
scheduled_tasks(id PK, name, schedule, scanners, enabled, last_run_at)
scanner_stats(scanner PK, runs, total_items, total_size,
              total_duration_ms, last_run_at)
disk_snapshots(taken_at, mount_point, total, available)
```

Notes:

- **WAL journaling, `synchronous = NORMAL`.** The UI stays responsive while a
  large scan is written, and full durability is unnecessary for history.
- **Timestamps are RFC 3339 text.** Readable with `sqlite3` and unambiguous
  about timezone.
- **`scanner_stats` accumulates** via `ON CONFLICT DO UPDATE`, so per-scanner
  lifetime totals do not require re-reading history.
- **`disk_snapshots` is append-only**, giving the storage trend line.
- **Dry runs are stored but excluded** from history queries and `total_freed` —
  a preview is not something that happened to the user's disk.
- `Connection` is `Send` but not `Sync`, so it lives behind a `Mutex`.
  Contention is irrelevant: writes happen once per scan or cleanup.

## Settings

`ScanOptions` is stored as JSON under the `scan_options` key:

| Setting | Default | Effect |
|---|---|---|
| `minResultSize` | 1 MiB | results below this are dropped (zero-byte results, e.g. empty folders, are exempt) |
| `largeFileThreshold` | 256 MiB | what the large-files scanner considers large |
| `staleAfterDays` | 90 | when downloads and desktop files count as forgotten |
| `duplicateMinSize` | 1 MiB | smallest file the duplicate finder will hash |
| `maxDepth` | 12 | traversal depth for the searching scanners |
| `disabledScanners` | `[]` | scanner ids the user turned off |
| `ignoredPaths` | `[]` | folders to leave alone |

The ignore list is authoritative in its own table and is always reloaded from
there rather than trusted from the serialised settings blob.

**None of these change what SpaceKeeper is willing to delete.** They change what
a scan reports. Safety is decided by `PathGuard`, which the user can only make
stricter (by adding ignored paths), never looser.
