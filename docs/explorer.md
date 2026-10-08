# The size explorer

Scanners answer "what is safe to delete?". The explorer answers the other
question people have, and the one that eventually matters: **"what is actually
in there?"**

No catalogue of well-known paths will ever find the 40 GB of raw video in
`~/projects/wedding-edit/`. Walking the tree by size does.

## What it does

Listing a folder measures every child recursively and shows:

- the real size of each entry, largest first;
- what share of the folder it accounts for, as a bar;
- how long since it was last opened;
- **whether it can be removed, and why not, before you click anything.**

Navigation is bounded to the home directory. The breadcrumb bar will not offer
a way above it, and `browse_directory` refuses a path outside it rather than
silently clamping — a mistake should be visible, not quietly redirected.

## Why it is fast enough to use

Measuring a folder means walking all of it, so listing `~` means measuring
everything underneath. Two things make that bearable:

**Children are measured in parallel** on the Rayon pool. The work is almost
entirely I/O latency, so measuring twelve subfolders at once is close to a free
twelve-fold speedup on an SSD.

**Results are memoised** in a `SizeCache` keyed by path. Stepping back up a
level — which is most of what browsing *is* — is instant, because the parent's
children were all measured on the way down.

The cache is invalidated for a path and all of its ancestors after a deletion,
since those are exactly the totals that just changed. Changes made by *other*
applications are not detected; the refresh button exists for that, and the
behaviour is asserted in a test rather than left to be discovered.

Enormous directories (a `node_modules`, a Maildir) are truncated to the largest
1 000 entries. The listing says how many were dropped, and the folder total
still counts everything — so the percentages do not silently rescale.

## How deletion is gated

The explorer is inherently path-addressed, which would otherwise hand the
webview the ability to name any path on disk for deletion. Three things prevent
that:

1. **The path must have been listed.** `AppState` remembers the paths from the
   most recent listing, and `delete_browsed` refuses anything not in that set.
   The frontend can only act on what the backend already showed it — the same
   property the scan flow gets from using result ids.
2. **The guard runs again**, with `Authority::Confirmed`. That permits exactly
   one extra tier — personal data — and nothing beyond it. System files,
   credentials, applications and volume roots are still refused, and no input
   from the UI can change that.
3. **Personal data needs a separate acknowledgement.** A second checkbox,
   unticked by default, with the delete button disabled until it is set. It
   names what the items actually are ("your documents") rather than asking for
   an abstract confirmation.

Symlinks are listed so the folder makes sense, but they are never measured
through and never deletable — following one could both inflate a folder's
apparent size and walk into a tree the guard already refused.

## The three tiers on screen

| Tier | In the list | Can it be deleted? |
|---|---|---|
| `refused` | lock icon in place of the checkbox, name dimmed | Never, by anything |
| `caution` | amber **Yours** badge | Yes, from here, after acknowledging |
| `allowed` | plain row | Yes |

Showing a *lock* where the checkbox would be — rather than a disabled checkbox —
is deliberate. A disabled checkbox looks almost identical to an enabled one, so
a user clicks it and nothing happens; the lock says "not selectable" before the
click instead of after it.

## Keyboard

- `Backspace` — up one folder (ignored while typing in a field)
- `Tab` / `Shift+Tab` — move through rows and controls
- `Space` — toggle the focused checkbox
- `Enter` — open the focused folder

## Limits worth knowing

- Sizes are *apparent* sizes (`metadata.len()`), so sparse files and compressed
  filesystems can disagree with what the OS reports.
- The first visit to a large folder is slow — there is no way around walking it.
  Subsequent visits come from the cache.
- The cache does not watch the filesystem. A `notify`-based watcher is on the
  [roadmap](roadmap.md).
