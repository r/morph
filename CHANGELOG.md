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

[Unreleased]: https://github.com/r/morph/compare/v0.37.3...HEAD
[0.37.3]: https://github.com/r/morph/compare/v0.37.2...v0.37.3
[0.37.2]: https://github.com/r/morph/compare/v0.37.1...v0.37.2
[0.37.1]: https://github.com/r/morph/compare/v0.37.0...v0.37.1
[0.37.0]: https://github.com/r/morph/compare/v0.36.0...v0.37.0
[0.36.0]: https://github.com/r/morph/releases/tag/v0.36.0
