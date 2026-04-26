# PR 1 — objmerge skeleton + LCA

Per-PR working spec for the first piece of the multi-machine plan. Status: pre-implementation drafting in plan mode. The full plan lives in `.cursor/plans/multi-machine-ssh-sync_*.plan.md`; this file is the executable test list for PR 1.

## Scope

Pure module work, no working-tree changes, no CLI changes. Everything in this PR lives behind `#[cfg(test)]` and is only consumed by later PRs.

- New module `morph-core/src/objmerge.rs` with conflict types and the top-level `merge_commits` dispatcher (suite stage only in this PR — pipeline and tree stages are stubbed).
- New `merge_base` function (placed inside `objmerge` since it's only used during merge planning).
- Wire `metrics::union_suites` ([morph-core/src/metrics.rs](../../morph-core/src/metrics.rs)) into the suite stage as the first real reconciliation step.

## Public API surface

```rust
// morph-core/src/objmerge.rs

use crate::Hash;
use crate::store::{Store, MorphError};
use crate::objects::{Commit, EvalSuite};
use std::collections::BTreeMap;

/// A single conflict surfaced during structural merge.
#[derive(Clone, Debug)]
pub enum ObjConflict {
    /// Suite or pipeline level — must be resolved before tree merge.
    Structural { kind: StructuralKind, message: String },
    /// File-level text or binary conflict in the working tree.
    Textual { path: std::path::PathBuf, base: Option<Hash>, ours: Option<Hash>, theirs: Option<Hash> },
    /// Merged metrics fail dominance.
    Behavioral { violations: Vec<crate::merge::DominanceViolation> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructuralKind {
    SuiteIncompatible,
    PipelineDivergent,
}

impl std::fmt::Display for ObjConflict { /* ... */ }

/// Result of structural merge planning. Only populated when there are no
/// blocking conflicts; otherwise `conflicts` is non-empty.
#[derive(Clone, Debug)]
pub struct MergeOutcome {
    pub head: Hash,
    pub other: Hash,
    pub base: Option<Hash>,
    /// Effective union eval suite (post-retirement).
    pub union_suite: Option<EvalSuite>,
    /// Conflicts that must be resolved before `morph merge --continue`.
    pub conflicts: Vec<ObjConflict>,
    /// True when commits are equal or one is ancestor of the other.
    pub trivial: TrivialOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrivialOutcome {
    /// `head == other` — nothing to merge.
    AlreadyMerged,
    /// `other` is ancestor of `head` — nothing to merge.
    AlreadyAhead,
    /// `head` is ancestor of `other` — caller can fast-forward.
    FastForward,
    /// Genuinely diverged.
    Diverged,
}

/// Lowest common ancestor of two commits via BFS over `commit.parents`.
/// Returns `Ok(None)` if histories are disjoint.
pub fn merge_base(store: &dyn Store, a: &Hash, b: &Hash) -> Result<Option<Hash>, MorphError>;

/// Top-level structural merge dispatcher (PR 1 implements suite stage; tree
/// and pipeline stages are returned as `Structural` conflicts pointing to
/// "not yet implemented" until later PRs).
pub fn merge_commits(
    store: &dyn Store,
    head: &Hash,
    other: &Hash,
    retire: Option<&[String]>,
) -> Result<MergeOutcome, MorphError>;
```

Notes:
- `merge_base` is `Option<Hash>` not `Hash` because disjoint histories must be expressible. Callers handle "no common base" explicitly.
- `Behavioral` conflicts are constructed by `--continue` later; PR 1 only needs the variant to exist.
- `merge_commits` is the integration point. PR 2 fills in the pipeline stage; PR 3 fills in tree.

## Test list (red → green sequence)

Each line is one TDD cycle. Run `cargo test -p morph-core --lib objmerge` between every cycle. Use the existing `setup_repo()` helper from [morph-core/src/sync.rs](../../morph-core/src/sync.rs)'s test module pattern.

### `merge_base` (LCA)

1. **`merge_base_self_returns_self`** — `merge_base(store, &c, &c) == Some(c)`. Stub: return `Some(*a)` when `a == b`.
2. **`merge_base_ancestor_returns_ancestor`** — make commits `c1 -> c2`; `merge_base(c2, c1) == Some(c1)`. Implement: BFS from `b` checking ancestor.
3. **`merge_base_symmetric`** — same as above but `merge_base(c1, c2)`. Generalize: BFS from both, mark-and-meet.
4. **`merge_base_two_siblings`** — `c0 -> c1`, `c0 -> c2` (separate branches off c0). `merge_base(c1, c2) == Some(c0)`.
5. **`merge_base_disjoint_returns_none`** — two unrelated initial commits. `merge_base(a, b) == Ok(None)`.
6. **`merge_base_criss_cross_deterministic`** — diamond history `c0 -> c1, c0 -> c2, merge1(c1,c2), merge2(c1,c2)`; `merge_base(merge1, merge2)` returns one valid LCA deterministically (we pick the first one met by BFS — document this).
7. **`merge_base_walks_both_parents_of_merge`** — ensures BFS enqueues all parents of merge commits, not just the first.

### `ObjConflict` and `MergeOutcome` types

8. **`obj_conflict_structural_display_includes_kind_and_message`** — verifies `Display` formatting for the structural variant.
9. **`obj_conflict_textual_display_shows_path`**.
10. **`merge_outcome_already_merged_when_equal_commits`** — `merge_commits(c, c)` returns `trivial = AlreadyMerged`, no conflicts.
11. **`merge_outcome_fast_forward_when_head_ancestor_of_other`** — `merge_commits(c1, c2)` where c1 is ancestor of c2 returns `trivial = FastForward`. Caller decides what to do.
12. **`merge_outcome_already_ahead_when_other_ancestor_of_head`** — symmetric case.
13. **`merge_outcome_diverged_for_two_branches`** — populated `base`, `trivial = Diverged`.

### Suite stage

14. **`merge_commits_unions_compatible_suites`** — both branches share `acc` metric with same threshold; `merge_commits` returns no conflicts and `union_suite` containing `acc`.
15. **`merge_commits_returns_structural_conflict_for_incompatible_thresholds`** — both branches' suites have `acc` with different thresholds; conflict variant `Structural { kind: SuiteIncompatible, ... }`. `union_suite` is `None`.
16. **`merge_commits_respects_retired_metrics`** — when `retire = Some(&["old".into()])`, `union_suite` excludes `old` and any threshold mismatch on `old` does not produce a conflict.
17. **`merge_commits_uses_explicit_suite_when_compatible`** — pass-through case where both commits already point at the same suite hash; no work done.

### Pipeline / tree stage stubs

18. **`merge_commits_emits_pipeline_stub_when_pipelines_differ`** — when `head.pipeline != other.pipeline`, emits a `Structural { kind: PipelineDivergent, message: "pipeline merge not yet implemented (PR 2)" }`. (PR 2 replaces this with real per-node merge.)
19. **`merge_commits_emits_tree_stub_when_trees_differ`** — same for tree. (PR 3 replaces this.)

That's 19 cycles for PR 1. Estimate: 1.5–3 hours total at TDD pace.

## Wiring

- `pub mod objmerge;` in [morph-core/src/lib.rs](../../morph-core/src/lib.rs).
- Re-export: `pub use objmerge::{merge_base, merge_commits, MergeOutcome, ObjConflict, StructuralKind, TrivialOutcome};` next to existing merge re-exports.
- No CLI wiring in PR 1 — the dispatcher is library-only until PR 4.

## Don't do in PR 1

- Don't touch [morph-core/src/merge.rs](../../morph-core/src/merge.rs); `prepare_merge` and `execute_merge` keep working as the `--continue` back-half. They'll be reorganized in PR 4 when the new `morph merge` flow lands.
- Don't add merge state files (`MERGE_HEAD` etc) — that's PR 3.
- Don't modify the `Store` trait — that's PR 5.
- Don't write CLI specs — no CLI surface yet.

## Done criteria

- `cargo test -p morph-core --lib objmerge` — all 19 tests green.
- `cargo test --workspace` — no regressions elsewhere.
- `morph eval record` with `{ "tests_total": <new_count>, "tests_passed": <new_count>, "pass_rate": 1.0 }` plus a `coverage_pct` for the new module (if tarpaulin available; otherwise omit).
- Commit with metrics per [.cursor/rules/behavioral-commits.mdc](../../.cursor/rules/behavioral-commits.mdc).
- No version bump in PR 1 (no user-visible change).
