# PR 4 — CLI: `morph merge` becomes the structural flow

Per-PR working spec. PR 1 landed `objmerge` (LCA + suite stage). PR 2 landed `pipemerge`. PR 3 landed `treemerge` + merge state files + index unmerged + workdir cleanliness + `repo_version` 0.5. PR 4 makes all of that user-facing: `morph merge` drives the structural engine, plans working-tree writes, marks unmerged paths in the index, leaves `.morph/MERGE_*` breadcrumbs, and offers `--continue` / `--abort` to finish or unwind. `morph status` learns the merge-in-progress block. `morph pull` learns to surface divergence as a typed error and to optionally hand off to `morph merge`. `morph upgrade` learns the 0.4 → 0.5 branch.

This PR turns the engine into a workflow. It is the first one users notice.

## Scope

### In

- New CLI orchestrator `morph-core/src/merge_flow.rs` — pure, library-side state machine that the CLI calls into:
  - `start_merge(store, repo_root, other_branch, opts) -> StartMergeOutcome`
  - `continue_merge(store, repo_root, opts) -> ContinueMergeOutcome`
  - `abort_merge(store, repo_root) -> AbortMergeOutcome`
  - `resolve_node(store, repo_root, node_id, side) -> ()` (for pipeline node conflicts)
- CLI rework in `morph-cli/src/cli.rs` + `morph-cli/src/main.rs`:
  - Extend `Command::Merge` with mutually-exclusive `--abort` / `--continue` flags.
  - Drop the previous *single-shot* mandatory `--pipeline` / `--metrics` / `--message` flags from the `start` path; they move onto `--continue`.
  - Add `Command::Merge { ... resolve_node: Option<String>, prefer: Option<String> }` for pipeline-node resolution.
- `morph status` extension: when `merge_in_progress(.morph) == true`, prepend a `Merge in progress` block listing unmerged paths, pipeline-node conflicts, and the suite/dominance gate status. Existing "changes not staged" / activity summary still print below.
- `morph pull --merge` flag: on divergence, instead of erroring, kick off the merge flow against the just-fetched remote tip.
- `pull_branch` returns a typed error variant `MorphError::Diverged { local_tip, remote_tip, branch }` so the CLI can offer the `--merge` hint cleanly.
- `morph upgrade` learns 0.4 → 0.5 (the `migrate_0_4_to_0_5` from PR 3).
- Bump CLI's allowed version list to include `STORE_VERSION_0_5`. Same for `morph-mcp`.
- Two new merge spec files in `morph-cli/tests/specs/`: `merge_structural.yaml` (happy path + conflict resolution + abort) and `pull_merge.yaml`.

### Out (deferred)

- SSH transport — PR 5.
- Server-readiness (`user.name`, `user.email`, `agent.instance_id`, `bare`, schema handshake, server closure validation, push-time gate) — PR 6.
- Writing automatic resolution heuristics ("prefer ours", auto-resolve identical changes by content) — PR 6 *only if* user asks; otherwise out-of-scope of v0.

## What `morph merge` looks like to a user

```
$ morph merge feature/xyz
Computing merge base ... a3b2c1d
Merging suite contracts ... ok
Merging pipeline ... 1 node conflict:
  - node "summarizer": modify/modify
    base    -> openai:gpt-4
    ours    -> openai:gpt-4-turbo
    theirs  -> anthropic:claude-3
    Run: morph merge resolve-node summarizer --prefer ours|theirs
Merging tree ... 2 textual conflicts, 1 modify/delete:
  CONFLICT (content): src/pipeline.py
  CONFLICT (content): docs/README.md
  CONFLICT (modify/delete): scripts/old.sh (theirs deleted, ours modified)
Working tree updated. Resolve conflicts and run:
  morph merge --continue --pipeline <hash> --metrics '{...}'
Or abort with:
  morph merge --abort
```

```
$ morph status
Merge in progress (merging 'feature/xyz' into 'main', base a3b2c1d):
  unmerged paths:
    src/pipeline.py        (textual)
    docs/README.md         (textual)
    scripts/old.sh         (modify/delete)
  unresolved pipeline nodes:
    summarizer             (modify/modify)
  suite gate:              ok (union resolved)
  dominance gate:          deferred to --continue

Changes not staged for commit:
  ...
```

```
$ morph merge --continue --pipeline <hash> --metrics '{...}' -m "Merge feature/xyz"
Re-checking unmerged paths ... all resolved.
Re-checking unresolved pipeline nodes ... all resolved.
Building merged tree ... ok
Building merged pipeline ... ok
Building union eval suite ... ok
Checking dominance ... ok
Created merge commit 7c4f...
Cleared merge state.
```

```
$ morph merge --abort
Restored HEAD to b1c2d3 (ORIG_HEAD).
Restored working tree.
Cleared merge state.
```

## Public API surface (library, called by CLI)

```rust
// morph-core/src/merge_flow.rs

pub struct StartMergeOpts<'a> {
    pub other_branch: &'a str,
    /// If true, refuse to start when working tree is dirty. Default true; mirror
    /// of `git merge` behavior.
    pub require_clean_workdir: bool,
}

pub struct StartMergeOutcome {
    pub head: Hash,
    pub other: Hash,
    pub base: Option<Hash>,
    /// Whether the merge engine reached a state requiring user resolution.
    /// `false` means trivial fast-forward / already-up-to-date / clean
    /// structural merge — the CLI proceeds directly to commit creation.
    pub needs_resolution: bool,
    pub textual_conflicts: Vec<String>,         // path strings
    pub structural_tree_conflicts: Vec<String>, // path strings (modify/delete etc.)
    pub pipeline_node_conflicts: Vec<NodeConflict>,
    pub suite_resolved: bool,
}

pub struct ContinueMergeOpts<'a> {
    pub message: &'a str,
    pub author: Option<&'a str>,
    /// Pipeline that the user attests is the merged pipeline (after any
    /// `resolve-node` resolutions). Required.
    pub pipeline_hash: &'a Hash,
    /// Observed metrics for the merged program. Required: this is the input
    /// to dominance.
    pub observed_metrics: BTreeMap<String, f64>,
    /// Optional eval-suite hash override; default is the suite written by
    /// `start_merge` into `.morph/MERGE_SUITE`.
    pub eval_suite: Option<&'a Hash>,
}

pub struct ContinueMergeOutcome {
    pub merge_commit: Hash,
}

pub struct AbortMergeOutcome {
    pub restored_head: Hash,
}

pub fn start_merge(
    store: &dyn Store,
    repo_root: &Path,
    opts: StartMergeOpts,
) -> Result<StartMergeOutcome, MorphError>;

pub fn continue_merge(
    store: &dyn Store,
    repo_root: &Path,
    opts: ContinueMergeOpts,
) -> Result<ContinueMergeOutcome, MorphError>;

pub fn abort_merge(
    store: &dyn Store,
    repo_root: &Path,
) -> Result<AbortMergeOutcome, MorphError>;

pub fn resolve_node(
    store: &dyn Store,
    repo_root: &Path,
    node_id: &str,
    prefer: ResolveSide,
) -> Result<(), MorphError>;

pub enum ResolveSide { Ours, Theirs }
```

```rust
// morph-core/src/store.rs (extension)
pub enum MorphError {
    // ... existing variants ...
    #[error("Branch '{branch}' has diverged from {remote_tip} (local at {local_tip}); fast-forward not possible")]
    Diverged { branch: String, local_tip: String, remote_tip: String },
}
```

```rust
// morph-core/src/sync.rs (revised pull_branch)
pub fn pull_branch(...) -> Result<Hash, MorphError> {
    // ... unchanged for already-up-to-date / can-fast-forward ...
    // On divergence:
    Err(MorphError::Diverged { branch, local_tip, remote_tip })
}
```

```rust
// morph-cli/src/cli.rs (revised Command::Merge)
Merge {
    /// Branch to merge into HEAD (positional). Required for start; ignored
    /// with --continue / --abort / resolve-node.
    branch: Option<String>,

    /// Resume an in-progress merge. Mutually exclusive with --abort and the
    /// `branch` positional.
    #[arg(long, conflicts_with_all = ["abort", "branch"])]
    cont: bool,

    /// Abort an in-progress merge.
    #[arg(long, conflicts_with_all = ["cont", "branch"])]
    abort: bool,

    /// Resolve a pipeline-node conflict by picking a side.
    /// e.g. `morph merge resolve-node summarizer --prefer theirs`
    #[arg(long)]
    resolve_node: Option<String>,
    #[arg(long, requires = "resolve_node")]
    prefer: Option<String>, // "ours" | "theirs"

    /// Required on --continue.
    #[arg(short, long, required_if_eq("cont", "true"))]
    message: Option<String>,
    #[arg(long, required_if_eq("cont", "true"))]
    pipeline: Option<String>,
    #[arg(long)]
    eval_suite: Option<String>,
    #[arg(long, required_if_eq("cont", "true"))]
    metrics: Option<String>,
    #[arg(long)]
    author: Option<String>,
    #[arg(long)]
    retire: Option<String>,
}
```

```rust
// morph-cli/src/cli.rs (revised Command::Pull)
Pull {
    remote: String,
    branch: String,
    /// On divergence, hand off to `morph merge` instead of erroring.
    #[arg(long)]
    merge: bool,
}
```

## Test list (red → green sequence)

CLI integration tests live in `morph-cli/tests/specs/*.yaml`; library logic tests live in `#[cfg(test)] mod tests` blocks at the bottom of each `.rs` file. Run between cycles:
- `cargo test -p morph-core --lib merge_flow::`
- `cargo test -p morph-cli --test cli_specs -- merge_structural`
- `cargo test --workspace` after each stage.

### Stage A — typed divergence error (cycles 1-3)

1. **`pull_branch_returns_diverged_for_diverged_branches`** *(`sync.rs`)* — local and remote each have an extra commit past the LCA; `pull_branch` returns `MorphError::Diverged { branch, local_tip, remote_tip }` with the right fields populated. Replaces the existing string-based assertion.
2. **`pull_branch_still_fast_forwards_when_local_is_ancestor`** *(`sync.rs`)* — regression: clean fast-forward path still works. Asserts that the existing test `pull_fast_forwards_when_remote_ahead` still passes after the error change.
3. **`pull_branch_already_up_to_date_returns_local_tip`** *(`sync.rs`)* — regression: idempotent pull.

### Stage B — `start_merge` library state machine (cycles 4-13)

Each cycle uses a `setup_two_branches`-style helper plus a temp `repo_root`. The test inspects the on-disk `.morph/MERGE_*` files directly to verify the state machine wrote what it claimed.

4. **`start_merge_already_up_to_date_no_op`** — other is an ancestor of HEAD → `needs_resolution=false`, no `MERGE_HEAD` written.
5. **`start_merge_fast_forwardable_returns_no_resolution_needed`** — HEAD is an ancestor of other → CLI is expected to fast-forward; library returns `needs_resolution=false`, fills in `head`/`other`/`base` for the CLI to print, no `MERGE_HEAD` written.
6. **`start_merge_clean_three_way_no_user_input`** — divergent branches, no conflicts (pipeline/tree merge cleanly, suites compatible) → `needs_resolution=false`, no `MERGE_HEAD` written; CLI proceeds to `continue_merge` directly with the auto-built artifacts.
7. **`start_merge_writes_merge_head_when_resolution_needed`** — at least one textual or structural conflict → `MERGE_HEAD == other_hash`, `ORIG_HEAD == head_hash`, `MERGE_MSG` populated with default `Merge branch 'X'`.
8. **`start_merge_writes_merge_pipeline_when_pipeline_needs_resolution`** — pipeline node conflict → `.morph/MERGE_PIPELINE.json` contains the partial pipeline + node-conflict metadata; `node_conflicts` is non-empty in the outcome.
9. **`start_merge_writes_merge_suite_when_suite_resolved`** — suite is reconcilable → `MERGE_SUITE` contains the union-suite hash.
10. **`start_merge_marks_unmerged_index_entries_for_textual_conflicts`** — for each path in `working_writes` that's a `Textual` conflict, `mark_unmerged` is called → `index.unmerged_entries` contains base/ours/theirs blob hashes for that path.
11. **`start_merge_writes_working_tree_for_clean_paths_and_conflict_markers`** — working tree on disk reflects `working_writes` from PR 3: clean files written as-is, conflict files written with markers.
12. **`start_merge_refuses_when_working_tree_dirty_and_require_clean_set`** — pre-existing modified file → returns an error, doesn't touch any `.morph/MERGE_*` file. Mirror of `git merge`'s behavior.
13. **`start_merge_aborts_cleanly_on_suite_incompatible`** — suite gate fails → returns a `Structural { kind: SuiteIncompatible }` conflict, doesn't write any `MERGE_*` file (suite incompatibility is fatal up-front).

### Stage C — `continue_merge` (cycles 14-20)

14. **`continue_merge_fails_when_no_merge_in_progress`** — fresh repo + `continue_merge` → error.
15. **`continue_merge_fails_when_unmerged_paths_remain`** — `MERGE_HEAD` set, `index.unmerged_entries` non-empty → error listing the unresolved paths.
16. **`continue_merge_fails_when_pipeline_nodes_unresolved`** — `MERGE_PIPELINE.json` has node-conflicts → error listing unresolved nodes.
17. **`continue_merge_fails_when_dominance_violated`** — all paths/nodes resolved, but supplied `metrics` don't dominate both parents → error from `MergePlan::check_dominance`, merge state preserved (so user can retry with corrected metrics).
18. **`continue_merge_succeeds_for_clean_resolutions`** — happy path: produces a merge commit with two parents, pipeline = supplied `pipeline_hash`, suite = `MERGE_SUITE` content, dominant metrics → `MERGE_*` files cleared, returns the new commit hash.
19. **`continue_merge_uses_merge_pipeline_json_when_pipeline_arg_omitted`** — *if* pipeline-node-conflicts were all resolved via `resolve_node`, `MERGE_PIPELINE.json` itself stores the final pipeline; supplying `--pipeline` is then optional (CLI accepts it but library prefers stored). Documented behavior choice; test pins it.
20. **`continue_merge_clears_unmerged_entries_on_success`** — `index.unmerged_entries` is empty after a successful continue.

### Stage D — `abort_merge` and `resolve_node` (cycles 21-25)

21. **`abort_merge_fails_when_no_merge_in_progress`**.
22. **`abort_merge_restores_head_to_orig_head_and_clears_state`** — after `start_merge`, abort → working tree restored to `ORIG_HEAD`'s tree, all `MERGE_*` files gone, `index.unmerged_entries` empty.
23. **`abort_merge_does_not_lose_uncommitted_work`** — if user has untracked files outside the merge, abort preserves them. (Mirror of git semantics: abort is "undo the merge attempt", not "reset --hard everything".)
24. **`resolve_node_picks_ours_writes_pipeline_json`** — `resolve_node("summarizer", Ours)` rewrites `MERGE_PIPELINE.json` so the conflict for `summarizer` is gone and the node value matches `ours`.
25. **`resolve_node_errors_for_unknown_node`** — node id not in current conflict set → error.

### Stage E — `morph status` integration (cycles 26-28)

26. **`status_prepends_merge_in_progress_block_when_merge_active`** *(spec test)* — set up a repo mid-merge, `morph status` first lines are the `Merge in progress` block.
27. **`status_lists_unmerged_paths_with_kind`** *(spec test)* — block contains both textual and structural paths with their kind.
28. **`status_lists_pipeline_node_conflicts_when_present`** *(spec test)* — block contains the unresolved node ids.

### Stage F — `morph pull --merge` (cycles 29-31)

29. **`pull_without_merge_flag_prints_diverged_message_with_hint`** *(spec test)* — divergence → exit code non-zero, stderr mentions `Diverged` and suggests `morph pull --merge`.
30. **`pull_with_merge_flag_kicks_off_structural_merge`** *(spec test)* — divergence + `--merge` → enters merge flow, leaves `MERGE_HEAD` if conflicts arise, exits 0 with conflict summary; or completes cleanly when no conflicts.
31. **`pull_with_merge_flag_clean_merge_creates_commit_and_clears_state`** *(spec test)* — divergence + `--merge` + clean → merge commit created, no `MERGE_*` left behind.

### Stage G — `morph upgrade` 0.4 → 0.5 (cycles 32-34)

32. **`upgrade_from_0_4_bumps_to_0_5`** *(spec test)* — repo at 0.4 + `morph upgrade` → version 0.5, stdout reports the bump.
33. **`upgrade_already_at_0_5_no_op`** *(spec test)* — repo at 0.5 + `morph upgrade` → "No upgrade needed".
34. **`upgrade_from_0_0_walks_through_all_steps_to_0_5`** *(spec test)* — legacy repo → upgrades through 0.0 → 0.2 → 0.3 → 0.4 → 0.5 in one call.

### Stage H — version gate (cycles 35-36)

35. **`cli_accepts_0_5_repos`** — the CLI's `require_store_version` allowed list includes 0.5 → opening a 0.5 repo with the new binary works.
36. **`mcp_accepts_0_5_repos`** — same for `morph-mcp`.

## Wiring summary

- `morph-core/src/merge_flow.rs` — new module. Calls `objmerge::merge_commits` (PR 1+2+3), then writes `MERGE_*` files via `merge_state` (PR 3), marks unmerged via `index` (PR 3), applies `working_writes` to disk via `workdir` helpers (PR 3 plus a small new `apply_workdir_ops` helper). `continue_merge` rebuilds the tree from the *resolved* working tree + index, calls `prepare_merge` + `execute_merge` (existing), with the merged-pipeline hash from CLI input and the union suite from `MERGE_SUITE`.
- `morph-core/src/store.rs` — add `MorphError::Diverged`.
- `morph-core/src/sync.rs` — `pull_branch` returns the typed error; existing string-based callers updated.
- `morph-core/src/lib.rs` — re-export `merge_flow::{start_merge, continue_merge, abort_merge, resolve_node, ResolveSide, StartMergeOpts, StartMergeOutcome, ContinueMergeOpts, ContinueMergeOutcome, AbortMergeOutcome}`.
- `morph-core/src/workdir.rs` — add `apply_workdir_ops(repo_root, ops: &[WorkdirOp])` helper if not already wired (PR 3 introduced `WorkdirOp` for planning; PR 4 owns the apply step).
- `morph-cli/src/cli.rs` — revised `Command::Merge` and `Command::Pull` (see snippets above). Bump `require_store_version` allowed list to include `STORE_VERSION_0_5`.
- `morph-cli/src/main.rs`:
  - `Command::Merge` dispatches to `start_merge`, `continue_merge`, `abort_merge`, or `resolve_node` based on the flag combination.
  - `Command::Pull { merge: true }` catches `MorphError::Diverged` and calls `start_merge` directly with the just-fetched remote tip.
  - `Command::Status` checks `merge_in_progress` and prepends the new block.
  - `Command::Upgrade` learns the 0.4 → 0.5 branch (calls `migrate_0_4_to_0_5`).
- `morph-mcp/src/main.rs` — add `STORE_VERSION_0_5` to allowed list.
- `morph-cli/tests/specs/merge_structural.yaml` — happy-path no-op, clean structural merge, conflict + `--continue`, conflict + `--abort`, dominance failure on `--continue`.
- `morph-cli/tests/specs/pull_merge.yaml` — `pull` divergence message, `pull --merge` clean, `pull --merge` with conflicts.
- `morph-cli/tests/specs/upgrade.yaml` — extend with the 0.4 → 0.5 case.

Total: **36 cycles** across 8 stages. Library state-machine first, CLI surface second, regression coverage last.

## Backward-compat notes

- The old `morph merge <branch> -m ... --pipeline ... --metrics ...` shape (single-shot, no resolution) still works *when the merge resolves cleanly without user input*. Practically: if `start_merge` returns `needs_resolution=false`, the CLI internally chains straight into `continue_merge` with the supplied flags, mirroring the previous behavior. This keeps old scripts running on uncomplicated merges.
- When conflicts exist and the user invokes the old shape, the CLI prints the conflict summary, writes `MERGE_*`, and exits non-zero with the new instructions. No silent behavior change for clean cases.
- `morph status` only changes when `merge_in_progress` is true; otherwise output is byte-identical.
- `pull_branch`'s error type changes from `Serialization(String)` to `Diverged { ... }`. Spec callers that match on string content need a one-line update; programmatic callers (none today outside the CLI) get a more useful error. Internally, `Display for MorphError::Diverged` produces a similar string for grep-style assertions.

## Migration story for users mid-development

- Users on 0.4 run `morph upgrade` once → 0.5. Their existing branches/refs/objects unchanged. `MERGE_*` files only appear when they invoke `morph merge` against a new binary.
- If a user is mid-merge (impossible on 0.4 since merge state didn't exist there) — N/A.
- If a 0.4 binary opens a 0.5 repo (e.g., another machine pushes 0.5 metadata to a shared dir), the 0.4 binary fails fast with `RepoTooNew` from PR 3.

## Don't do in PR 4

- Don't introduce auto-resolution heuristics ("prefer ours globally", "prefer non-empty", "conflict-marker majority"). Resolution is explicit (textual: edit and `morph add`; pipeline-node: `resolve-node`).
- Don't surface SSH transport. The `--merge` flag works over the same `open_remote_store` filesystem path; SSH lands in PR 5.
- Don't add server-readiness checks (bare-repo, schema handshake, push-time gate). PR 6.
- Don't add a `morph rebase`. Out of scope of v0.
- Don't change the underlying `merge_commits` algorithm. PR 4 is plumbing; PR 1–3 own the engine.

## Done criteria

- All 36 new tests green; `cargo test --workspace` overall green.
- `morph eval record` with updated counts.
- `docs/MERGE.md` (concise) and a paragraph in `docs/MULTI-MACHINE.md` describing the new flow — actual long-form docs land in PR 7, but PR 4 ships a stub so the help text and CLI message hints can link to them.
- Workspace `Cargo.toml` version bump (minor): user-visible CLI changes ship.
- Commit on the same `feat/multi-machine-pr1-objmerge` branch.
