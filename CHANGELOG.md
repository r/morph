# Changelog

All notable user-visible changes to Morph are recorded here. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Morph
adheres to [Semantic Versioning](https://semver.org/) (pre-1.0: minor = new
behavior, patch = fix or small improvement).

This file starts with version `0.37.2`. Earlier history (every version since
`0.3.0`) is preserved in the git log — `git log --grep '(0\.\|v0\.'`. The
website mirrors only the most recent few entries; see
[site/changelog.html](site/changelog.html).

When you bump the workspace version in `Cargo.toml`, add a matching section
here before committing. Behavioral commits should also carry their evaluation
metrics — see `.cursor/rules/behavioral-commits.mdc`.

## [Unreleased]

## [0.37.7] — 2026-04-29

### Fixed

- **Every `Store::list(t)` is now fast on `FsStore`.** 0.37.4 and
  0.37.5 fixed the two acute symptoms of an unindexed type
  (short-hash prefix lookup, then `morph status` / `morph eval gaps`),
  but every remaining unindexed type — `Blob`, `Tree`, `Pipeline`,
  `Commit`, `Artifact`, `TraceRollup` — was a latent version of the
  same bug: any code path calling `Store::list(<that-type>)` walked
  the whole object fanout and JSON-deserialized every object just to
  filter by type. This release closes that surface area: every
  top-level object type now has a dedicated `<type>/` index dir
  maintained by `Store::put`, and `fs_list` reads directly from it.
  Newly-indexed types use zero-byte marker files instead of full JSON
  copies — on a 33 GB blob-heavy store, copying every blob into
  `.morph/blobs/` would have doubled disk usage; a marker only costs
  an inode. The five pre-existing indexes (`runs/`, `traces/`,
  `evals/`, `prompts/`, `annotations/`) keep their full-content
  format because at least one fallback path (`morph tap`) reads them.
  Legacy stores (≤0.37.6) trigger a one-shot lazy rebuild on the
  first `list(<unindexed type>)` call: the rebuild walks the store
  *once* and populates *every* missing index simultaneously, so
  whichever list call happens first amortizes the cost for all
  subsequent list calls of every type.

### Tests

- New unit tests in `morph-core::store::tests`:
  - `list_every_type_uses_index_not_get` — wraps a real `FsStore` in
    a proxy whose `Store::get(...)` panics, populates one object of
    each easily-constructible type via `put`, and asserts `list(t)`
    returns the right hashes via the index alone (no deserialize).
  - `list_pipeline_uses_index_after_put` — separate fixture for the
    Pipeline type, which has a heavier `PipelineGraph` field surface.
  - `list_legacy_store_rebuilds_every_missing_type_index_in_one_walk`
    — writes raw object JSON into the fanout (bypassing `put`) so no
    index dirs exist, then asserts the first `list(Blob)` call
    creates *every* `<type>/.indexed` marker, not just `blobs/`.
  - `type_index_files_are_markers_for_new_types_full_content_for_legacy`
    — asserts `blobs/<hash>.json` is a zero-byte marker while
    `annotations/<hash>.json` keeps full JSON content.
  - `prompt_blob_lands_in_both_blobs_and_prompts_indexes` — covers
    the kind-subset case so `list(Blob)` surfaces prompts via the
    new `blobs/` index without `prompts/` having to widen its scope.
- `count_dir_entries` (in `working.rs`) and the cucumber
  `then_repo_has_n_run_records` step both now filter to `*.json`,
  so the `.indexed` marker doesn't inflate counts.

## [0.37.6] — 2026-04-29

### Changed

- **Homepage leads with the Homebrew install path.** The hero on
  `site/index.html` now shows a copy-able `brew tap r/morph` /
  `brew install morph` block above the CTA buttons, with a
  click-to-copy button and a footnote pointing at the source-build
  alternative for non-macOS users. The `#install` section was
  rewritten to lead with the Homebrew path and keep
  `cargo install --path …` as the cross-platform fallback. Goal: cut
  the time from "land on the page" to "actually try Morph" on macOS
  to two commands.

### CI

- **Releases auto-tag from `Cargo.toml`.** A new
  `.github/workflows/auto-tag.yml` watches the workspace version on
  `main`; when it advances to a value that has not yet been tagged on
  origin, the workflow creates `vX.Y.Z` (annotated) and dispatches
  `release-homebrew.yml` against that tag. Cutting a release is now
  "bump `Cargo.toml`, push to `main`" — no more manual
  `git tag && git push origin <tag>`. The dispatch indirection is
  required because `GITHUB_TOKEN`-pushed refs do not retrigger
  workflows; dispatching against the tag's ref makes
  `release-homebrew`'s metadata step see
  `GITHUB_REF=refs/tags/v<version>` and run the full tag-release path
  (`is_tag_release=true`, formula update, etc.).

## [0.37.5] — 2026-04-29

### Fixed

- **`morph status` and `morph eval gaps` no longer hang on stores with
  many objects.** Both commands route through
  `list_stale_certifications`, which calls
  `Store::list(ObjectType::Annotation)`. Annotation was the only
  top-level object type without a per-type index dir, so on `FsStore`
  the call fell through to `fs_list`'s slow path: walk the entire
  object fanout and JSON-deserialize every object just to filter by
  type. On a 94k-object / 33 GB store that was a ~2-minute walk on
  every invocation. Annotations now have an `annotations/` type-index
  directory maintained opportunistically by `Store::put`. Legacy
  stores (≤0.37.4) trigger a one-shot lazy rebuild on first
  `list(Annotation)` and drop a `.indexed` marker so subsequent calls
  use the fast path. Verified on the bug-surfacing repo: first call
  after upgrade pays the ~2 min rebuild once; second call drops to
  **0.18 s** (a ~600× speedup), and `morph status` lands in **0.17 s**.

### Tests

- New unit tests in `morph-core::store::tests`:
  - `list_annotation_lazily_builds_index_on_legacy_store` — writes
    annotation objects directly into the fanout (bypassing `put`),
    asserts `list(Annotation)` rebuilds the index dir, returns every
    annotation, and writes the `.indexed` marker.
  - `list_annotation_after_marker_does_not_deserialize_objects` —
    drops a poison file in the object fanout, asserts a marker-present
    `list(Annotation)` returns the indexed result without touching
    the fanout.
  - `list_annotation_filters_to_annotations_only_after_rebuild` —
    mixes 20 noise blobs with 4 annotations, asserts only the
    annotations come back via the indexed fast path.
- New acceptance suite `annotations_indexed.yaml` (3 cases) covers
  `morph certify` populating the index, `morph status` rendering
  cleanly post-certification, and `morph eval gaps` succeeding on a
  certified branch.

## [0.37.4] — 2026-04-29

### Fixed

- **`morph certify --commit <prefix>` no longer hangs on stores with
  many objects.** Short-hash prefix resolution (`resolve_hash_prefix`,
  used by `certify`, `show`, `run show`, `trace show`, `annotate`,
  `revert`, etc.) iterated all 10 object types and called
  `Store::list(t)` for each. For backends without a per-type index
  (every type except Run, Trace, EvalSuite on `FsStore`),
  `list(t)` deserialized every object on disk just to filter by type —
  turning a single prefix lookup into O(7·N) JSON reads. On a real repo
  with thousands of objects this looked like a hang. Resolution now
  goes through a new `Store::list_hashes_with_prefix` method; `FsStore`
  with fanout layout (the default since 0.4) walks only the matching
  `objects/<2chars>/` subdirectory and performs zero JSON
  deserialization. Behavior is unchanged — same matches, same
  ambiguous/not-found errors — just fast.

### Tests

- New unit test `resolve_hash_prefix_does_not_iterate_object_types`
  in `morph-core::store::tests` wraps a real `FsStore` in a proxy whose
  `Store::list(type)` panics, proving the prefix path never iterates
  types again.
- New unit test `list_hashes_with_prefix_fanout_walks_only_target_subdir`
  drops a poison file into an unrelated fanout subdirectory and
  asserts the fast path leaves it untouched.
- New acceptance suite `certify_prefix_lookup.yaml` (2 cases) covers
  `morph certify --commit <8-char prefix>` and `<12-char prefix>`
  on repos seeded with multiple commits and runs.

## [0.37.3] — 2026-04-29

### Fixed

- **`morph eval gaps` now surfaces stale certifications.** When a morph
  commit had a `kind: "certification"` annotation that was later
  invalidated by a `kind: "rewritten"` annotation (typically because
  `git commit --amend` or `git rebase` superseded the underlying git
  SHA), `morph status` would print a one-line `stale certification: N`
  summary but the structured `morph eval gaps --json` output was silent.
  Agents and CI watching the gap stream couldn't detect rebase-rotted
  evidence programmatically. `compute_eval_gaps` now emits a
  `stale_certifications` entry — `kind`, `count`, `commits` array of
  affected morph hashes, and a hint pointing at
  `morph certify --commit <successor> --metrics ...`. The check is
  mode-agnostic (works in both reference and standalone repos); the
  existing `morph status` line is unchanged.

### Tests

- New acceptance suite `eval_gaps_stale_certifications.yaml` (3 cases)
  covering the reference-mode amend path, the audit-trail invariant
  (re-certifying the successor does not retroactively clear the gap),
  and the standalone-mode equivalent.

## [0.37.2] — 2026-04-29

### Added

- `CHANGELOG.md` at the repo root and a matching `Changelog` page on the
  website ([site/changelog.html](site/changelog.html)) so user-visible
  changes are tracked in one place. Three most recent versions surface on
  the website; the codebase file grows with every release.
- Reminder in `.cursor/rules/version-bump.mdc` to update the changelog when
  bumping the workspace version.

### Changed

- Homepage version badge bumped to `v0.37.2`. Nav now links to the new
  changelog page.

## [0.37.1] — 2026-04-28

### Fixed

- **Reference mode, multi-commit fast-forward pulls** — `sync_to_head` used
  to mirror only the tip of a multi-commit `git pull`, collapsing N new
  commits into a single Morph mirror with the wrong parent edge. It now
  walks first-parent ancestry back to the last-mirrored commit (or root)
  and mirrors the unmirrored span in topo-forward order via a shared
  `sync_range` helper that `backfill_from_init` also uses.
- **Reference mode, drift detection** — `drift_summary` returned
  `unmirrored_count = 0` whenever the git `HEAD` itself was mirrored,
  silently masking the multi-commit-pull bug above and any other tool path
  where `HEAD` has a mirror but ancestors don't. The early-return on
  `cache.contains_key(head)` is gone; the walk is uniform from `HEAD` until
  either a mirrored ancestor or a root, with the 10k-commit cap preserved.

### Tests

- New acceptance suites: `reference_mode_multi_commit_pull.yaml` (4 cases)
  and `reference_mode_drift_topo.yaml` (3 cases). Workspace test count:
  1119 / 1119 passing.

## [0.37.0] — 2026-04-28

A workspace-wide audit-and-repair pass: correctness fixes, library swaps in
favor of well-tested crates, +20 tests, and a documentation rewrite.

### Added

- 20 new tests, including `SshUrl` edge cases (IPv6 brackets ± port,
  unbracketed IPv6 rejection, malformed schemes, Windows drive letters), an
  agent instance-id 1000-id uniqueness test, three `morph gc` acceptance
  specs, eight `morph traces` acceptance specs (every `TracesCmd`), an
  `morph-mcp` read-only handler smoke test, and a `reference_sync` error
  path test.

### Changed

- **`uuid` v4 replaces the hand-rolled `generate_instance_id`.** The old
  scheme packed `time × pid` into 24 bits (~16M id space); now it slices 12
  hex chars from a v4 UUID.
- **`strip-ansi-escapes` replaces the hand-rolled CSI scanner in
  `eval_parsers`.** Full terminal-escape grammar instead of `ESC[`-only.
- `default_clone_dest` delegates to `SshUrl::parse` so SSH/SCP/IPv6 forms
  share one authoritative parser.
- README, `docs/v0-spec.md`, `docs/MERGE.md`, `docs/EVAL-DRIVEN.md`,
  `docs/SERVER-SETUP.md`, `docs/MULTI-MACHINE.md`, `docs/reference-mode.md`,
  and `docs/TESTING.md` rewritten (not patched) for current behavior.
  `MORPH_EVAL_GAP_ANALYSIS.md` moved to `docs/plans/morph-tap-gap-analysis.md`
  with a historical-snapshot banner.

### Fixed

- `SshUrl::parse` handles bracketed IPv6 literals
  (`ssh://user@[::1]:22/repo`) and rejects unbracketed IPv6 instead of
  silently mis-splitting on the colon.
- `set_branch_upstream` no longer reads `config.json` twice (TOCTOU +
  redundant IO).
- `init_morph_dir_at` propagates serde failures as
  `MorphError::Serialization` instead of panicking via
  `expect()` / `unwrap()`.
- `agent::write_instance_id` maps the `as_object_mut()` failure through
  `Result` instead of `unwrap()`.
- `cli::run_merge` single-shot path uses an exhaustive if-let-tuple instead
  of three coupled `.unwrap()`s on pipeline / metrics / message; replaces
  `serde_json::to_string` `.unwrap()`s with `?`.

## [0.36.0] — 2026-04-26

Three coordinated changes to repo setup, adoption, and migration.

### Added

- **Fresh `morph init` lands at the latest store version (0.5)** — the
  modern fan-out + git-format-hash backend, instead of seeding every new
  repo as legacy 0.0. New `write_repo_version` helper lets test fixtures
  and migration tooling downgrade explicitly when they need a legacy
  starting point.
- **`morph init` inside an existing `.git` working tree** now detects the
  git repo, prints a one-line nudge on stderr pointing at
  `morph init --reference`, and adds `.morph/` to `.git/info/exclude` so
  Morph state stays out of `git status`. `--reference` and `--bare`
  branches are unchanged.
- **`migrate_to_latest()` in `morph-core::migrate`** replaces the
  hand-rolled migration ladder in `Command::Upgrade`. Walks the chain in
  one call and returns a `MigrateReport` of
  `MigrationStep { from, to, description }`. Centralized
  `STORE_VERSION_LATEST` and `SUPPORTED_REPO_VERSIONS` constants; CLI
  gates, MCP gates, and `morph version --json` now read from the same
  source of truth. Adding a new store version is two edits (constant +
  chain step), not five.
- 15 new YAML acceptance spec cases in the default eval suite:
  `init_at_latest:*` ×4, `init_in_git_dir:*` ×6, `upgrade:*` ×5.

[Unreleased]: https://github.com/r/morph/compare/v0.37.7...HEAD
[0.37.7]: https://github.com/r/morph/compare/v0.37.6...v0.37.7
[0.37.6]: https://github.com/r/morph/compare/v0.37.5...v0.37.6
[0.37.5]: https://github.com/r/morph/compare/v0.37.4...v0.37.5
[0.37.4]: https://github.com/r/morph/compare/v0.37.3...v0.37.4
[0.37.3]: https://github.com/r/morph/compare/v0.37.2...v0.37.3
[0.37.2]: https://github.com/r/morph/compare/v0.37.1...v0.37.2
[0.37.1]: https://github.com/r/morph/compare/v0.37.0...v0.37.1
[0.37.0]: https://github.com/r/morph/compare/v0.36.0...v0.37.0
[0.36.0]: https://github.com/r/morph/releases/tag/v0.36.0
