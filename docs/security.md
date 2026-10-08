# Security and privacy

## Privacy

SpaceKeeper contains **no network code**. Not disabled telemetry, not opt-out
analytics — no HTTP client is linked into the application at all. You can verify
this:

```bash
cargo tree -p spacekeeper-desktop | grep -iE "reqwest|hyper|ureq|curl"   # no matches
```

Scans, history and settings live in one SQLite file in your platform data
directory. Delete it at any time; the app recreates it and nothing else breaks.

Duplicate detection hashes file contents locally with BLAKE3. The hashes are
used within a single scan and are not persisted.

The Tauri app declares only `core:default`. No filesystem, shell or HTTP plugin
permission is granted to the webview.

## Threat model

### The webview is untrusted

Treated as an attacker throughout. A compromised frontend — via a dependency,
say — should not be able to destroy data.

- **Scan cleanups take result ids, not paths.** `build_request` resolves ids
  against the report the backend produced.
- **The explorer is path-addressed, and compensates.** `delete_browsed` refuses
  any path that was not in the listing the backend just produced, so the webview
  can still only act on what it was shown.
- **Items the backend did not mark deletable are dropped** from any request,
  even if their ids are sent.
- **`PathGuard` runs again at deletion time**, so a stale or forged report is
  not a path to arbitrary deletion.
- **Elevated authority widens by exactly one tier.** `Authority::Confirmed`
  permits personal data and nothing else; it defaults to `Automated` even when
  deserialised from a payload that omits the field, so a malformed or hostile
  request fails safe rather than open.
- **`reveal_path` only reveals.** It never opens a file, so a hostile filename
  cannot become code execution.

### A scanner is untrusted

Built-in scanners and future third-party plugins are treated identically.

- A scanner **proposes** a risk level; `apply_policy` can only raise it.
- A scanner claiming `~/.ssh` is `Safe` and deletable is overruled at ranking
  time *and* refused again at deletion time.
- Path patterns support `~` and `*` but **not `..`**, which would defeat the
  guard's prefix checks.
- Scanners cannot delete anything. They return descriptions; only
  `CleanupEngine` removes files.

### Other disks are not ours to delete

A folder with an external drive, network share or container volume mounted
inside it looks completely ordinary. Deleting it would empty that other disk.

* `PathGuard` refuses any path that *is* a mount point, by comparing its device
  id with its parent's.
* `fsutil::measure` records whether a tree spans more than one filesystem, and
  both the scanners and the explorer refuse to offer such a folder for deletion.

### The filesystem is hostile

- **Symlinks are never followed** when measuring, and are **refused** when
  deleting. A symlink into `/System` cannot make a cache look enormous, and
  cannot be a route out of a permitted directory.
- Traversal skips protected directories entirely rather than filtering results
  afterwards — the guard is not something a scanner can forget to apply.
- Unreadable entries are skipped, not fatal.

## What is protected

**Refused — nothing in SpaceKeeper can remove these:**

| Category | Examples |
|---|---|
| Credentials | `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.kube`, `~/.password-store`, keychains |
| Configuration | `~/.config`, `~/Library/Preferences`, `~/AppData/Roaming` |
| Applications | `/Applications`, `~/Applications`, any `*.app` / `*.framework` / `*.kext` |
| Device backups | `~/Library/Application Support/MobileSync` |
| System | `/System`, `/Library`, `/usr`, `/etc`, `C:\Windows`, `C:\Program Files`, … |
| Volume roots | `/Volumes/*`, `/media/*`, `/mnt/*` |
| The home directory itself | protected exactly, not as a subtree |
| Anything too shallow | fewer than two path components |
| User ignore list | whatever the user added |

**Caution — never cleaned automatically; removable from the explorer after an
explicit acknowledgement:**

| Category | Examples |
|---|---|
| Your own files | `~/Documents`, `~/Pictures`, `~/Movies`, `~/Music`, `~/Desktop` |
| Cloud drives | iCloud Drive, OneDrive, Dropbox, Google Drive |
| Communications | `~/Library/Mail`, `~/Library/Messages` |
| Version history | any `.git`, `.hg`, `.svn` |

The full list is shown in Settings → *Never touched*, generated from the same
`PathGuard` that enforces it, so the screen cannot drift from the behaviour.

## Least privilege

- No administrator or root privileges are requested. Ever. If a file needs
  elevation to delete, SpaceKeeper reports the failure and moves on.
- No background daemon, no login item, no kernel extension.
- Trash is the default; permanent deletion must be chosen per cleanup.
- Nothing classified `Dangerous` is deletable at all.

## Memory safety

`unsafe_code = "forbid"` is set workspace-wide. There is no `unsafe` in
SpaceKeeper's own code, and a crate would have to opt out explicitly with
written justification to add any.

## Reporting a vulnerability

Open a GitHub security advisory rather than a public issue. Please include the
path or scanner involved and what you were able to make it delete.
