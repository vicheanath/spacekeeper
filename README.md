# SpaceKeeper

**Understand why your disk is full, and reclaim space without breaking anything.**

SpaceKeeper is an open-source desktop app that finds what is using your storage,
explains each finding in plain language, and helps you remove it safely. It runs
entirely on your machine — there is no network code in the application at all.

It is deliberately not a "one-click speed-up" tool. It shows you what it found,
tells you what each thing actually is, and lets you decide.

```
Rust + Tauri v2 + React 19 · MIT OR Apache-2.0 · macOS, Linux, Windows
```

## What makes it different

| | Typical cleaner | SpaceKeeper |
|---|---|---|
| Tells you what "DerivedData" is | ✗ | ✓ every finding is explained in plain words |
| Sorts by size | ✓ | ✓ **and** by how safe, stale and recoverable it is |
| Can be talked into deleting `~/.ssh` | sometimes | ✗ refused in Rust, at the moment of deletion |
| Browse your disk by size and delete in place | rarely | ✓ with per-row protection shown before you click |
| Sends telemetry | usually | ✗ no network code |
| Undo | rarely | ✓ Trash by default; permanent deletion is opt-in |

## Quick start

```bash
git clone https://github.com/spacekeeper/spacekeeper
cd spacekeeper/apps/desktop && npm install && npm run tauri dev
```

Requires Rust (stable), Node 20+, and the
[Tauri v2 prerequisites](https://tauri.app/start/prerequisites/) for your OS.

## Safety model

The one thing a cleanup tool must never do is delete something irreplaceable.
Four mechanisms enforce that, and all of them live in Rust where they are unit
tested:

1. **Scanners propose, policy decides.** A scanner suggests a risk level.
   [`recommend::apply_policy`](crates/spacekeeper-core/src/recommend.rs) can only
   ever *raise* it. A buggy or hostile scanner cannot mark `~/.ssh` as safe.
2. **The path guard runs twice.** [`PathGuard`](crates/spacekeeper-core/src/protect.rs)
   is checked when results are produced *and again* at the moment of deletion,
   because a scan may be minutes old by the time someone clicks. It refuses
   credentials, documents, photos, system directories, anything too shallow
   (`/usr`), relative paths, and any path containing `..`.
3. **Protection has three tiers, not two.** `Refused` (system files,
   credentials, applications, volume roots, `*.app` bundles) can be deleted by
   nothing, ever. `Caution` (your documents, photos, iCloud Drive, a repo's
   `.git`) is never touched by automated cleanup, but *can* be removed from the
   file explorer after a separate, explicit acknowledgement. The tiering is why
   the explorer is useful in `~/Documents` without a scanner bug being able to
   eat a thesis.
4. **The UI never names a path.** Scan cleanups send result *ids*, resolved
   against the report the backend produced. The explorer is path-addressed by
   nature, so it carries the same property a different way: a path is only
   accepted if the backend listed it in the folder currently on screen.

Deletion defaults to the OS Trash. Permanent deletion exists but must be chosen
explicitly, and nothing classified `Dangerous` is ever deletable at all.

## How recommendations are ranked

Sorting by size alone recommends the wrong things — your 20 GB of active project
dependencies outranks a 2 GB cache you have not touched in a year. SpaceKeeper
scores each finding:

```
score = size × staleness × safety × confidence × recoverability
```

with size compressed logarithmically so one enormous item does not flatten
everything else. See [`recommend.rs`](crates/spacekeeper-core/src/recommend.rs);
the properties this is meant to have are asserted directly as tests.

## What it looks for

**General** — trash, application caches, logs, crash reports, stale downloads,
duplicates (BLAKE3), large files, empty folders, plus Slack, Discord, Spotify,
Adobe and Steam caches.

**Developer** — npm/yarn/pnpm/pip/Cargo/Gradle/Maven/NuGet/Go/Composer/
RubyGems/Bun/Deno caches, `node_modules` (dated by the *project*, not the
folder), Xcode DerivedData, device support and archives, iOS Simulator data,
Android system images and emulators, CocoaPods, Homebrew, Unity, Godot,
JetBrains, VS Code, Podman, Docker usage.

**Browsers** — Chrome, Firefox, Safari, Edge, Brave caches.

**AI** — Ollama, LM Studio, Hugging Face, ComfyUI model weights.

**Or browse it yourself** — the Explore tab walks your home folder with every
folder's real recursive size, so you can find the 40 GB nobody's scanner would
ever guess at. Each row shows whether it is removable *before* you click.

## Documentation

| Document | What it covers |
|---|---|
| [Architecture](docs/architecture.md) | How the pieces fit, and why they are shaped that way |
| [Data model](docs/data-model.md) | Core types and the SQLite schema |
| [Writing a scanner](docs/scanners.md) | The extension point, with a worked example |
| [Explorer](docs/explorer.md) | Browsing by size, and how deletion is gated there |
| [Command interface](docs/commands.md) | Every Tauri command |
| [Testing](docs/testing.md) | What is tested and how |
| [Security & privacy](docs/security.md) | Threat model and guarantees |
| [Roadmap](docs/roadmap.md) | What is done and what is next |
| [Contributing](CONTRIBUTING.md) | Development workflow |

## Repository layout

```
crates/spacekeeper-core/       types, scan engine, ranking, cleanup, SQLite
crates/spacekeeper-scanners/   every built-in scanner
apps/desktop/                  React 19 + Base UI frontend
apps/desktop/src-tauri/        Tauri v2 shell and commands
docs/                          architecture and developer documentation
```

## Status

Early but real: scanning, ranking, cleanup, the size explorer and history all
work end to end and are covered by 215 tests. See the [roadmap](docs/roadmap.md) for what is
not built yet — including actionable Docker cleanup, scheduling, near-duplicate
detection and the out-of-process plugin host.

## Licence

MIT or Apache-2.0, at your option.
