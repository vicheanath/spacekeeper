# Testing

```bash
cargo test --workspace          # 231 Rust tests
cargo clippy --all-targets      # lints; warnings are errors in CI
cd apps/desktop && npm run typecheck && npm run build
```

## What is tested, and why that

The tests are organised around the question "what would it mean for this app to
be *wrong*?" For a cleanup tool the answers are ranked, and the test suite is
weighted the same way:

1. **It deletes something irreplaceable.** Catastrophic.
2. **It fails to delete what the user asked for**, or reports the wrong number.
3. **It recommends badly** — technically correct, practically useless.
4. **It crashes or hangs.**

### 1. Safety

The largest group. `protect.rs`, `cleanup.rs` and `recommend.rs` each assert the
guarantees directly, and the end-to-end suite re-asserts them through the whole
pipeline:

- `policy_forces_protected_paths_to_dangerous_and_undeletable` — a scanner
  claiming `~/.ssh` is `Safe` is overruled;
- `policy_never_lowers_risk` — the lattice only moves one way;
- `protected_paths_are_skipped_even_when_requested` — an explicit request for
  credentials is refused;
- `a_cleanup_cannot_be_talked_into_touching_protected_files` — a simulated
  hostile frontend asks for `~/.ssh`, `~/Documents` and `~` itself; all three
  are skipped and the files survive;
- `symlinks_are_refused` — nothing is deleted through a link;
- `deleting_an_ancestor_of_a_protected_path_is_refused` — `/home` is refused
  because `~/.ssh` lives under it;
- `relative_and_traversing_paths_are_refused` — `..` cannot defeat the prefix
  checks;
- `credentials_stay_refused_even_when_confirmed` — the explorer's elevated
  authority does not reach an SSH key;
- `a_confirmed_request_still_refuses_credentials_and_system_files` — the same
  guarantee through the cleanup engine;
- `requests_default_to_the_strictest_authority` — including when deserialised
  from a payload that omits the field, so a malformed request fails safe;
- `volume_roots_are_refused` / `application_bundles_are_refused` — a whole disk
  or an installed app is not a folder to clean;
- `navigation_cannot_escape_the_roots` — the explorer's parent link cannot walk
  out of the home directory. This one caught a real bug: a symmetric-looking
  `root.starts_with(parent)` check was true for every root's own parent.

### 2. Correctness of the numbers

The headline "reclaimable" figure is the number users act on, so the arithmetic
behind it is tested directly:

- `overlapping_results_are_not_counted_twice` and
  `the_engine_never_double_counts_overlapping_findings` — a generic sweep and a
  specific scanner reporting the same bytes must not be summed;
- `exact_duplicate_paths_keep_the_more_confident_scanner` — and the user gets
  the better explanation of the two;
- `a_protected_parent_does_not_swallow_an_actionable_child`;
- `no_two_rules_claim_exactly_the_same_path`,
  `no_rule_is_nested_inside_another_rules_literal_path` and
  `a_wildcard_sweep_cedes_every_folder_a_named_rule_owns` — these treat the
  catalogue as data and fail the build when a new rule overlaps an existing
  one. They found three real cases the day they were written;
- `hard_links_are_not_reported_as_duplicates` — two names for the same inode
  free nothing, so offering one is a promise the cleanup cannot keep;
- `a_project_without_a_lockfile_is_never_one_click_safe` — reinstalling
  unpinned dependencies can change what the project runs.


- `preview_then_clean_frees_exactly_what_was_promised` — the dry run and the
  real run agree on `freed` and `removed`, so the confirmation dialog cannot
  over-promise;
- `the_summary_agrees_with_the_results` — the dashboard totals are the sum of
  the rows;
- `dry_runs_are_excluded_from_history_and_totals` — a preview is not something
  that happened;
- `one_bad_item_does_not_stop_the_batch` — a protected item in a batch does not
  prevent the rest.

### 3. Ranking

The recommendation engine is tested as *properties* rather than fixed values, so
the formula can be tuned without rewriting the suite:

- bigger scores higher, all else equal;
- staler scores higher, all else equal;
- safer scores higher, all else equal;
- `a_small_stale_safe_item_can_beat_a_huge_risky_one` — the whole reason for
  scoring instead of sorting by size;
- scores stay in `0..=100` even at `u64::MAX`;
- ranking is deterministic, so the list does not reshuffle between scans.

### 4. Robustness

- `a_panicking_scanner_does_not_bring_down_the_scan`;
- `a_failing_scanner_does_not_lose_other_results`;
- `a_scanner_that_discovers_it_is_unavailable_mid_run_is_skipped` — a stopped
  Docker daemon is not a red error;
- `cancelling_releases_a_paused_worker` — no deadlock between pause and cancel;
- `missing_directory_is_not_an_error`;
- `sending_after_the_receiver_is_gone_does_not_panic`.

### Scanner-level

Every built-in scanner has the same five cases: it finds the thing; it ignores
what it should; it respects the size threshold; it refuses protected paths; a
missing directory yields nothing rather than an error.

The catalogue is tested as *data*: ids are unique, every rule has paths and
platforms, no rule targets `~` or `/` or contains `..`, confidences are
probabilities, and nothing that costs a re-download is marked `Safe`.

### End to end

[`end_to_end.rs`](../crates/spacekeeper-scanners/tests/end_to_end.rs) builds a
synthetic home that looks like a developer's machine — caches, an abandoned npm
project, a stale download, two byte-identical PDFs, plus `~/.ssh` and
`~/Documents` — and runs the real pipeline over it: registry → engine → ranking
→ policy → preview → cleanup → SQLite → explanation.

This is what proves the pieces are *wired together*, which unit tests cannot.

## Conventions

- **Test names are sentences.** `nothing_protected_is_ever_reported_as_deletable`
  states a guarantee; `test_guard_2` does not.
- **Real filesystems, not mocks.** `tempfile` throughout. The bugs worth
  catching here are about symlinks, permissions and missing directories, and a
  mock filesystem has none of those.
- **No `unwrap` in test bodies** where a message would help; `.expect("mkdir")`
  says which step failed.
- **Behaviour, not implementation.** Tests assert what a user would observe.

## Not yet covered

Honestly stated:

- **No frontend unit tests.** TypeScript typechecking and the production build
  run in CI, but component behaviour is not tested. Vitest + Testing Library is
  on the roadmap.
- **No Windows or Linux CI runs yet** — the workflow matrix is defined and the
  path guard has per-platform tests, but the matrix has not been exercised.
- **`undo` is untested against a real trash**, since that would mutate the
  developer's actual Trash. The logic is small; the risk is accepted and noted.
- **No property-based testing.** `proptest` over the scoring function and the
  path guard would be a good addition.
