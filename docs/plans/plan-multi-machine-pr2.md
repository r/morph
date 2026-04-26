# PR 2 — pipemerge: structural Pipeline merge

Per-PR working spec. The full plan lives in `.cursor/plans/multi-machine-ssh-sync_*.plan.md`. PR 1 landed `objmerge` (LCA + suite stage + pipeline/tree stubs); PR 2 replaces the pipeline stub with a real 3-way merge over `Pipeline` objects.

## Scope

Pure module work, no working-tree changes, no CLI changes. Library only.

- New module `morph-core/src/pipemerge.rs` containing the 3-way pipeline merge engine.
- Wire `pipemerge::merge_pipelines` into `objmerge::merge_commits` so the `PipelineDivergent` stub becomes either a clean merge or a structured set of `NodeConflict`s.
- Conflicts produced by `pipemerge` are surfaced through the existing `ObjConflict::Structural { kind: PipelineDivergent, message }` channel — one `ObjConflict` per conflicting node, message includes the node id and the diff axis (added/modified/deleted).

The `MERGE_PIPELINE.json` on-disk schema and the `morph merge resolve-node` CLI are deferred to PR 3 / PR 4 once merge state files exist.

## Public API surface

```rust
// morph-core/src/pipemerge.rs

use crate::objects::{Pipeline, PipelineNode, PipelineEdge};

/// Outcome of a 3-way pipeline merge.
pub struct PipelineMergeOutcome {
    /// Fully merged pipeline when no conflicts; partial merge (best-effort)
    /// when conflicts present so callers can preview.
    pub merged: Pipeline,
    pub conflicts: Vec<NodeConflict>,
}

/// One conflicting node from the structural pipeline merge.
pub struct NodeConflict {
    pub id: String,
    pub axis: ConflictAxis,
    pub base: Option<PipelineNode>,
    pub ours: Option<PipelineNode>,
    pub theirs: Option<PipelineNode>,
}

pub enum ConflictAxis {
    /// Same id added on both sides with different bodies.
    AddAdd,
    /// Modified differently on both sides.
    ModifyModify,
    /// One side modified, other side deleted.
    ModifyDelete,
}

/// 3-way merge two pipelines against a common base. `base = None` means
/// no common ancestor (criss-cross or first merge); we fall back to
/// "both ours and theirs are additions, anything in common must match".
pub fn merge_pipelines(
    base: Option<&Pipeline>,
    ours: &Pipeline,
    theirs: &Pipeline,
) -> PipelineMergeOutcome;
```

Notes:
- Merge happens **per-node by `id`**, not by hash. Two nodes with the same id are "the same node"; two nodes with different ids are independent regardless of body similarity.
- Edge merge is done after node reconciliation: an edge survives only if both endpoints exist in the merged graph. Edges with the same `(from, to, kind)` from any side de-dupe.
- Prompts are merged as a set-preserving-order union: take base's order, then ours-only additions in their order, then theirs-only additions in their order.
- `attribution`/`provenance` are best-effort: take ours when present, else theirs, else base. Not strictly part of the structural contract.

## Test list (red → green sequence)

Each cycle is one TDD increment. Run `cargo test -p morph-core --lib pipemerge` between every cycle.

### Trivial cases

1. **`merge_pipelines_identical_returns_same`** — base==ours==theirs returns the input verbatim, no conflicts.
2. **`merge_pipelines_only_ours_changed_takes_ours`** — theirs == base, ours differs (added node) → result = ours.
3. **`merge_pipelines_only_theirs_changed_takes_theirs`** — symmetric.

### Node add

4. **`merge_pipelines_disjoint_node_adds_unioned`** — base = [A], ours adds B, theirs adds C → merged = [A, B, C].
5. **`merge_pipelines_same_id_same_body_added_both`** — both add identical node X → merged has X, no conflict.
6. **`merge_pipelines_same_id_diff_body_added_both_conflicts`** — both add X with different params → AddAdd conflict, X dropped from merged.

### Node modify

7. **`merge_pipelines_node_modified_one_side`** — base = [A], ours modifies A, theirs unchanged → merged has modified A.
8. **`merge_pipelines_node_modified_both_sides_same_way`** — identical modify → merged has change, no conflict.
9. **`merge_pipelines_node_modified_both_sides_differently_conflicts`** — ModifyModify conflict.

### Node delete

10. **`merge_pipelines_node_deleted_one_side_unchanged_other`** — A removed.
11. **`merge_pipelines_node_deleted_both_sides`** — A removed.
12. **`merge_pipelines_modify_delete_conflicts`** — ours modifies A, theirs deletes A → ModifyDelete conflict (A kept on the modify side in `merged` for preview).

### Edges

13. **`merge_pipelines_edges_unioned_disjoint`** — both sides add different edges between existing nodes → all present.
14. **`merge_pipelines_edges_orphaned_dropped`** — ours deletes node A, theirs adds an edge B→A → edge dropped from merged (no conflict — derived consequence).

### Prompts

15. **`merge_pipelines_prompts_unioned`** — base = ["p1"], ours adds "p2", theirs adds "p3" → merged prompts = ["p1", "p2", "p3"] in stable order.

### Integration with `objmerge::merge_commits`

16. **`merge_commits_resolves_pipeline_when_pipemerge_clean`** — when both branches' pipelines load and structurally merge, the dispatcher emits no `PipelineDivergent` conflict and the outcome's pipeline is implicitly "merged" (no field on `MergeOutcome` yet — we'll add `union_pipeline: Option<Hash>` in this PR if needed for the test).
17. **`merge_commits_surfaces_node_conflicts_as_structural`** — when pipemerge produces NodeConflicts, dispatcher emits one `ObjConflict::Structural { kind: PipelineDivergent, message }` per node conflict.

That's 17 cycles for PR 2.

## Wiring

- `pub mod pipemerge;` in [morph-core/src/lib.rs](../../morph-core/src/lib.rs).
- Re-export: `pub use pipemerge::{merge_pipelines, PipelineMergeOutcome, NodeConflict, ConflictAxis};`.
- Extend `MergeOutcome` with `pub union_pipeline: Option<crate::objects::Pipeline>` so cycle 16 has something concrete to assert. (Mirror of `union_suite`.)
- Update `objmerge::merge_commits` pipeline stage:
  - Load both pipelines (handle the all-zero placeholder case from PR 1's `raw_commit` helpers — return cleanly).
  - Call `merge_pipelines(base_pipeline, head_pipeline, other_pipeline)`.
  - On success: set `outcome.union_pipeline = Some(merged)`.
  - On NodeConflicts: push one `Structural { kind: PipelineDivergent, message: format!("node '{}': {}", id, axis) }` per conflict, leave `union_pipeline = None`.

## Don't do in PR 2

- Don't add merge state files (`MERGE_PIPELINE.json`) — that's PR 3.
- Don't touch the CLI — `morph merge resolve-node` lands in PR 4.
- Don't reorganize `merge.rs` — back-half stays as-is until PR 4.
- Don't change `Store` trait.

## Done criteria

- `cargo test -p morph-core --lib pipemerge` — all 17 tests green.
- `cargo test --workspace` — no regressions.
- `morph eval record` with updated test count + `pipemerge_tests_total/passed`.
- Commit on the same branch (`feat/multi-machine-pr1-objmerge` extended, or new `feat/multi-machine-pr2-pipemerge` per user choice).
- No version bump (still no user-visible surface).
