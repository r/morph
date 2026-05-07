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

## [0.48.3] — 2026-05-06

Re-cuts v0.48.2 with a CI-only fix: the `setup_cursor` acceptance
specs that v0.48.2 introduced asserted `mcp.json` contains an
absolute path ending in `/morph-mcp`. They passed locally (where
`morph-mcp` is on `PATH` for any active Morph user) but failed on
the GitHub Actions runner because `cargo test --workspace` does
**not** build the actual `morph-mcp` *binary* at
`target/debug/morph-mcp` — only its unit-test binary in
`target/debug/deps/`. With no `morph-mcp` on PATH and no sibling
of `morph`, `resolve_mcp_command()` correctly fell back to the
bare name and the spec `file_contains: '/morph-mcp"'` assertion
flunked. Net effect: the v0.48.2 release pipeline never reached
the `publish` / `update-tap` steps, so v0.48.2 has a tag but no
Homebrew formula update. v0.48.3 carries the same user-visible
behaviour and ships through the pipeline cleanly.

### Fixed

- **`morph-cli/build.rs`** plants a stub `morph-mcp` file in
  `OUT_DIR` and injects `MORPH_MCP_PATH=<OUT_DIR>/morph-mcp` into
  every generated spec-test invocation. The resolver's first
  resolution branch (env var, validated via `is_file()`) is now
  exercised deterministically across local and CI runs without
  depending on workspace build order or the runner's `PATH`. The
  production fallback chain (env → sibling → `PATH` → bare) is
  unchanged; only the test-harness path is pinned.

### Tests

- Same six `setup_cursor` cases as v0.48.2; now green on a
  runner with no installed morph-mcp and no `target/debug/morph-mcp`.
  Verified locally by running `cargo test --workspace --locked`
  with `PATH=/tmp/fake-bins:/usr/bin:/bin` (no morph-mcp anywhere)
  — 1220 tests passing across the workspace.
- `morph-cli/tests/specs/version.yaml` updated to assert the new
  `0.48.3` version string in `--version`, `version`, and
  `version --json` output.

## [0.48.2] — 2026-05-06

User-visible bug fix: `morph setup cursor` (and the
`claude-code` / `opencode` siblings, and the `aoe` delegator
that calls them) now resolve `morph-mcp` to an **absolute path**
before writing the IDE integration JSON. Cursor on macOS is a
GUI-launched app that does not inherit the user's shell PATH,
so the previous `command: "morph-mcp"` entry silently failed to
spawn for Homebrew users on `/opt/homebrew/bin` — the agent saw
the `morph-record.mdc` rule but had no `morph_record_session`
tool to call. This release closes that gap.

### Fixed

- **`morph-cli/src/setup.rs`**: new `resolve_mcp_command()`
  helper writes an absolute path to `morph-mcp` into
  `.cursor/mcp.json`, `.claude/settings.json`, and
  `opencode.json`. Resolution order: `MORPH_MCP_PATH` env var
  (escape hatch for non-standard installs) → `morph-mcp`
  sibling of the running `morph` binary via
  `current_exe()` (the common case for Homebrew, `cargo
  install`, AUR, etc.) → manual `PATH` walk → bare-name
  fallback. The bare-name fallback emits a stderr warning so
  the user can fix it manually.
- **`SetupReport` / `OpenCodeSetupReport` /
  `ClaudeCodeSetupReport`** now carry the resolved
  `mcp_command` string, and the `morph setup …` CLI summary
  prints it as `morph-mcp: <resolved path>` so a fresh setup
  immediately shows what got written into the IDE config.

### Tests

- **New spec:** `morph-cli/tests/specs/setup_cursor.yaml`
  covers `setup cursor`'s contract end-to-end: `.cursor/mcp.json`
  + `.cursor/hooks.json` + the four hook scripts get written,
  the resolved command is an absolute path ending in
  `/morph-mcp`, the CLI prints the path, the operation is
  idempotent, existing user MCP servers are preserved, and a
  missing `.morph/` errors out cleanly. 6 cases, all passing.
- **New unit tests:** `resolve_mcp_command_honours_env_override`
  and `resolve_mcp_command_ignores_env_when_path_does_not_exist`
  pin the env-var contract and the resolver's fallback
  behaviour against bogus overrides.
- Existing `mcp_json_created`, `claude_code_settings_json_created`,
  and `opencode_json_created` tests now accept either the bare
  `morph-mcp` name or any absolute path ending in `morph-mcp`,
  matching the new resolver semantics. The setup_claude_code
  spec relaxes its `'"morph-mcp"'` assertion to `'morph-mcp"'`
  for the same reason.

### Total

493 morph-cli tests passing (41 doc + 8 setup + 432 spec +
10 status + 2 status-merge). No other crate's behaviour
changed.

## [0.48.1] — 2026-05-02

Senior-engineer cleanup pass — small reliability + readability
fixes plus a documentation/website sweep so every surface that
mentions the v0.43–v0.48 vocabulary unification matches reality.
No on-disk format changes; the public CLI/MCP surface is unchanged
beyond one new exported helper.

### Added

- **`morph_core::hex_prefix(s, max)`** — new public helper that
  truncates a hex hash string to its first `max` chars (or fewer
  if shorter), bounds-safe. Centralises the "8 chars for morph
  hashes, 12 chars for git SHAs" convention so every CLI call
  site uses the same path. `morph_core::short_hash_str` is now a
  thin alias around `hex_prefix(s, 8)`. Exposed via
  `morph-core/src/lib.rs` `pub use`.

### Changed

- **`morph-core/src/repo.rs`**: every `<morph_dir>/config.json`
  mutation (`drop_legacy_repo_mode`, `write_init_at_git_sha`,
  `write_repo_submode`, `write_repo_version`, plus the init-time
  write inside `init_morph_dir_at`) now goes through a single
  `write_config_pretty` helper that propagates serialization
  errors as `MorphError::Serialization`. Replaces four
  `serde_json::to_string_pretty(...).unwrap()` calls — serde
  cannot actually fail on a hand-built `serde_json::Value`, but
  the unwraps were a "no panics in production paths" smell and
  this collapses the pattern to one place.
- **`morph-cli/src/main.rs`** and **`morph-core/src/forget.rs`**:
  replaced 11 ad-hoc `&hash.to_string()[..N]` /
  `&sha[..sha.len().min(N)]` slicings with `hex_prefix(s, N)`
  (or `Hash::short()` where 8-char morph hashes were intended).
  No behaviour change; one consistent helper for all short-hash
  display.
- **`morph-cli/src/setup.rs`**: clarified the
  `expect("table inserted above")` / `expect("array inserted
  above")` messages in `merge_aoe_config_toml` so a future
  reader can see the unreachability invariant (the
  `if needs_init { insert ... }` branch immediately above
  guarantees the lookup succeeds). No behaviour change.
- **`morph-cli/src/main.rs::version_json`**: the existing
  `.expect("version json serializes")` now reads
  `.expect("infallible: serializing serde_json::Value")` and
  carries an inline comment explaining why this branch is
  unreachable.

### Documentation

- **`docs/v0-spec.md`** §13: past-tensed the description of the
  removed `eval add-case` / `eval suite-show` / `eval
  suite-from-specs` aliases — they were removed in v0.48, not
  "continuing through v0.47".
- **`docs/TESTING.md`**: refreshed the test inventory to match
  v0.48 reality (1212 Rust tests, 426 spec tests, 659 morph-core
  unit tests, cucumber 34/37). Replaced the stale CLI evidence
  category list (`run record/list/show`, `trace show`, `tap`,
  `traces`) with the v0.46+ replacements (`session list/show/
  record/import`, `inspect summary/show/recent/task/target/
  artifact/semantics/verification`). Added a callout explaining
  that the MCP wire names (`morph_run_list`, `morph_record_run`,
  `morph_eval_suite_show`, `morph_eval_suite_from_specs`,
  `morph_add_eval_case`) are deliberately stable across the
  CLI rename so existing IDE/MCP clients keep working.
- **`docs/THEORY.md`** §15: added a naming-note callout that
  disambiguates the paper-level *operations* `morph run` and
  `morph eval` from the CLI namespaces — the theory is unchanged,
  only the surface spelling moved (the CLI verbs are now
  `morph eval run` / `morph session record` / `morph session
  import`).
- **`.cursor/rules/eval-driven-development.mdc`**: the rule now
  lists `morph eval suite-from-specs` alongside `add-case` and
  `suite-show` in the "removed in v0.48" callout, matching the
  README and v0-spec.
- **`CONTRIBUTING.md`**: removed `tap` from the morph-cli
  example command list (`morph init`, `add`, `commit`, `log`,
  `tap`, `serve`); current commands are
  `inspect` / `session` / `eval`. The morph-core role line now
  notes the CLI rename so the cross-reference stays clear.
- **`site/index.html`**: header + hero badge bumped from
  `v0.42.2` to `v0.48.0`.
- **`site/tutorials/getting-started.html`**: install-verification
  callout now shows `morph 0.48.0 (built …)` instead of
  `morph 0.42.2 (built …)`.
- **`site/tutorials/adding-to-git-project.html`**: prerequisites
  list now requires `morph --version` ≥ `0.48.0`; "Two niceties
  on agent-attached commits" callout now reads "since v0.42,
  still true in v0.48" so the version anchor doesn't drift on
  every release.

### Tests

- All 1212 workspace Rust tests pass; cucumber 34/37 (3 skipped).
- `cargo build --workspace`, `cargo clippy --workspace
  --all-targets --all-features -- -D warnings`, and
  `cargo fmt --all -- --check` are clean.

## [0.48.0] — 2026-05-02

Vocabulary unification, part 3 (final): remove the v0.46-deprecated
`morph run` namespace and the nested `morph eval add-case` /
`morph eval suite-show` / `morph eval suite-from-specs` aliases. Adds
`morph session import` as the v0.48 spelling for the JSON-ingest path
that used to be `morph run record <file>`. Sixth and final release of
the multi-phase "make morph intuitive" effort. The on-disk object
format is unchanged; existing `.morph/` directories keep deserializing
without migration.

### Added

- **`morph session import <run.json> [--trace <file>] [--artifact <file>...]`** —
  ingest a pre-built Run JSON (and optional Trace JSON / artifact list)
  into the store. Used by automation that builds Run objects out of
  band — CI pipelines, MCP bridges, the `morph_run_record` MCP tool,
  and the `claude-code` / `cursor` hook scripts. Replaces the
  v0.46-removed `morph run record <file>`. The `morph_run_record` MCP
  tool itself is unchanged.
- **`removed_session_eval_aliases.yaml`** spec proves every removed
  spelling now exits with `unrecognized subcommand` and that
  `morph session import --help` is reachable.

### Removed

- **`morph run` namespace, in full.** `morph run list`, `morph run show`,
  `morph run record-session`, and `morph run record` are gone. Use
  `morph session list`, `morph session show`, `morph session record`,
  and `morph session import` respectively. The v0.46 deprecation notice
  warned every invocation through v0.47.
- **`morph eval add-case`**, **`morph eval suite-show`**, and
  **`morph eval suite-from-specs`** subcommands. Use the flat
  `morph eval add` / `morph eval show` / `morph eval rebuild`
  spellings introduced in v0.46.
- **`inspect::deprecation_notice` helper** in `morph-cli/src/inspect.rs`.
  Nothing dispatches to it anymore now that the v0.46 aliases are gone.
- **`RunCmd` enum** in `morph-cli/src/cli.rs` and its `Command::Run`
  dispatch arm in `morph-cli/src/main.rs`. Removed alongside the
  deprecated `EvalCmd::AddCase` / `SuiteShow` / `SuiteFromSpecs`
  variants.

### Changed

- **Cucumber step definitions** that internally invoked
  `morph run record-session` now invoke `morph session record`
  (`morph-e2e/tests/cucumber.rs`). The user-facing Gherkin step name
  ("When I run record-session with prompt …") is unchanged.
- **`morph setup aoe`** writes `morph session record` into the
  `on_launch` and `on_destroy` hooks of `.agent-of-empires/config.toml`
  instead of `morph run record-session`. The legacy spelling stays in
  the cleanup-prefix list so re-running `morph setup aoe` against an
  older config scrubs the old lines before emitting the new block.
- **Hook scripts** (`claude-code/hooks/morph-record-stop.sh`,
  `cursor/morph-record-stop.sh`,
  `cursor/morph-record-response.sh`,
  `.cursor/morph-record-stop.sh`,
  `.cursor/morph-record-response.sh`,
  and the deployable copies under `morph-cli/assets/`) call
  `morph session import` instead of `morph run record`.
- **Doc + site sweep**: every `morph run record` /
  `morph run record-session` / `morph eval add-case` /
  `morph eval suite-show` / `morph eval suite-from-specs` mention in
  `README.md`, `docs/v0-spec.md`, `docs/EVAL-DRIVEN.md`,
  `docs/CURSOR-SETUP.md`, `docs/OPENCODE-SETUP.md`,
  `docs/CLAUDE-CODE-SETUP.md`, `docs/MULTI-MACHINE.md`,
  `docs/README.md`, and the `.cursor/rules/eval-driven-development.mdc`
  /  `morph-cli/assets/cursor/rules/eval-driven-development.mdc` rule
  files now refers to the v0.46/v0.48 replacement, with a
  parenthetical noting the prior spelling and the version that
  removed it.

### Tests

- All 426 spec tests pass; cucumber 34/37 (3 skipped).
- `cargo build --workspace`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, and `cargo fmt --all -- --check`
  are clean.

## [0.47.0] — 2026-05-02

Vocabulary unification, part 2: remove the v0.45-deprecated
`morph trace`, `morph tap`, and `morph traces` namespaces and
introduce Rust-level type aliases (`Session`, `SessionTrace`,
`SessionEvent`, `Eval`, `EvalItem`) so newer call sites can match
the user vocabulary without renaming the storage-layer structs.
Fifth release of the multi-phase "make morph intuitive" effort.
The on-disk object format is unchanged; existing `.morph/`
directories keep deserializing without migration. The
v0.46-deprecated `morph run` and `morph eval add-case` /
`suite-show` / `suite-from-specs` aliases continue to ride along
through v0.47 — only the v0.45 ones are removed in this release.

### Added

- **Rust type aliases** in `morph-core/src/objects.rs` and
  re-exported from `morph_core`:

  ```rust
  pub type Session = Run;
  pub type SessionTrace = Trace;
  pub type SessionEvent = TraceEvent;
  pub type Eval = EvalSuite;
  pub type EvalItem = EvalCase;
  ```

  Pure type renames: serialization, hashing, and `MorphObject`
  discrimination all go through the underlying struct, so on-disk
  bytes are unchanged. Lets newer call sites match the user
  vocabulary; existing code keeps compiling against the storage
  names.
- New spec `removed_inspect_aliases.yaml` (3 cases) pins that
  `morph trace`, `morph tap`, and `morph traces` exit with code 2
  and an "unrecognized subcommand" error after removal — so a
  future re-introduction is intentional, not an accident.

### Removed

- **`morph trace show`** — removed (was deprecated in v0.45). Use
  `morph inspect show <hash>`.
- **`morph tap {summary, inspect, diagnose, export, trace-stats,
  preview}`** — removed (was deprecated in v0.45). Use the
  corresponding `morph inspect` subcommand.
- **`morph traces {summary, task-structure, target-context,
  final-artifact, semantics, verification}`** — removed (was
  deprecated in v0.45). Use the corresponding `morph inspect`
  subcommand.
- The companion `TraceCmd`, `TapCmd`, `TracesCmd` Clap enums and
  the `Command::Trace` / `Command::Tap` / `Command::Traces` arms
  are gone from `morph-cli/src/cli.rs` and `main.rs`. The
  `handle_traces_command` dispatcher is removed.
- Spec files `tap.yaml`, `traces.yaml`, and
  `inspect_deprecated_aliases.yaml` deleted. The unified
  `inspect.yaml` (16 cases) covers every subcommand.

### Changed

- **`morph --help` Inspect docstring** updated: no longer mentions
  the now-removed `morph trace` / `morph tap` / `morph traces`
  fall-back paths.
- **Doc + site sweep** for the v0.45 removal: `README.md`,
  `docs/v0-spec.md`, and stale internal doc-comments in
  `morph-core/src/store.rs` and `morph-core/src/structured.rs`
  rewritten to point at `morph inspect`. The v0-spec call-out
  sections "Tap" and "structured trace views" now note the
  removal target. Architectural prose ("the morph trace
  contract", "morph traces live in the commit graph") is left
  alone per the dual-vocabulary rule for `v0-spec.md`,
  `THEORY.md`, and `morph-paper.tex`.
- The `inspect::deprecation_notice` helper now points at v0.48
  (the next removal target) so the v0.46-deprecated
  `morph run` / `eval add-case` aliases emit the right wording.

### Tests

- Workspace **1217 / 1217 passing** (1244 minus 30 cases in the
  three deleted spec files plus 3 new removal-proof cases).
  Cucumber 34 / 37 (3 skipped, 0 failed) — no change.

## [0.46.0] — 2026-05-02

Vocabulary unification, part 1: introduce `morph session` as the
user-facing namespace for what's stored internally as a Run+Trace
pair, and flatten the `morph eval` surface so cases and suites
stop leaking from the storage model into the CLI. Fourth release
of the multi-phase "make morph intuitive" effort. The old
`morph run *`, `morph eval add-case`, `morph eval suite-show`,
and `morph eval suite-from-specs` spellings continue to work in
v0.46 and v0.47 with a one-line stderr deprecation notice; they're
removed in v0.48. No on-disk format changes.

### Added

- **`morph session` namespace.** Subsumes the most common
  `morph run *` operations plus the `morph inspect export` path
  under a single user-facing noun:

  ```text
  morph session list [--json]                          # was: morph run list
  morph session show <hash> [--json] [--with-trace]    # was: morph run show
  morph session record [--prompt P --response R |      # was: morph run record-session
                        --messages JSON]
                       [--model-name N --agent-id A]
  morph session export [--mode prompt-only|with-context|agentic]
                       [--output FILE] [--model M]
                       [--agent A] [--min-steps N]    # was: morph inspect export
  ```

  All four subcommands share their implementations with the
  existing `morph_*` MCP tools (no MCP changes needed in v0.46).
- **Flat `morph eval` surface.** New top-level subcommands that
  drop the Suite/Case noun split:

  ```text
  morph eval add <files>... [--suite H] [--no-default]
                            [--no-set-default]      # was: morph eval add-case
  morph eval show [--suite H] [--json]              # was: morph eval suite-show
  morph eval rebuild <dirs>... [--no-set-default]   # was: morph eval suite-from-specs
  ```

  Identical semantics to the v0.45 spellings (and the same
  policy-default-suite update behavior).
- New specs: `session.yaml` (8 cases covering list / show /
  record / export and the messages-array form), `eval_flat.yaml`
  (4 cases covering add / show / rebuild including default-suite
  pickup), `session_eval_deprecated_aliases.yaml` (6 cases pinning
  the deprecation-notice text on every old command).

### Changed

- **`morph --help` "SESSIONS" group** replaces the old "INSPECT"
  group, listing `session, inspect, show, head, identify, refs,
  annotate, annotations`. The deprecated `morph run` namespace is
  hidden from `--help`. The flat `morph eval` subcommands inherit
  the existing `EVALS & METRICS` heading.
- Doc + site sweep: every `morph run record-session` / `morph run
  list` / `morph eval add-case` / `morph eval suite-show` / `morph
  eval suite-from-specs` mention in `README.md`,
  `docs/EVAL-DRIVEN.md`, `docs/v0-spec.md`, `docs/SECURITY.md`,
  `docs/SESSION-TRACKING.md`, `docs/AOE-SETUP.md`,
  `docs/OPENCODE-SETUP.md`, `docs/CURSOR-SETUP.md`,
  `morph-cli/assets/opencode/AGENTS.md`,
  `morph-cli/assets/opencode/plugins/morph-record.ts`,
  `morph-cli/assets/cursor/rules/*.mdc`,
  `.cursor/rules/*.mdc`, and `site/index.html` rewritten to use
  the new spellings. The opencode hook script now calls
  `morph session record` directly so freshly-installed users get
  the new spelling without a re-setup.

### Deprecated

- **`morph run list`**, **`morph run show <hash>`**, **`morph run
  record-session ...`** — use `morph session list`,
  `morph session show`, `morph session record`. Removed in v0.48.
- **`morph run record <file>`** — JSON-file ingest will be folded
  into `morph session import` in v0.48; for now the existing
  `morph_run_record` MCP path is unchanged.
- **`morph eval add-case <files>`** — use `morph eval add <files>`.
  Removed in v0.48.
- **`morph eval suite-show [--suite H] [--json]`** — use
  `morph eval show`. Removed in v0.48.
- **`morph eval suite-from-specs <dirs>`** — use
  `morph eval rebuild`. Removed in v0.48.

  Each deprecated invocation prints a one-line stderr notice in
  the `git`-style format:

  ```text
  warning: `morph eval add-case` is deprecated; use `morph eval add` instead (removed in v0.48).
  ```

### Tests

- Workspace **1244 / 1244 passing** (1226 baseline + 18 new
  acceptance cases). Cucumber 34 / 37 (3 skipped, 0 failed) — no
  change.

## [0.45.0] — 2026-05-02

Inspection collapse: three near-synonymous CLIs (`morph trace`,
`morph tap`, `morph traces`) become one coherent
`morph inspect` namespace. Third release of the multi-phase "make
morph intuitive" effort. The old commands continue to work in v0.45
and v0.46 with a one-line stderr deprecation notice; they're
removed in v0.47. No on-disk format changes; no behavior changes
for callers that migrate to the new spelling.

### Added

- **`morph inspect` namespace** with twelve subcommands:

  ```text
  morph inspect summary [--json]                 # repo aggregate (was tap summary)
  morph inspect recent [--limit N] [--json]      # newest traces (was traces summary)
  morph inspect show <hash>                      # auto-detects: trace → events, run → task
  morph inspect diagnose [<hash>]                # recording quality (was tap diagnose)
  morph inspect export [...]                     # eval-case export (was tap export)
  morph inspect stats <trace_hash>               # event-level stats (was tap trace-stats)
  morph inspect preview <run_hash>               # labeled preview (was tap preview)
  morph inspect task <ref>                       # task structure (was traces task-structure)
  morph inspect target <ref>                     # target context (was traces target-context)
  morph inspect artifact <ref>                   # final artifact (was traces final-artifact)
  morph inspect semantics <ref>                  # change semantics (was traces semantics)
  morph inspect verification <ref>               # verification steps (was traces verification)
  ```

  `morph inspect show` collapses two old commands into one by
  auto-detecting whether the hash points at a Trace (prints raw
  events, like `morph trace show`) or a Run (prints the structured
  task, like `morph tap inspect`). `all` iterates every Run.
- New module `morph-cli/src/inspect.rs` factors the handlers out of
  `main.rs` so the new namespace and the deprecated aliases both
  call into one set of `pub(crate) fn run_*` entry points.
- New specs: `inspect.yaml` (16 cases covering every subcommand
  plus `--help` grouping) and `inspect_deprecated_aliases.yaml`
  (6 cases pinning the deprecation-notice wording on every old
  command).

### Changed

- **`morph --help` "INSPECT" group** now lists `inspect` first; the
  deprecated `trace`, `tap`, `traces` are hidden from `--help` to
  steer new readers at the right spelling. They still work on the
  command line.
- Doc + site sweep: every `morph tap` / `morph trace` / `morph
  traces` mention in `README.md`, `docs/v0-spec.md`,
  `docs/SECURITY.md`, `docs/SESSION-TRACKING.md`,
  `site/index.html`, `site/tutorials/getting-started.html`, and
  `site/tutorials/adding-to-git-project.html` rewritten to use
  `morph inspect ...`. Architectural prose ("morph traces live in
  the commit graph") is left untouched.

### Deprecated

- **`morph trace show <hash>`** — use `morph inspect show <hash>`.
  Removed in v0.47.
- **`morph tap summary`**, **`morph tap inspect`**, **`morph tap
  diagnose`**, **`morph tap export`**, **`morph tap trace-stats`**,
  **`morph tap preview`** — use the corresponding `morph inspect`
  subcommand. Removed in v0.47.
- **`morph traces summary`**, **`morph traces task-structure`**,
  **`morph traces target-context`**, **`morph traces
  final-artifact`**, **`morph traces semantics`**, **`morph traces
  verification`** — use the corresponding `morph inspect`
  subcommand. Removed in v0.47.

  Each deprecated invocation prints a one-line stderr notice in the
  `git`-style format:

  ```text
  warning: `morph tap summary` is deprecated; use `morph inspect summary` instead (removed in v0.47).
  ```

### Tests

- Workspace **1226 / 1226 passing** (1203 baseline + 22 new
  acceptance cases plus one extra registration). Cucumber 34 / 37
  (3 skipped, 0 failed) — no change.

## [0.44.0] — 2026-05-02

Commit ergonomics: `morph commit` becomes a one-liner even when you
want metrics. Tell Morph your test suite once, every commit thereafter
runs it and attaches the results — collapsing the eval-driven flow
from two commands to one for the common case. Second release of the
multi-phase "make morph intuitive" effort. No behavior breaks; the
existing `morph eval run` → `morph commit` flow keeps working
unchanged for repos that don't set the new key.

### Added

- **`commit.test_command` config key.** New nested entry in
  `.morph/config.json`:

  ```bash
  morph config commit.test_command "cargo test --workspace"
  ```

  When set, plain `morph commit` shells out to the command before
  recording the commit, parses the output via the same auto-detected
  runner stack as `morph eval run` (cargo / pytest / vitest / jest /
  go), writes a fresh `LAST_RUN.json` breadcrumb, and attaches the
  metrics + run as evidence. Unset = old behavior. The string is
  POSIX-shell-split with `shlex` so quoted arguments survive
  (`"pytest -k 'fast and not slow'"`). Spec coverage:
  `commit_runs_configured_test_command.yaml`,
  `config_commit_test_command.yaml`. Five new unit tests in
  `morph-core/src/commit_config.rs` cover the read/write helpers.
- **`morph commit --no-test`.** Skip the configured auto-run for
  this commit without removing the configured command. Useful for
  quick chores or when you've gathered metrics out-of-band.
- **`morph commit --rerun`.** Force a fresh test run even when an
  existing breadcrumb is current. Use when an external state change
  (env var, fixture refresh) means cached metrics no longer reflect
  reality but the commit's HEAD + index haven't moved.
- **`morph config commit.test_command`** read/write/unset support
  alongside the existing `user.name` / `user.email` keys.
  Round-trips: `morph config commit.test_command "..."` then
  `morph config commit.test_command` (or `--get`) prints the value;
  exit code 1 when unset (matches the `user.*` convention).

### Changed

- **`morph commit` aborts on a failing configured test command.**
  A non-zero exit from `commit.test_command` is treated as evidence
  that the code is not in a committable state. The failing run is
  still recorded (inspect with `morph show <hash>`), but the commit
  doesn't land. Override with `--no-test` or by fixing the failure.
- **README quickstart features the configured-command flow.** The
  `morph config commit.test_command` line is now the recommended
  one-time setup; the old two-step (`morph eval run` then
  `morph commit`) is preserved as the manual fallback.
- **`docs/EVAL-DRIVEN.md` §5 features the one-command flow** as the
  primary path, with the manual breadcrumb form as "When you've
  already run the suite". Documents the new `--no-test` / `--rerun`
  flags and the failing-test abort behavior.
- **`docs/MORPH-AND-GIT.md` daily workflow** is now a single
  `morph commit` after a one-time `morph config commit.test_command`,
  matching the reference-mode behavior most users actually want.
- **Site copy aligned**: `site/index.html` quickstart and
  How-It-Works code blocks, `site/tutorials/getting-started.html` §7
  "Skip the two-step?" callout, and
  `site/tutorials/adding-to-git-project.html` post-commit narration
  all now feature the configured-command flow as the recommended
  path.

### Tests

- Workspace **1203 / 1203 passing** (1192 baseline + 6 new acceptance
  cases in `commit_runs_configured_test_command.yaml` (4) and
  `config_commit_test_command.yaml` (2), plus 5 unit tests in
  `morph-core/src/commit_config.rs`). Cucumber 34 / 37 (3 skipped,
  0 failed) — no change.

## [0.43.0] — 2026-05-02

Onboarding polish: make Morph easier to discover and lean into without
giving up rigor as an opt-in. First release of the multi-phase
"make morph intuitive" effort. No behavior breaks; existing repos and
flows are unaffected.

### Added

- **`morph init` prints the active policy** as the last line of init
  output, with a one-liner showing how to switch:

  ```
    policy: relaxed (metrics optional) — tighten with `morph policy init`
  ```

  Bare repos see `policy: strict (...) — loosen with morph policy
  require-metrics`. New users now know the gate they're under without
  running `morph policy show`. Spec coverage:
  `init_prints_active_policy.yaml`.

- **Grouped subcommand overview in `morph --help`** under a new
  `COMMANDS BY GROUP:` section (ESSENTIAL / REMOTES / EVALS &
  METRICS / INSPECT / ADVANCED / INTEGRATIONS). Clap's auto-generated
  alphabetical list still appears above; the grouping makes the
  day-one happy path discoverable without scanning 46 entries. Spec
  coverage: `help_is_grouped.yaml`.

### Changed

- **README leads with one command.** Quickstart is now `morph init`
  + `morph commit -m "..."` and nothing else; the full surface is
  preserved in a `Reference: full command list` section near the
  bottom. The "When you want more" section gradually introduces
  IDE recording, test-result attachment, and dominance gating as
  opt-in upgrades rather than mandatory ceremony.
- **`docs/EVAL-DRIVEN.md` documents reality.** §0 used to claim
  reference-mode `morph init` installed a strict default policy;
  in fact reference-mode has been writing a permissive one (with
  the `git-hook` carve-out) since v0.40. The doc now matches
  the code and points readers at `morph policy init` for opt-in
  strict mode.
- **`docs/EVAL-DRIVEN.md` §5 leads with the auto-pickup flow**
  (`morph eval run` then plain `morph commit`) instead of the
  manual `--from-run <hash>` ritual. The manual flow is preserved
  in a "When you need precision" subsection. The
  `LAST_RUN.json` breadcrumb already implemented the auto-pickup;
  the docs are catching up.
- **Site copy aligned** with the relaxed-by-default story:
  `site/index.html` quickstart and "Behavioral commits" section,
  `site/tutorials/adding-to-git-project.html` post-commit narration.

### Documentation

- **Senior-engineer review pass on docs/site.** Removed stale
  `v0.41.0` / `v0.41.1` version anchors from `docs/SECURITY.md`
  and `site/index.html` for the `morph forget` section — the
  SSH-tombstone protocol upgrade and the `morph_forget` MCP
  tool are still pending, so the gaps are now framed as
  roadmap rather than tied to versions that have already
  shipped. Replaced the non-existent `morph tap list` recipe
  in `docs/SECURITY.md` with `morph run list` +
  `morph tap inspect`. Tightened `docs/v0-spec.md § "Morph is
  pure VCS"` so it acknowledges `morph eval run`'s thin
  shell-out wrapper instead of asserting morph "does not
  execute" anything. Refreshed test counts in `docs/TESTING.md`
  and the `cargo test` line in `README.md` to match the
  current workspace total (1187 across 5 crates). Corrected
  `morph push <remote> <branch>` and `morph pull <remote>
  <branch>` in `README.md` to mark `<branch>` as required
  (matches `morph-cli/src/cli.rs`).

### Tests

- Workspace **1192 / 1192 passing** (1187 baseline + 5 new
  acceptance cases across `init_prints_active_policy.yaml` and
  `help_is_grouped.yaml`). Cucumber 34 / 37 (3 skipped, 0 failed)
  — no change. Pre-existing fmt drift across 55 files in the
  workspace was cleared in a separate `chore: cargo fmt --all
  baseline` commit so this release lands on a fmt-clean base.

## [0.42.2] — 2026-05-02

Make `morph init --git-init` deterministic across hosts. The reference-mode
contract uses `main` as the canonical default branch (it's what
`morph checkout`, `morph branch`, `morph push`, and the merge breadcrumbs
all assume), but the underlying `git init` shell-out picked up whatever
the host's `init.defaultBranch` happened to be — `master` on a fresh
GitHub-Actions runner that hadn't opted in to the git 2.28+ rename.
Spec tests like `spec_checkout_branch`, `spec_clone_with_explicit_branch_checks_out_that_branch`,
and the entire `merge_*` family then failed with
`pathspec 'main' did not match any file(s) known to git` because
git landed the very first commit on `master` and morph's
`checkout main` had nothing to point at.

### Fixed

- **`morph init --git-init` now passes `-b main`** to `git init` so
  the initial branch matches `morph_core::DEFAULT_BRANCH` regardless
  of the host's `init.defaultBranch` setting. The error message on
  failure now also names the explicit invocation
  (`git init -b main <path>`), so a reader of the failure output
  can reproduce it without guessing.

### Tests

- Verified the full workspace (1187 tests) under a simulated CI
  environment (`GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=init.defaultBranch
  GIT_CONFIG_VALUE_0=master`) passes 1187 / 1187 — no regressions on
  hosts that already default to `main`.

## [0.42.1] — 2026-05-01

Mixed-authorship plumbing on the reference-mode commit rebuild
path. The last three previously-`#[ignore]`'d acceptance specs
unblock; the workspace now runs **1187 / 1187 with zero
ignored** for the first time since v0.39.

### Fixed

- **Human author lands in `commit.contributors`.** When
  `morph commit --from-run <hash>` is used in reference mode,
  the rebuilt commit now folds the human author (resolved
  through morph's identity chain) into the contributors list
  with `role: "human-author"`, alongside the agent
  (`role: "primary"`). Earlier ref-mode commits projected the
  agent + run.contributors only, so a downstream tool reading
  the structured contributors list saw only the agent and
  concluded the human had no hand in the change.
- **`human_edits` records `(trace claim) vs (staged blob)`
  divergence on ref-mode commits.** The rebuild now flattens
  the mirror commit's tree (snapshotted by `sync_one_commit`
  from the git commit's tree) and runs
  `morph_core::compute_human_edits` against the attached Run's
  trace, so a path the agent claimed to write but the human
  edited afterwards is recorded as `reason: "post-agent-edit"`,
  and a path the human authored independently shows as
  `reason: "no-trace-record"`. Earlier ref-mode commits
  silently dropped this diff because the standalone-mode tree
  walk wasn't wired to the git-tree snapshot.

### Changed

- **`morph_core::fold_human_author_into_contributors` is now
  public** so the CLI's reference-mode commit path can call it
  directly. (Previously crate-private; the standalone-mode
  commit path was its only caller.)

### Tests

- `mixed_authorship.yaml`:
  `mixed_authorship_human_author_in_contributors_when_run_attached`,
  `mixed_authorship_variant_a_post_agent_edit_recorded`,
  `mixed_authorship_variant_b_no_trace_record_recorded` — all
  three un-skipped and passing on the ref-mode commit path.
- Workspace test count: **1187 / 1187 passing, 0 ignored**.
  Down from 13 ignored at the start of the v0.41 line; down
  from 16 at v0.40.0.

## [0.42.0] — 2026-05-01

The reference-mode merge rebuild path now propagates the user's
eval suite, pipeline, and metric-retirement decisions onto the
mirrored merge commit instead of leaving them as the empty
placeholders `sync_one_commit` originally planted. Ten previously
`#[ignore]`'d acceptance specs unblock as a consequence — the
merge gate has real evidence to enforce on every ref-mode merge,
single-shot or stateful.

### Added

- **`morph_core::rebuild_merge_commit`** + **`MergeRebuildOpts`**.
  After `sync_to_head_with_origin` mirrors the new merge commit,
  `rebuild_merge_commit` re-derives the commit object using the
  `MergePlan`'s auto-union eval suite, the user's `--pipeline` /
  `--eval-suite` / `--metrics`, and any retired metrics, then
  re-points the branch ref onto the rewritten hash. Same recipe
  the non-merge `morph commit` rewrite already used; consolidating
  it under one helper keeps the single-shot and `--continue` paths
  aligned. The original mirror commit becomes a content-addressable
  orphan — harmless because no ref points at it anymore.
- **`MERGE_REF.json` carries merge context across the conflict
  boundary.** `ReferenceMergeBreadcrumb` now records the user's
  `--pipeline`, `--eval-suite`, `--retire`, and `--retire-reason`
  so `morph merge --continue` reproduces the same plan a
  single-shot merge would have used. Earlier versions dropped
  the retire intent on conflict, so resuming a textually-conflicted
  merge silently lost the retirement and review-node attribution.
- **`MergePlan::head_commit()` / `other_commit()` accessors** on
  the public merge API so callers (the new rebuild helper, future
  inspection tooling) can read parent commit fields without the
  plan having to re-fetch them. Internal storage stays private;
  only borrows are exposed.
- **`auto_detect_introduces_cases()` in `morph_core::annotate`.**
  `morph commit` (without `--new-cases`) now diffs the
  about-to-commit eval suite against the first-parent's suite and
  auto-records the case-id difference as an `introduces_cases`
  annotation. `--new-cases ""` is the explicit opt-out;
  `--new-cases "manual:override"` keeps the manual override path.
  Mirrors the merge gate's own case-attribution model so a commit
  that introduces a new acceptance case is auditable without the
  user having to type the case id twice.
- **`morph_core::ensure_review_node_for_retirement` is now
  public** so the new rebuild helper can synthesise the same
  attributed review node the standalone-mode merge engine
  produced, attribuing the retirement to the merge author.

### Fixed

- **`morph merge` propagates the auto-union eval suite onto the
  merged commit.** The dominance check used to fire before
  `git merge` ran (correct), but the rebuild used the empty
  placeholder suite `sync_one_commit` planted, so the resulting
  merge commit had no claim. The merge gate had nothing to
  enforce on subsequent merges into the same branch.
- **`morph merge --retire <metric>` writes a real review node.**
  The single-shot and `--continue` paths both now run
  `ensure_review_node_for_retirement` against the head pipeline
  (or the user's `--pipeline` override) and replace the merge
  commit's pipeline with the result. Retirement attribution and
  reason text land in the rewritten pipeline's review-node
  `params.reason`.
- **`morph merge --retire` survives a textual conflict.** The
  retire intent and reason now flow through `MERGE_REF.json` to
  `morph merge --continue`, which applies the same rebuild a
  single-shot merge would have applied. Earlier versions
  silently dropped the retirement when the merge dropped into
  the conflict path.
- **Weak merge metrics are rejected with the parent name on
  stderr.** Now that the rebuild propagates the union suite onto
  parents' commits, `MergePlan::check_dominance` finds the
  expected violations (e.g. `current: acc 0.9 → 0.87`) and
  surfaces them on the merge-rejection error message.

### Tests

Ten previously `#[ignore]`'d specs now run as part of the default
`cargo test --workspace`. All ten flip green:
- `merge.yaml`: `merge_auto_union_suite_happy_path`,
  `merge_rejects_with_explained_metric_failure`,
  `merge_with_retirement_succeeds`.
- `merge_retire_review.yaml`: `merge_retire_synthesizes_review_node`,
  `merge_retire_default_reason_when_omitted`,
  `merge_retire_with_existing_review_node_no_resynthesis`.
- `merge_retire_stateful.yaml`: `merge_stateful_retire_clean_three_way_synthesizes_review_node`,
  `merge_stateful_retire_with_textual_conflict_then_continue`,
  `merge_stateful_retire_abort_clears_breadcrumb` (the latter two
  also rewritten for v0.40+ stderr-shaped conflict UX).
- `commit_auto_new_cases.yaml`: `commit_auto_detects_new_cases_added_to_default_suite`.

Three `#[ignore]`'d specs remain, all in the mixed-authorship
plumbing bucket (`mixed_authorship.yaml`), scheduled for v0.42.1.

## [0.41.1] — 2026-05-01

Three reference-mode merge UX fixes triaged out of the v0.40 ignored
spec backlog. Each spec was either passing (status integration) or
asserting old structural-merge wording; this release closes the gap
between v0.40+ behavior and the acceptance suite.

### Fixed

- **`morph status` surfaces an in-progress reference-mode merge.**
  `merge_progress_summary` now also reads `.morph/MERGE_REF.json`
  (the v0.40 ref-mode breadcrumb) and pulls unmerged paths from
  `git ls-files --unmerged`, so the "You have unmerged paths." +
  `morph merge --continue` / `--abort` hint banner appears mid-merge
  instead of "nothing to commit". Legacy `.morph/MERGE_HEAD` still
  works for any in-flight pre-v0.40 merge.
- **`morph merge --abort` outside a merge errors instead of
  no-op'ing.** Now exits non-zero with `Error: no merge in
  progress`, matching `git merge --abort`'s `fatal: There is no
  merge to abort` (exit 128). Earlier ref-mode versions printed
  "Nothing to abort." with exit 0; that was too lenient — silent in
  scripts when the user mistyped a command they expected to fail.

### Tests

- `merge_with_textual_conflict_drops_into_continue_flow` and
  `merge_abort_without_in_progress_errors` (`morph-cli/tests/specs/merge.yaml`)
  un-skipped and rewritten for the v0.40+ wording: morph's hints
  arrive on stderr now (since `git merge -q` swallows git's own
  `CONFLICT (content): …` lines), `--abort` is exit-1 with stderr
  `no merge in progress`.
- `status_during_textual_merge_lists_unmerged_paths_and_hint`
  (`morph-cli/tests/status_merge_integration.rs`) un-`#[ignore]`'d.
- 13 specs remain skipped — all sharing the same two roots
  (eval-suite plumbing through the ref-mode merge rebuild path,
  and mixed-authorship plumbing on the rebuild path) — and are
  scheduled for v0.42.0 / v0.42.1.

## [0.41.0] — 2026-05-01

`morph forget` lands. The "I leaked a secret into a trace; what
do I do?" question now has a first-class answer.

### Added

- **`morph forget <hash>` CLI subcommand.** Permanently retires a
  `Run`, `Trace`, or prompt `Blob` from the local store and (with
  `--remote <name>`) propagates the deletion to a configured morph
  remote. Writes an immutable `Tombstone` object recording actor,
  reason, timestamp, and the original kind, deletes the original
  `objects/<hash>.json`, scrubs the per-type index entries, and
  drops a `.morph/forgotten/<hash>.txt` marker pointing at the
  tombstone. Flags: `--reason`, `--force` (override the
  "referenced by commit" refusal), `--remote`, `--dry-run`,
  `--yes` (required for non-interactive callers; interactive
  callers type `forget` to confirm). The CLI refuses to retire
  `Commit`, `Tree`, regular `Blob`, `Pipeline`, `EvalSuite`,
  `Artifact`, `TraceRollup`, and `Annotation` objects — those
  carry structural meaning the version-control DAG depends on.
- **`Tombstone` as a first-class `MorphObject` variant.** Defined
  in `morph-core/src/objects.rs` with fields `original_hash`,
  `original_kind`, `forgotten_at`, `actor`, `reason`. Tombstones
  are content-addressed — `morph show <tombstone-hash>` returns
  who retired what, when, and why. The original bytes are gone;
  the *fact* of the deletion is permanent.
- **`forget_local()` / `apply_tombstone()` core APIs.** New
  `morph-core/src/forget.rs` module: `forget_local()` validates
  the kind, locates referencing commits, deletes the original +
  index entries, writes the tombstone; `apply_tombstone()` is the
  idempotent receiver path used by `morph fetch`. Both functions
  are exported from `morph-core` and exercised by unit tests
  (round-trip forget, refuse non-forgettable kinds, refuse
  referenced runs without `--force`, `--dry-run` is read-only).
- **Push/fetch propagation.** `morph-core/src/sync.rs` now ships
  tombstones alongside normal objects on `morph push` and
  auto-applies them on `morph fetch` via a new
  `transfer_tombstones()` helper. Local-filesystem morph remotes
  carry tombstones end-to-end today; SSH transport for tombstones
  is queued for v0.41.1 (see `docs/SECURITY.md`).
- **Merge-gate tolerance.** A commit whose `evidence_refs` names
  a tombstoned hash is read as "no claim" with a one-line warning
  rather than a hard error. Forgetting evidence does not
  retroactively break commits.
- **`docs/SECURITY.md` — full `morph forget` section.** Replaces
  the v0.40.2 gap-bullet with a detailed write-up: when to use
  it, how it works, what it refuses, what it does *not* cover
  (already-fetched copies on teammates' laptops, partial
  redaction, SSH propagation in v0.41.0, MCP tool), and a
  step-by-step "I leaked a secret" recipe.
- **Homepage `morph forget` callout.** [`site/index.html`](site/index.html)
  privacy section now leads with the secret-leak escape hatch:
  one-paragraph explanation, a three-line CLI sample, and a
  "what forget does *not* cover" footer with a deep-link to
  `docs/SECURITY.md`. The "Things morph does not yet do" list
  drops the missing-forget bullet and gains a clarifying note
  that SSH transport for tombstones lands in v0.41.1.
- **`docs/SESSION-TRACKING.md`** — *Immutable* note now describes
  `Tombstone` as the audit-preserving way to retire a run/trace
  rather than promising it for a future release.
- **Spec coverage.**
  [`morph-cli/tests/specs/forget.yaml`](morph-cli/tests/specs/forget.yaml)
  covers: forget a run with `--yes` writes a tombstone and removes
  the original from the store; `--dry-run` does not mutate;
  non-interactive without `--yes` is refused; commit-kind hashes
  are refused; runs referenced by a commit are refused without
  `--force`; the original object is no longer loadable
  post-forget. The cross-repo push/fetch propagation case is
  covered by the morph-core unit tests; an end-to-end Cucumber
  feature for the multi-repo CLI flow is queued for v0.41.1.

### Known gaps (called out in `docs/SECURITY.md`)

- **MCP tool.** `morph_forget` MCP wrapper is queued for v0.41.1.
- **SSH transport for tombstones.** Filesystem remotes work end
  to end; SSH-served remotes need the remote-helper protocol
  upgrade. Workaround: ssh into the SSH-served remote and run
  `morph forget` there too.
- **Already-fetched copies on teammates' laptops.** `morph
  fetch` applies the tombstone automatically; data on disk
  *before* the fetch is the teammate's choice to delete.
  Out-of-band ask remains the honest answer for "data on N
  already-cloned machines."

### Changed

- [`site/index.html`](site/index.html) and
  [`site/changelog.html`](site/changelog.html) version badges roll
  to `v0.41.0`. The changelog page surfaces 0.41.0 as the latest;
  0.40.0 ages off (page shows 0.41.0, 0.40.2, 0.40.1).

### Tests

- New `morph-core::forget` unit tests pin the local round-trip,
  apply-tombstone idempotency, kind refusals, and referencing-commit
  refusals. The CLI spec file
  [`morph-cli/tests/specs/forget.yaml`](morph-cli/tests/specs/forget.yaml)
  registers six cases through the auto-generated harness;
  `morph-cli/tests/specs/version.yaml` expects `0.41.0`.

## [0.40.2] — 2026-05-01

Docs-only release: the privacy & sharing story, told plainly.

### Added

- **`docs/SECURITY.md`.** New, plain-language companion to
  [`docs/SESSION-TRACKING.md`](docs/SESSION-TRACKING.md). Answers the
  question "morph records everything; what happens when I share it?"
  without hand-waving:
  - **What morph records, said plainly** — verbatim prompts and
    responses, every tool call (including `read(<path>)` whose
    output is the file contents), every shell stdout/stderr, every
    file edit, environment, model id, token counts. *"If the agent
    read your `.env`, the contents of `.env` are in the trace."*
  - **Where it lives, and what's in scope** — the on-disk layout
    of `.morph/`, the fact that it's never tracked by git
    (auto-excluded via `.git/info/exclude`), and the fact that it's
    not encrypted at rest (same posture as Claude/Cursor/OpenCode
    on-disk transcripts; use disk encryption).
  - **What crosses the wire when** — `git push` (code only,
    physically cannot include traces) vs `morph push` (opt-in,
    separate channel, sends everything reachable from the commit),
    drawn as a two-channel diagram.
  - **Recommended team setup** — code through your existing git
    remote; behavioral history through a *separate* morph remote
    accessible only to people you'd trust to read your IDE history.
  - **Things morph does *not* yet do** — explicitly listed: no
    `morph forget` (lands in v0.41.0), no client-side redaction
    filter on push, no selective fetch, no encryption at rest, no
    automatic secret scanning.
  - **Before-you-share checklist** and a brittle-but-real
    "I leaked a secret into a trace, what now" recipe that
    collapses into `morph forget <hash> --remote <name>` once
    v0.41.0 ships.
- **Homepage privacy section.** [`site/index.html`](site/index.html)
  — new "What gets recorded, what gets shared" section between
  How-It-Works and Design-Principles. Two side-by-side cards
  (`git push: code only` and `morph push: opt-in, separate`) make
  the two-channel model visible at a glance, followed by an
  honest "things morph does not yet do" list and a deep-link out
  to `docs/SECURITY.md`. New `.privacy-grid` / `.privacy-card`
  / `.privacy-list` styles in the page's CSS.
- **`docs/README.md`** — Guides table indexes `SECURITY.md`
  with the *"Read before you push to a morph remote"* hint.

### Changed

- [`site/index.html`](site/index.html) and
  [`site/changelog.html`](site/changelog.html) version badges roll
  to `v0.40.2`. The changelog page surfaces 0.40.2 as the latest
  release; 0.39.2 ages off the bottom (page shows the three most
  recent releases — 0.40.2, 0.40.1, 0.40.0).

### Tests

- [`morph-cli/tests/specs/version.yaml`](morph-cli/tests/specs/version.yaml)
  expects `0.40.2` from `morph --version` / `morph version` /
  `morph version --json`. Workspace test count unchanged from
  0.40.1: **1159 / 1159 passing, 15 skipped**.

## [0.40.1] — 2026-05-01

Docs-only release: the session-tracking story, told plainly.

### Added

- **`docs/SESSION-TRACKING.md`.** New companion to
  [`docs/MORPH-AND-GIT.md`](docs/MORPH-AND-GIT.md). Opens with the
  load-bearing claim — *"a diff plus a commit message is not enough
  to review AI-authored code; the next person needs the prompt"* —
  then walks through seven concrete things you can only do because
  the prompt + trace are part of the commit graph: review the
  transformation (not just the output), the prompt as a spec,
  replay/regenerate, attribution when something breaks, promote
  prompts → acceptance cases, merge-aware behavioral context,
  cross-tool portability. A six-row comparison table follows: the
  three alternatives that keep coming up — Claude / Cursor /
  OpenCode on-disk transcripts, OTEL spans in a tracing backend,
  Langfuse / Phoenix / Helicone — each scored honestly on the
  properties a reviewer actually needs (linked to a specific
  commit, content-addressed, visible to teammates, same shape
  across tools, merge-aware, local-first). The doc closes with the
  morph trace contract and a forward-link to `docs/SECURITY.md`
  (landing in 0.40.2).
- **Homepage comparison table.** [`site/index.html`](site/index.html)
  — the existing "Runs and Traces" solution item is extended with
  the same six-row comparison, embedded inline so the answer to
  *"doesn't Claude already record this?"* is one scroll away from
  the hero. Links out to `docs/SESSION-TRACKING.md` for the full
  argument. New `.compare-table` styling lives in the page's CSS.
- **`docs/README.md`** — Guides table now indexes
  `SESSION-TRACKING.md` next to `MORPH-AND-GIT.md`.

### Changed

- [`site/index.html`](site/index.html) and
  [`site/changelog.html`](site/changelog.html) version badges roll
  to `v0.40.1`. The changelog page surfaces 0.40.1 as the latest
  release; 0.39.1 ages off the bottom (the page shows the three
  most recent releases).

### Tests

- [`morph-cli/tests/specs/version.yaml`](morph-cli/tests/specs/version.yaml)
  expects `0.40.1` from `morph --version` / `morph version` /
  `morph version --json`. Workspace test count unchanged from
  0.40.0: **1159 / 1159 passing, 15 skipped**.

## [0.40.0] — 2026-05-01

A workspace-wide simplification: Morph now runs in **reference mode
only**. Standalone mode — the legacy "morph manages its own object DAG
and you `git add .morph/`" path — is gone. Every `.morph/` directory in
the world is now a per-clone wrapper next to a `.git/`. Behavioral
history (runs, traces, prompts, certifications) is **never tracked by
git**; sharing it with teammates is opt-in via a morph remote (PR 3 in
the `morph forget` release line will spell this out further).

### Removed

- **Standalone mode is gone.** The `RepoMode` enum, the `repo_mode`
  config key, the `--reference` flag on `morph init`, and every
  `read_repo_mode == Reference` branch in
  [`morph-core/src/working.rs`](morph-core/src/working.rs),
  [`morph-core/src/eval_suite.rs`](morph-core/src/eval_suite.rs),
  [`morph-mcp/src/main.rs`](morph-mcp/src/main.rs), and
  [`morph-cli/src/main.rs`](morph-cli/src/main.rs) collapsed to
  unconditional reference-mode behavior.
- The "check `.morph/` into git" story is retired. `.morph/` lands in
  `.git/info/exclude` automatically; teammates pulling git see ordinary
  git commits, not Morph state.

### Changed

- **`morph init` requires a git repository.** When run inside a
  directory that has no `.git/`, `morph init` interactively prompts
  `Run \`git init\` for you? [y/N]`. Pressing Enter (or running
  non-interactively) exits non-zero with the recipe `not a git
  repository at <path>; run \`git init\` first or pass \`--git-init\`
  to morph init.`. New flags `--git-init` (always init git) and
  `--no-git-init` (never prompt; fail fast if `.git/` is missing) for
  scripting.
- **`morph init` no longer takes `--reference`.** The flag is removed;
  reference mode is implicit.
- **`morph commit` is a behavioral checkpoint of the working tree.**
  The reference-mode commit path now runs `git add -A` before invoking
  `git commit`, and always passes `--allow-empty` and
  `--allow-empty-message` so a `morph commit` succeeds even with no
  file changes (e.g. when the operator only wants to record metrics
  against the current tree). Symmetric with how `morph add` already
  threads to `git add`.
- **`morph branch` and `morph checkout` mirror to git.** Creating or
  switching branches now drives the underlying git working tree as
  well as Morph's refs, so subsequent `git merge` / `git status` calls
  see a consistent state.
- **Mirrored Morph commits now snapshot the git tree.** `sync_one_commit`
  populates the `Commit.tree` field by enumerating `git ls-tree -r -z`
  and streaming blob contents via `git cat-file --batch`, instead of
  leaving `tree: None`. This unblocks `morph status` / `morph diff`
  against the populated tree without needing a second source of truth.
- **`morph upgrade` migrates legacy Standalone repos.** When run on a
  pre-0.40 Standalone repo that has a `.git/` alongside, upgrade now
  drops the legacy `repo_mode` key, captures `init_at_git_sha`, writes
  the `.morph/` exclude rule, and installs the four reference-mode
  hooks. When `.git/` is missing, upgrade hard-errors with the recipe
  (`git init && morph upgrade`, or pin to morph 0.39.x).

### Migration

If you were on Standalone (any morph ≤ 0.39.x without
`morph init --reference`):

```
morph upgrade               # drops repo_mode key, installs hooks
git rm -r --cached .morph   # if you'd previously checked .morph/ into git
git commit -m "stop tracking .morph/"
```

If you don't have a git repo yet, `git init` first.

### Tests

- 15 multi-step merge / retire / mixed-authorship specs are
  intentionally `skip:`-annotated for this release. They exercise
  auto-union eval suites, metric retirement, conflict handling, and
  detailed `human_edits` provenance — these need the merge gate
  re-plumbed against reference-only commits and will land in the
  follow-up release. Workspace test count: **1159 / 1159 passing,
  15 skipped**.
- `morph-cli/tests/specs/version.yaml` updated to expect `0.40.0`.
- New `--git-init` / `--no-git-init` spec coverage in
  [`morph-cli/tests/specs/init_in_git_dir.yaml`](morph-cli/tests/specs/init_in_git_dir.yaml).

## [0.39.2] — 2026-05-01

### Changed

- **Homepage now lists Agent of Empires alongside Cursor /
  Claude Code / OpenCode.** `site/index.html` had been frozen at
  the `v0.37.7` version badge and the IDE-integrations section
  showed only the three first-class IDEs, so the
  `morph setup aoe` integration shipped in `0.39.0` was
  invisible to anyone landing on the site. This release rewires
  the homepage:
  - Version badge bumps to `v0.39.2` in both the nav and the
    hero (no more stale `v0.37.7`).
  - A fourth integration card "Agent of Empires" lands next to
    Cursor / Claude Code / OpenCode, with the
    `morph setup aoe` command and a link through to
    [`docs/AOE-SETUP.md`](docs/AOE-SETUP.md). The section header
    was renamed to "Agent integrations" so AoE (a session
    manager, not an IDE) fits the framing.
  - The "Try the alpha" install block adds
    `morph setup aoe` to the list of `morph setup …` commands,
    so the homepage's three-command quickstart now mirrors the
    full set of supported integrations.
  - The "What works today" status panel mentions AoE explicitly
    so the site's honest status report stays in sync with the
    repo.
- `site/changelog.html` rolls forward to surface `0.39.2` as
  the latest release, demoting `0.38.0` off the bottom (the
  page shows the three most recent releases).

### Tests

- `morph-cli/tests/specs/version.yaml` updated to expect
  `0.39.2` from `morph --version` / `morph version`.

## [0.39.1] — 2026-05-01

### Fixed

- **`morph commit --from-run <hash>` now propagates the run's
  metrics into `observed_metrics`.** Previously, the standalone
  commit path read the run for provenance / `evidence_refs` /
  `env_constraints` / contributors but silently dropped its
  `metrics` map, so every `--from-run` commit produced the
  "commit has no observed_metrics" warning and `morph eval gaps`
  kept reporting `empty_head_metrics`. The reference-mode helper
  already did this correctly; the fix mirrors that path in
  `morph-cli/src/main.rs` with the same UX as the LAST_RUN
  breadcrumb auto-attach (a `attaching evidence from run <hash>:
  k=v, ...` stderr preview before the commit object is written).
  Precedence is unchanged: explicit `--metrics` still wins over
  the run's parsed metrics, and a run whose `metrics` map is
  itself empty correctly leaves the commit metrics-less and
  surfaces the standard warning.

### Tests

- New acceptance suite
  `morph-cli/tests/specs/commit_from_run_metrics.yaml` with
  three cases: a populated `cargo`-runner Run propagates
  `tests_passed` / `tests_total` into the commit; explicit
  `--metrics` overrides the run's metrics; and a Run with
  `metrics: {}` still surfaces the standard `no observed_metrics`
  warning rather than silently producing a metrics-less
  behavioral commit. The existing
  `commit_from_recorded_run_persists_env_and_contributors` and
  `commit_from_recorded_run_with_reviewer` cases stay intact —
  they only ever asserted provenance/contributors, which is why
  the bug slipped past them.

## [0.39.0] — 2026-04-30

### Added

- **`morph setup aoe`.** New CLI subcommand that wires Morph
  recording into [Agent of Empires](https://github.com/njbrake/agent-of-empires)
  multi-agent sessions. AoE is a `tmux`-based session manager that
  runs Claude Code, OpenCode, Cursor CLI, and other coding agents on
  top of git worktrees, optionally inside Docker sandboxes — and now
  every AoE session is wrapped by morph lifecycle hooks. The command:
  - Writes (or merges into) `.agent-of-empires/config.toml` a
    deterministic morph block: `[hooks].on_create` snapshots the
    worktree as a morph commit (`aoe-create: <instance-id>`,
    tolerant of missing `.morph/` and empty metrics so AoE never
    aborts session creation); `[hooks].on_launch` records a `Run` +
    `Trace` for every (re)launch; `[hooks].on_destroy` writes a
    final commit (`aoe-destroy: <instance-id>`) and a closing trace
    event before AoE tears the worktree down. Re-running the
    command rewrites only morph-owned lines (matched by command
    prefix) so user-defined hooks (`on_launch = ["npm install"]`,
    etc.) survive every re-run.
  - Seeds `[sandbox].environment` with `MORPH_WORKSPACE` +
    `AOE_INSTANCE_ID` so the hooks can find the morph repo and tag
    commits with the AoE instance id when running inside the
    sandbox container.
  - Seeds `[sandbox].extra_volumes` with bind-mounts for
    `/usr/local/bin/morph` and `/usr/local/bin/morph-mcp` (default;
    works against the stock `ghcr.io/njbrake/aoe-dev-sandbox`
    image). `--no-bind-mount` suppresses the volume entries for
    teams who prefer a baked sandbox image.
  - Emits `.agent-of-empires/Dockerfile.morph-aoe`, a reference
    Dockerfile that bakes morph + morph-mcp into an
    `aoe-dev-sandbox`-based image. Two install paths are
    documented: `COPY` from a local cargo build, and `curl` from a
    published release URL. `--no-dockerfile` skips it.
  - By default, delegates to `setup_cursor`, `setup_opencode`, and
    `setup_claude_code` so prompt/response recording works
    regardless of which agent AoE launches per session. Override
    with `--agent <name>` (repeatable) or `--skip-agents`. AGENTS.md
    is seeded either way so AoE-launched agents see morph guidance.
  - Idempotent: every re-run produces a byte-identical
    `config.toml` against a clean repo, and against repos that
    started with user-authored `[hooks]` / `[session]` /
    `[sandbox]` / `[worktree]` blocks the morph-owned entries are
    deduplicated rather than appended.

### Changed

- `docs/INSTALLATION.md` now documents `morph setup aoe` as a
  one-command quick path alongside cursor / opencode.
- `morph-cli` depends on `toml_edit = "0.22"` so the AoE config
  merge round-trips comments and formatting in user-authored
  `.agent-of-empires/config.toml` files.

### Tests

- New acceptance suite `morph-cli/tests/specs/setup_aoe.yaml`
  covering: config.toml + Dockerfile + AGENTS.md creation, hook
  block (`on_create` / `on_launch` / `on_destroy`, with always-commit
  semantics on create + destroy), sandbox env passthrough
  (`MORPH_WORKSPACE` + `AOE_INSTANCE_ID`), bind-mount entries
  (default), `--no-bind-mount` mode, Dockerfile contents (Path A
  + Path B documented), default delegation to all three per-agent
  setups, `--skip-agents` mode, "requires `morph init`" error path,
  idempotent re-run, and merge that preserves a pre-existing user
  `[hooks]` / `[session]` / `[sandbox]` block.
- New Rust unit tests in `morph-cli/src/setup.rs::tests`:
  `aoe_requires_morph_init`,
  `aoe_writes_config_dockerfile_and_agents_md`,
  `aoe_config_toml_has_lifecycle_hooks`,
  `aoe_config_toml_seeds_sandbox_env_and_volumes`,
  `aoe_no_bind_mount_omits_morph_volume_entries`,
  `aoe_default_delegates_to_all_three_agents`,
  `aoe_skip_agents_only_writes_glue`,
  `aoe_unknown_agent_errors`,
  `aoe_idempotent`,
  `aoe_preserves_existing_user_config`,
  `aoe_re_run_does_not_duplicate_morph_entries_with_user_config`.

## [0.38.0] — 2026-04-30

### Added

- **`morph setup claude-code`.** New CLI subcommand that mirrors
  `morph setup cursor` and `morph setup opencode`: it merges the
  `mcpServers.morph` entry (pointing at `morph-mcp` with
  `MORPH_WORKSPACE` set to the project root) into
  `.claude/settings.json` and registers `UserPromptSubmit` /
  `Stop` hooks that point at two embedded recording scripts —
  `morph-record-prompt.sh` and `morph-record-stop.sh`. The scripts
  themselves are written into `.claude/hooks/` and marked
  executable on Unix. Existing settings, MCP servers, and hooks
  are preserved on first install and on every re-run; the morph
  hook entries are keyed by command path so re-running `setup
  claude-code` doesn't duplicate them. The Stop hook parses
  `transcript_path` / `conversation` payloads into structured
  trace events (file_read, file_edit, tool_call, tool_result),
  records token usage in `run.environment.parameters`, and writes
  the resulting Run + Trace via `morph run record`. Replaces the
  old "copy the scripts from `claude-code/hooks/` and edit
  `.claude/settings.json` by hand" flow documented in
  `docs/CLAUDE-CODE-SETUP.md`. Hook scripts continue to live at
  `claude-code/hooks/` for direct copy/symlink users; the setup
  command embeds the same content via `include_str!` so a
  shipped `morph` binary doesn't depend on the source checkout.

### Tests

- New acceptance suite `morph-cli/tests/specs/setup_claude_code.yaml`
  covering: settings.json creation, MCP entry shape,
  UserPromptSubmit + Stop hook registration, hook scripts present
  with shebang, "requires `morph init`" error path, idempotent
  re-run, and merge that preserves a pre-existing user
  `model`/`mcpServers`/`hooks` block.
- New Rust unit tests in `morph-cli/src/setup.rs::tests` paralleling
  the OpenCode + Cursor coverage:
  `claude_code_requires_morph_init`,
  `claude_code_settings_json_created`,
  `claude_code_hook_scripts_written_and_executable`,
  `claude_code_hooks_registered_for_userpromptsubmit_and_stop`,
  `claude_code_settings_json_merge_preserves_existing`,
  `claude_code_idempotent`.

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

[Unreleased]: https://github.com/r/morph/compare/v0.48.3...HEAD
[0.48.3]: https://github.com/r/morph/compare/v0.48.2...v0.48.3
[0.48.2]: https://github.com/r/morph/compare/v0.48.1...v0.48.2
[0.48.1]: https://github.com/r/morph/compare/v0.48.0...v0.48.1
[0.48.0]: https://github.com/r/morph/compare/v0.47.0...v0.48.0
[0.47.0]: https://github.com/r/morph/compare/v0.46.0...v0.47.0
[0.46.0]: https://github.com/r/morph/compare/v0.45.0...v0.46.0
[0.45.0]: https://github.com/r/morph/compare/v0.44.0...v0.45.0
[0.44.0]: https://github.com/r/morph/compare/v0.43.0...v0.44.0
[0.43.0]: https://github.com/r/morph/compare/v0.42.2...v0.43.0
[0.42.2]: https://github.com/r/morph/compare/v0.42.1...v0.42.2
[0.42.1]: https://github.com/r/morph/compare/v0.42.0...v0.42.1
[0.42.0]: https://github.com/r/morph/compare/v0.41.1...v0.42.0
[0.41.1]: https://github.com/r/morph/compare/v0.41.0...v0.41.1
[0.41.0]: https://github.com/r/morph/compare/v0.40.2...v0.41.0
[0.40.2]: https://github.com/r/morph/compare/v0.40.1...v0.40.2
[0.40.1]: https://github.com/r/morph/compare/v0.40.0...v0.40.1
[0.40.0]: https://github.com/r/morph/compare/v0.39.2...v0.40.0
[0.39.2]: https://github.com/r/morph/compare/v0.39.1...v0.39.2
[0.39.1]: https://github.com/r/morph/compare/v0.39.0...v0.39.1
[0.39.0]: https://github.com/r/morph/compare/v0.38.0...v0.39.0
[0.38.0]: https://github.com/r/morph/compare/v0.37.7...v0.38.0
[0.37.7]: https://github.com/r/morph/compare/v0.37.6...v0.37.7
[0.37.6]: https://github.com/r/morph/compare/v0.37.5...v0.37.6
[0.37.5]: https://github.com/r/morph/compare/v0.37.4...v0.37.5
[0.37.4]: https://github.com/r/morph/compare/v0.37.3...v0.37.4
[0.37.3]: https://github.com/r/morph/compare/v0.37.2...v0.37.3
[0.37.2]: https://github.com/r/morph/compare/v0.37.1...v0.37.2
[0.37.1]: https://github.com/r/morph/compare/v0.37.0...v0.37.1
[0.37.0]: https://github.com/r/morph/compare/v0.36.0...v0.37.0
[0.36.0]: https://github.com/r/morph/releases/tag/v0.36.0
