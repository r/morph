# PR 3 — treemerge + text leaf + merge state + index unmerged + workdir cleanliness + 0.5 migration

Per-PR working spec. PR 1 landed `objmerge` (LCA + suite stage). PR 2 landed `pipemerge` (3-way pipeline merge). PR 3 replaces the `TreeDivergent` stub with a real 3-way tree merge, drops the in-progress-merge state files into `.morph/`, extends the index with unmerged entries, adds a working-tree cleanliness check, and bumps `repo_version` to `0.5`.

This is the largest PR of the plan because it's the first one that touches the on-disk repository layout. Still library-only — the CLI rework lands in PR 4.

## Scope

### In

- New module `morph-core/src/treemerge.rs` — 3-way tree walker.
- New module `morph-core/src/text3way.rs` — leaf text 3-way merge by shelling out to `git merge-file`.
- New module `morph-core/src/merge_state.rs` — read/write/clear for `.morph/MERGE_HEAD`, `.morph/MERGE_MSG`, `.morph/ORIG_HEAD`, `.morph/MERGE_PIPELINE.json`, `.morph/MERGE_SUITE`.
- Extend `morph-core/src/index.rs`:
  - Add `unmerged_entries: BTreeMap<String, UnmergedEntry>` (with `#[serde(default)]`, skip-serialize when empty so old binaries reading a clean index see byte-identical JSON).
  - Helpers: `mark_unmerged`, `resolve_unmerged`, `unmerged_paths`, `has_unmerged`.
- Working-tree cleanliness check: `morph-core/src/workdir.rs::working_tree_clean(morph_dir, working_dir) -> CleanResult`.
- Wire `treemerge` into `objmerge::merge_commits`:
  - `MergeOutcome` gains `union_tree: Option<Hash>` and `working_writes: Vec<(PathBuf, Vec<u8>)>`.
  - Existing `Textual` variant of `ObjConflict` gets populated from textual leaf conflicts.
- Migration: `STORE_VERSION_0_5 = "0.5"`, `migrate_0_4_to_0_5` (config-only), `morph upgrade` learns the new branch.
- Improve `require_store_version` so the error distinguishes "repo too old, run `morph upgrade`" from "repo too new, update your morph binary".

### Out (deferred)

- CLI surface (`morph merge`, `morph merge --abort`, `morph merge --continue`, `morph status` integration) — PR 4.
- The actual write to the working tree from `MergeOutcome.working_writes` — PR 4 owns that orchestration.
- SSH transport — PR 5.
- Server-readiness features — PR 6.

## Public API surface

```rust
// morph-core/src/text3way.rs
pub enum TextMergeResult {
    Clean(Vec<u8>),
    Conflict { content_with_markers: Vec<u8> },
}

/// Shell out to `git merge-file` for 3-way text merge. Falls back gracefully
/// if `git` is missing on PATH (returns Err with a structured MorphError so
/// callers can surface a clear "install git" message).
pub fn merge_text(
    base: Option<&[u8]>,
    ours: &[u8],
    theirs: &[u8],
    labels: TextMergeLabels,
) -> Result<TextMergeResult, MorphError>;

pub struct TextMergeLabels {
    pub base: String,   // e.g. "base"
    pub ours: String,   // e.g. "HEAD"
    pub theirs: String, // e.g. "MERGE_HEAD"
}
```

```rust
// morph-core/src/treemerge.rs
pub struct TreeMergeOutcome {
    /// Hash of the merged Tree object. None when conflicts prevent it.
    pub merged_tree: Option<Hash>,
    /// Textual / structural conflicts produced during the walk.
    pub conflicts: Vec<ObjConflict>,
    /// Files to materialize in the working tree. For clean merges this is
    /// the merged content; for textual conflicts this is the conflict-marked
    /// blob so the user can edit and resolve.
    pub working_writes: Vec<(PathBuf, Vec<u8>)>,
}

/// 3-way merge two tree hashes against an optional common base.
pub fn merge_trees(
    store: &dyn Store,
    base: Option<&Hash>,
    ours: &Hash,
    theirs: &Hash,
) -> Result<TreeMergeOutcome, MorphError>;
```

```rust
// morph-core/src/merge_state.rs
pub fn write_merge_head(morph_dir: &Path, hash: &Hash) -> Result<(), MorphError>;
pub fn read_merge_head(morph_dir: &Path) -> Result<Option<Hash>, MorphError>;
pub fn write_merge_msg(morph_dir: &Path, msg: &str) -> Result<(), MorphError>;
pub fn read_merge_msg(morph_dir: &Path) -> Result<Option<String>, MorphError>;
pub fn write_orig_head(morph_dir: &Path, hash: &Hash) -> Result<(), MorphError>;
pub fn read_orig_head(morph_dir: &Path) -> Result<Option<Hash>, MorphError>;
pub fn write_merge_pipeline(morph_dir: &Path, p: &Pipeline) -> Result<(), MorphError>;
pub fn read_merge_pipeline(morph_dir: &Path) -> Result<Option<Pipeline>, MorphError>;
pub fn write_merge_suite(morph_dir: &Path, hash: &Hash) -> Result<(), MorphError>;
pub fn read_merge_suite(morph_dir: &Path) -> Result<Option<Hash>, MorphError>;
pub fn clear_merge_state(morph_dir: &Path) -> Result<(), MorphError>;
pub fn merge_in_progress(morph_dir: &Path) -> bool;
```

```rust
// morph-core/src/index.rs (extension)
#[derive(...)]
pub struct StagingIndex {
    pub entries: BTreeMap<String, String>,
    /// Unmerged paths from an in-progress merge. Empty in the steady state;
    /// emitted only when `merge_in_progress(.morph) == true`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unmerged_entries: BTreeMap<String, UnmergedEntry>,
}

#[derive(...)]
pub struct UnmergedEntry {
    pub base_blob: Option<String>,
    pub ours_blob: Option<String>,
    pub theirs_blob: Option<String>,
}

pub fn mark_unmerged(morph_dir: &Path, path: &str, entry: UnmergedEntry) -> Result<(), MorphError>;
pub fn resolve_unmerged(morph_dir: &Path, path: &str, blob_hash: &str) -> Result<(), MorphError>;
pub fn unmerged_paths(morph_dir: &Path) -> Result<Vec<String>, MorphError>;
pub fn has_unmerged(morph_dir: &Path) -> Result<bool, MorphError>;
```

```rust
// morph-core/src/workdir.rs
pub struct CleanResult {
    pub clean: bool,
    pub dirty_paths: Vec<String>,
}

/// Compare HEAD's tree with the current working directory contents. Files
/// listed in the index but missing on disk count as dirty; files in HEAD
/// not in the working tree count as dirty; modified content counts as dirty.
pub fn working_tree_clean(morph_dir: &Path, working_dir: &Path) -> Result<CleanResult, MorphError>;
```

```rust
// morph-core/src/objmerge.rs (extension)
pub struct MergeOutcome {
    // ... existing fields ...
    pub union_tree: Option<Hash>,
    pub working_writes: Vec<(PathBuf, Vec<u8>)>,
}
```

```rust
// morph-core/src/repo.rs / migrate.rs
pub const STORE_VERSION_0_5: &str = "0.5";
pub fn migrate_0_4_to_0_5(morph_dir: &Path) -> Result<(), MorphError>;

// Improved require_store_version error
pub enum MorphError {
    // ... existing variants ...
    RepoTooOld { current: String, needed: Vec<String> },   // run morph upgrade
    RepoTooNew { current: String, needed: Vec<String> },   // update binary
}
```

## Test list (red → green sequence)

Run `cargo test -p morph-core --lib <module>` between cycles. CLI integration deferred to PR 4 — no `morph-cli` test changes here.

### Stage A — text 3-way leaf (cycles 1-4)

1. **`merge_text_no_conflict_returns_clean`** — base/ours/theirs all merge cleanly via `git merge-file`; result is `Clean(merged_bytes)`.
2. **`merge_text_with_conflict_returns_markers`** — disjoint changes on the same line range produce git-style `<<<<<<<`/`=======`/`>>>>>>>` markers.
3. **`merge_text_identical_inputs_returns_unchanged`** — base==ours==theirs returns `Clean` with the same bytes.
4. **`merge_text_handles_missing_base_for_add_add`** — `base = None` → uses an empty file as base; AddAdd-equivalent merge.

(If `git` binary missing → return a structured `MorphError::ToolMissing("git")`. Tested by deliberately overriding PATH in one cycle.)

### Stage B — 3-way tree walker (cycles 5-15)

Each cycle uses a `setup_repo()` + helpers that put blobs and Tree objects directly. The walker produces a merged Tree (stored in the store) plus a list of working-tree writes (path → bytes).

5. **`merge_trees_no_changes_returns_same_hash`** — base==ours==theirs → merged_tree == ours, no writes, no conflicts.
6. **`merge_trees_one_side_added`** — ours adds `a.txt`, theirs unchanged → merged tree contains `a.txt`, working_writes has it.
7. **`merge_trees_both_sides_added_same_content`** — both add `a.txt` with same bytes → merged contains it, no conflict.
8. **`merge_trees_both_sides_added_diff_content_text_merges`** — both add `a.txt` with different content; text 3-way merge with `base=None` succeeds → clean blob in tree.
9. **`merge_trees_both_sides_added_diff_content_text_conflicts`** — same as 8 but disjoint changes → `Textual` conflict + working_writes contains conflict-marked blob.
10. **`merge_trees_one_side_modified_other_unchanged`** — ours modifies `a.txt`, theirs unchanged → merged has the modified blob.
11. **`merge_trees_both_sides_modified_same_way`** — both modify identically → merged has the change.
12. **`merge_trees_both_sides_modified_clean_text_merge`** — non-overlapping line edits → text merger merges cleanly.
13. **`merge_trees_both_sides_modified_text_conflict`** — overlapping edits → `Textual` conflict + working_writes carries conflict markers.
14. **`merge_trees_one_side_deleted_other_unchanged`** — ours deletes `a.txt`, theirs unchanged → not in merged tree, working_writes records a delete (`bytes = vec![]` won't do — see note below).
15. **`merge_trees_modify_delete_conflicts`** — ours modifies, theirs deletes → `Structural { kind: TreeDivergent, message: "modify/delete: a.txt" }` (we reuse `TreeDivergent` for path-level structural conflicts) + working_writes preserves the modified blob.

> **Note on deletes**: `working_writes: Vec<(PathBuf, Vec<u8>)>` can't encode "delete this file". For PR 3 we extend this to `Vec<WorkdirOp>` where `WorkdirOp::Write { path, bytes }` and `WorkdirOp::Delete { path }`. Cycle 14 specifies this directly so the API is right from the start.

### Stage C — merge state files (cycles 16-19)

16. **`merge_state_head_msg_orig_roundtrip`** — write/read/clear for MERGE_HEAD, MERGE_MSG, ORIG_HEAD; missing-file returns `None`.
17. **`merge_state_pipeline_json_roundtrip`** — `MERGE_PIPELINE.json` round-trips a Pipeline including conflicts metadata.
18. **`merge_state_suite_hash_roundtrip`** — `MERGE_SUITE` stores a single hash.
19. **`clear_merge_state_removes_all_files`** + **`merge_in_progress_returns_true_iff_merge_head_exists`** (one cycle, two assertions).

### Stage D — index unmerged entries (cycles 20-23)

20. **`index_reads_old_format_without_unmerged_field`** — read a `.morph/index.json` written before PR 3 (no `unmerged_entries` key) → loads with empty map, no error.
21. **`index_writes_omit_unmerged_when_empty`** — fresh `StagingIndex` round-trips to JSON without the `unmerged_entries` key (byte-equal to old format).
22. **`mark_unmerged_persists_entry_with_three_blobs`**.
23. **`resolve_unmerged_clears_entry_and_writes_normal_entry`**.

### Stage E — workdir cleanliness (cycles 24-27)

24. **`working_tree_clean_when_no_changes`** — fresh checkout matches HEAD.
25. **`working_tree_dirty_when_file_modified`** — return list contains the modified path.
26. **`working_tree_dirty_when_tracked_file_deleted`**.
27. **`working_tree_dirty_when_untracked_change_to_tracked_path`** — same path different bytes → dirty.

(Untracked files outside HEAD don't count as dirty for merge gating; mirror Git's `merge` behavior, not `commit`.)

### Stage F — `objmerge::merge_commits` integration (cycles 28-31)

28. **`merge_commits_resolves_tree_when_clean`** — both branches edit different files cleanly → `union_tree` set, no `TreeDivergent`/`Textual` conflicts, working_writes contains both.
29. **`merge_commits_emits_textual_for_overlapping_edits`** — same file, conflicting edits → one `ObjConflict::Textual` per path; working_writes carries conflict-marked blob.
30. **`merge_commits_modify_delete_emits_tree_divergent`** — uses `Structural::TreeDivergent` (no longer a stub).
31. **`merge_commits_falls_back_to_stub_when_tree_unloadable`** — placeholder hashes still emit a generic `TreeDivergent` so PR 1's stub-coverage test keeps passing (mirror of PR 2's pipeline fallback).

### Stage G — repo_version 0.5 + improved errors (cycles 32-35)

32. **`migrate_0_4_to_0_5_writes_correct_version`** — config-only migration, no object rewriting.
33. **`open_store_handles_0_5`** — same fan-out backend as 0.4.
34. **`require_store_version_returns_repo_too_new_for_unknown_higher_version`** — repo says `"0.99"`, allowed list is `["0.5"]` → `RepoTooNew`, error message says "update your morph binary".
35. **`require_store_version_returns_repo_too_old_for_known_lower_version`** — repo says `"0.3"`, allowed `["0.5"]` → `RepoTooOld`, message says "run `morph upgrade`".

(Per the user's confirmation, the `morph upgrade` CLI branch and its integration spec stay in PR 4. PR 3 ships the migration function and constants; PR 4 wires them into the user-facing command.)

## Wiring summary

- `morph-core/src/lib.rs` — add `pub mod text3way; pub mod treemerge; pub mod merge_state; pub mod workdir;`. Re-export the public API.
- `morph-core/src/objmerge.rs` — pipeline-stage style integration:
  - Replace `TreeDivergent` stub with `treemerge::merge_trees(...)`.
  - Wire `union_tree`, `working_writes` into `MergeOutcome`.
  - Keep the unloadable-hash fallback (mirror of PR 2) so PR 1's stub tests stay green.
- `morph-core/src/repo.rs` — `STORE_VERSION_0_5` constant; updated `open_store` (no behavior change since same FsStore backend); refined `require_store_version` returning the new error variants.
- `morph-core/src/migrate.rs` — `migrate_0_4_to_0_5` (config-only).
- No CLI changes in this PR. `morph upgrade`'s 0.4→0.5 branch lands in PR 4 along with the rest of the CLI rework. PR 3 ships the `migrate_0_4_to_0_5` function so PR 4 can wire it in trivially.

Total: **35 cycles**. Long PR, but each stage is small and independently testable.

## Migration notes (for `docs/MULTI-MACHINE.md` later)

This PR establishes the migration pattern that all subsequent PRs follow:

1. Bump `STORE_VERSION_0_X` constant.
2. Add a `migrate_0_(X-1)_to_0_X` (config-only when possible).
3. Extend `morph upgrade` with the new branch.
4. Update `require_store_version` callers' allowed list.
5. New repo state (files / config keys / index extensions) must be either out-of-band or `#[serde(default)]` so an old binary reading a fresh-state new-version repo doesn't crash — it errors at the version gate, not at JSON parse time.
6. The new `RepoTooNew` error directs users to update their binary; `RepoTooOld` directs them to `morph upgrade`.

## Don't do in PR 3

- Don't write to the working tree from inside `merge_commits`. The `working_writes` field is just a *plan*; PR 4's CLI applies it after dominance gating.
- Don't auto-create the `.morph/MERGE_*` files anywhere yet — `merge_state.rs` only exposes the read/write primitives. The orchestrator calling them lives in PR 4.
- Don't change the `Store` trait.
- Don't add `morph status` extensions for unmerged paths — PR 4.
- Don't bump the workspace `Cargo.toml` version yet — that's PR 7's wrap-up.

## Done criteria

- `cargo test --workspace` — no regressions; expect ~36 new tests added (likely 35 unit + 1 spec).
- `morph eval record` with updated counts and per-module breakdown.
- Commit on the same branch (`feat/multi-machine-pr1-objmerge`) keeping the linear history.
- repo_version constant bumped to 0.5; migration in place; `morph upgrade` covers the new step.
- No version bump for the `morph` binary (workspace `Cargo.toml`) — still no user-visible CLI surface from this PR alone.
