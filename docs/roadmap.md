# Roadmap

Honest about what exists. Everything under "Built" is implemented and tested;
everything below it is not.

## Built (v0.1)

**Scanning**
- `Scanner` trait, registry, concurrent engine with a semaphore-bounded pool
- Declarative `PathRule` scanners — 47 entries covering general, developer,
  browser, desktop-app and AI locations
- Bespoke scanners: duplicates (BLAKE3), large files, `node_modules`, stale
  downloads, empty folders, Docker usage
- Cancellation, pause/resume, streaming progress
- Failure isolation: a failing *or panicking* scanner never costs other results
- Overlap collapsing, so two scanners finding the same bytes never inflate the
  reclaimable figure — enforced by catalogue tests as well as at runtime
- Broad sweeps report per application, not as one undifferentiated blob
- Duplicate detection skips hard links and clones, which free nothing
- `node_modules` is only one-click safe when a lockfile makes the reinstall
  reproducible

**Safety**
- `PathGuard` with three tiers (`Refused` / `Caution` / `Allowed`), each with a
  plain-language reason, plus descendant, ancestor, shallowness, volume-root,
  app-bundle and traversal checks
- Risk that policy can only escalate, never lower
- Guard re-validated at deletion time, with authority that widens by at most
  one tier and defaults to the strictest setting
- Symlinks never followed, never deleted
- Mount points refused, and folders containing one are never offered

**Size explorer**
- Browse the home directory by real recursive size, with parallel measurement
  and a memoised size cache
- Per-row protection shown before the click, not after
- Path-addressed deletion validated against the backend's own listing

**Cleanup**
- Trash by default, permanent deletion opt-in
- Dry run driving the confirmation dialog from the same code path
- Per-item audit record; one failure never aborts a batch
- Undo on Linux and Windows

**Storage** — SQLite with migrations: scan and cleanup history, settings, ignore
list, scanner statistics, disk snapshots

**Explanation** — offline template engine turning a report into plain language,
quoting only numbers the report contains

**UI** — dashboard with disk gauge and category breakdown, ranked results with
filtering and selection, the size explorer, cleanup confirmation, history, and
settings including the full protected-paths list with reasons;
light/dark/system themes

## Next (v0.2)

- **Actionable Docker cleanup.** Today Docker usage is reported and the command
  is printed. Running `docker image prune` on the user's behalf needs a
  command-based cleaner alongside the path-based one, with per-item confirmation
  and a hard exclusion for volumes.
- **Frontend tests.** Vitest + Testing Library. The selection logic, the
  confirmation dialog and the progress reducer are the parts worth covering.
- **CI matrix in anger.** The workflow covers macOS, Linux and Windows but has
  not been exercised on the latter two.
- **Scheduled scans.** The `scheduled_tasks` table exists and is unused.
- **Storage trend chart.** `disk_snapshots` is being written; nothing draws it
  yet. `recharts` is already a dependency.
- **Per-scanner selective scanning** in the UI — the backend already accepts a
  scanner subset.

## Later (v0.3+)

- **Out-of-process plugin host.** Today a plugin is a Rust type registered at
  startup, so third-party scanners require recompiling. The planned host runs
  them over the same `Scanner` shape in a WASM sandbox with no filesystem write
  capability. Safety does not change: a plugin still only proposes, and
  `PathGuard` still decides.
- **Near-duplicate detection.** Perceptual hashing for images, chunk-level
  similarity for video, text similarity for documents. Exact duplicates are the
  easy case and are already done.
- **Folder size treemap.** A graphical counterpart to the explorer's list.
- **`notify`-based watching**, so the dashboard reflects a large download
  without a rescan.
- **More scanners**: Photos libraries, Windows Recycle Bin, Wine prefixes,
  Proton, Blender caches, Time Machine local snapshots.
- **Localisation.** The UI has no hardcoded layout assumptions, but no strings
  are extracted yet.
- **CLI** (`spacekeeper scan --json`) for scripted use — the core crate is
  already usable without a window.

## Explicitly not planned

- **Cloud sync, accounts, telemetry.** The privacy guarantee is the product.
- **"Speed up your Mac" features.** Memory purging, startup item removal and
  registry cleaning range from placebo to harmful.
- **Automatic cleanup without confirmation.** Nothing is deleted that a person
  did not select. This is not a setting.
- **Deleting Docker volumes.** Reported, explained, never removed — there is no
  way to know whether a volume holds a database someone needs.

## Known gaps

- `undo` cannot restore on macOS (no OS API); the app says so rather than
  offering a button that fails.
- `~/.config` is a refused tree, which is aggressive on Linux — a few legitimate
  caches under it are therefore invisible. Safety was chosen over coverage;
  revisit with a narrower rule.
- The explorer's size cache does not watch the filesystem, so changes made by
  other applications need the refresh button. `notify` is the fix.
- There is no `~/Desktop` scanner: desktop files are classified as personal
  data, so automated cleanup cannot offer them. The explorer covers that folder
  instead.
- Directory sizes are *apparent* sizes (`metadata.len()`), so sparse files and
  compressed filesystems can differ from what the OS reports.
- The large-files and duplicate scanners cap their output (300 and 500 items).
  The cap is documented in code but not surfaced in the UI.
