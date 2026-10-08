# Command interface

Every backend call the frontend can make. All are `#[tauri::command]` in
[`commands.rs`](../apps/desktop/src-tauri/src/commands.rs) and wrapped with
types in [`api.ts`](../apps/desktop/src/lib/api.ts).

Commands are deliberately thin: validate, delegate to `spacekeeper-core`,
serialise. No safety decision is made here — that would put it outside the reach
of the core's tests.

## Scanning

| Command | Arguments | Returns |
|---|---|---|
| `list_scanners` | — | `ScannerMetadata[]` |
| `start_scan` | `scanners?: string[]` | `ScanReport` |
| `cancel_scan` | — | — |
| `pause_scan` | — | — |
| `resume_scan` | — | — |
| `scan_status` | — | `{ paused, cancelled }` |

`start_scan` streams progress on the `scan://progress` event channel while it
runs and resolves with the finished report. Cancelling returns the results found
so far rather than discarding them.

## Cleanup

| Command | Arguments | Returns |
|---|---|---|
| `preview_cleanup` | `{ itemIds, mode }` | `CleanupRecord` (dry run) |
| `run_cleanup` | `{ itemIds, mode }` | `CleanupRecord` |
| `undo_cleanup` | `cleanupId` | `string[]` restored paths |
| `undo_supported` | — | `bool` |

**The UI sends ids, never paths.** `build_request` resolves them against the
report the backend itself produced, and drops anything the backend did not mark
`deletable`. A compromised webview cannot name an arbitrary path, and cannot
resurrect an item that policy refused.

`mode` is `"trash"` (default) or `"permanent"`.

## Explorer

| Command | Arguments | Returns |
|---|---|---|
| `browse_directory` | `path?`, `sort?` | `DirectoryListing` |
| `delete_browsed` | `paths`, `mode?`, `dryRun?` | `CleanupRecord` |

`browse_directory` measures every child recursively (in parallel, memoised) and
starts at the home directory when `path` is omitted. A path outside the home
directory is **refused**, not clamped — a mistake should be visible.

`delete_browsed` is the only path-addressed deletion in the app. It carries two
extra checks the id-based path does not need: every path must have appeared in
the listing the backend just produced, and the guard is consulted with
`Authority::Confirmed`, which permits personal data and nothing else. See
[explorer.md](explorer.md).

## Information

| Command | Arguments | Returns |
|---|---|---|
| `list_disks` | — | `DiskInfo[]` |
| `explain_disk` | — | `Explanation \| null` |
| `scan_history` | `limit?` | `ScanHistoryEntry[]` |
| `cleanup_history` | `limit?` | `CleanupRecord[]` |
| `total_freed` | — | `Bytes` |
| `disk_trend` | `limit?` | `DiskSnapshot[]` |
| `scanner_stats` | — | `ScannerStats[]` |

## Settings

| Command | Arguments | Returns |
|---|---|---|
| `get_settings` | — | `ScanOptions` |
| `save_settings` | `options` | `ScanOptions` |
| `ignored_paths` | — | `string[]` |
| `add_ignored_path` | `path` | `string[]` |
| `remove_ignored_path` | `path` | `string[]` |
| `protected_paths` | — | `ProtectedEntry[]` |
| `reveal_path` | `path` | — |

`protected_paths` returns `{ path, reason, refused }` — the reason in plain
language, and whether it is absolutely refused or merely personal data. It is
generated from the same guard that enforces it, so the settings screen cannot
drift from the behaviour.

`reveal_path` only *reveals* a path in the file manager (`open -R`,
`explorer /select,`, `xdg-open` on the parent). It never opens the file itself,
so a hostile filename in a scan result cannot become code execution.

## Errors

Every fallible command returns:

```ts
interface CommandError { kind: string; message: string }
```

`kind` is one of `cancelled`, `unavailable`, `protected`, `refused`, `database`,
`not_found`, `no_scan`, `internal` — so the UI can tell "nothing to do here"
apart from "something went wrong" without parsing prose.

## Events

```ts
listen<ScanProgress>("scan://progress", …)
```

```ts
interface ScanProgress {
  scanner: string;
  phase: "started" | "scanning" | "finished" | "skipped" | "failed";
  filesScanned: number;
  bytesFound: number;
  currentPath: string | null;
  scannersTotal: number;
  message: string | null;
}
```

The number of *completed* scanners is deliberately absent: the consumer counts
terminal phases itself, which keeps one source of truth and stops the progress
bar jumping backwards when concurrent scanners interleave.

## Capabilities

The app declares only `core:default` in
[`capabilities/default.json`](../apps/desktop/src-tauri/capabilities/default.json).
No filesystem, shell or HTTP plugin permission is granted — everything goes
through the commands above, which validate paths in Rust.
