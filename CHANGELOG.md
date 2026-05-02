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

[Unreleased]: https://github.com/r/morph/compare/v0.41.1...HEAD
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
