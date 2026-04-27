# Merging in Morph

Morph merges the same working tree Git merges, but it also reconciles the structured objects Git can't see — pipelines, evaluation suites, and the certified metrics that decide whether a merge is *behaviorally safe*. This document explains how the merge engine works end-to-end, from object-level reconciliation up through the user-facing `morph merge` flow and the textual-conflict fallback.

If you only need to use the feature, jump to the [user flow](#user-flow). If you want to understand or extend the engine, read the whole thing top to bottom.

---

## 1. What a merge has to reconcile

A Morph commit is more than a tree hash. It points at:

- a **Tree** (`tree`) — the file-tree snapshot, like Git
- a **Pipeline** (`pipeline`) — the DAG of operators that produced it (prompt calls, tool calls, transforms)
- an **EvalSuite** via `eval_contract.suite` — the cases and metrics the commit claims to satisfy
- observed metric scores in `eval_contract.observed_metrics`
- optional `evidence_refs` — Run / Trace hashes that back up the claim
- optional `env_constraints`, `morph_version`, `morph_instance`, …

Two commits that diverged from a common ancestor differ in *all* of these. A naive "git merge" of just the file tree would silently throw away the rest. Morph's merge engine reconciles each of them on its own terms:

| Object | Reconciled by | Lives in |
|---|---|---|
| Two `Tree`s + ancestor | 3-way structural tree merge with textual fallback | `morph-core/src/treemerge.rs` |
| Two `Pipeline`s + ancestor | Node/edge structural merge | `morph-core/src/pipemerge.rs` |
| Two `EvalSuite`s + ancestor | Case/metric structural merge | `morph-core/src/objmerge.rs` |
| Two metric vectors | Behavioral dominance check | `morph-core/src/merge.rs` (`check_dominance`) |
| Two `evidence_refs` lists | Deduped sorted union | `morph-core/src/merge.rs` (`union_evidence_refs`) |
| Two trees with file conflicts | `git merge-file` line-level | `morph-core/src/treemerge.rs` |

The common ancestor used everywhere is the **lowest common ancestor** (LCA) of the two commits in the parent DAG. It's computed once by `merge_base` in `morph-core/src/merge.rs` and threaded through every layer below.

---

## 2. Object-level merges

### 2.1 EvalSuite

Two suites can disagree on cases or metrics. `objmerge::merge_eval_suites(base, ours, theirs)`:

- **Cases**: keyed by `id`. Identical edits on both sides take that edit. Disjoint edits compose. Conflicting edits (both sides changed the same case differently) are an error the user must resolve manually before retrying.
- **Metrics**: keyed by `name`. Same rules; aggregation, threshold, and direction must agree on both sides for a non-conflicting metric.
- **Adds**: a case present on one side and absent on the ancestor and the other side is added to the merged suite.
- **Deletes**: a case present on the ancestor and absent on one side is removed.

### 2.2 Pipeline

`pipemerge::merge_pipelines(base, ours, theirs)` walks the pipeline DAG:

- **Nodes**: keyed by `id`. Same composition rules as eval cases. Identical edits compose; divergent edits on the same node are a conflict.
- **Edges**: edge sets are unioned. Two sides that both add the same edge end up with one copy.
- **Prompts** (`prompts: Vec<Hash>`): set union, sorted by hash for determinism.
- **Provenance**: rolled forward from whichever side touched it; if both sides set provenance differently it's a conflict.

### 2.3 Tree

This is the heaviest piece of the engine and lives in `morph-core/src/treemerge.rs`. The output is a `MergePlan` with three things:

1. The merged `Tree` object hash (when there are no conflicts).
2. A list of **`WorkdirOp`s** — `Write { path, bytes }` and `Delete { path }` operations to apply to the working tree.
3. A list of **conflicting paths** — files where Morph couldn't reconcile structurally and needs the user to resolve text first.

The structural rules:

- **Disjoint adds / deletes / edits compose.**
- **Both sides identical → take it.**
- **One side edited, the other untouched → take the edit.**
- **Both sides edited the same path differently** → write a *conflict marker* (`<<<<<<<` … `=======` … `>>>>>>>`) by shelling out to `git merge-file`, mark the path **unmerged** in the staging index, and add the path to `MergePlan.conflicts`.
- **Both sides deleted → delete.**
- **One side deleted while the other edited → conflict; user picks.**

Textual fallback uses Git's own `git merge-file` binary: write `base.txt`, `ours.txt`, `theirs.txt` into a tempdir, invoke the tool, capture the merged buffer with conflict markers, and write that to the workdir. We trust Git's diff3 implementation rather than reimplementing it.

The unmerged paths are recorded in `index.unmerged_entries: BTreeMap<Path, UnmergedEntry { base_blob, ours_blob, theirs_blob }>` so `morph status` can list them and `morph merge --continue` can drop them once the user runs `morph add` on a resolved file.

---

## 3. Behavioral dominance

A clean structural merge is necessary but not sufficient. Morph also requires the merged commit to **dominate both parents on every declared metric**.

`check_dominance(parent_a_metrics, parent_b_metrics, candidate_metrics, retired)` enforces:

- For each metric `m` not in `retired`:
  - if direction is `maximize`: `candidate[m] >= max(parent_a[m], parent_b[m])`
  - if direction is `minimize`: `candidate[m] <= min(parent_a[m], parent_b[m])`
- If a metric is missing from `candidate`, dominance fails for that metric (you can't merge code that drops a measured behavior unless you explicitly retire the metric).
- If `retired` lists a metric, that metric is not checked. Retiring is how you signal "this metric is no longer relevant — the pipeline changed enough that we shouldn't compare on it anymore."

Behavioral dominance only fires if `RepoPolicy.merge_policy` is `"dominance"` (the default). Setting it to `"none"` lets you merge purely on the structural result, which is occasionally what you want during rapid prototyping.

The CLI surfaces dominance failures with the exact metric, parent values, and candidate value so you know what regressed.

---

## 4. Evidence union (PR 6)

The merge commit's `evidence_refs` is the **deduped, sorted union** of `parent_a.evidence_refs` and `parent_b.evidence_refs`. The implementation is `merge::union_evidence_refs`. Two consequences:

- Run/Trace provenance is preserved across merge boundaries — `morph log` and `morph show` can still walk back to either parent's evidence.
- Bare-server fetches that traverse `evidence_refs` (PR 5) work transparently after a merge: the union is exactly the closure both parents needed.

If both parents lack evidence the field stays `None` (rather than `Some(vec![])`) so legacy commits round-trip through serde unchanged.

---

## 5. The merge state machine

A merge in progress is recorded as files inside `.morph/`. They are managed by `morph-core/src/merge_state.rs`:

| File | Means |
|---|---|
| `MERGE_HEAD` | hash of "their" commit being merged in |
| `ORIG_HEAD` | the HEAD commit before the merge started; consulted by `--abort` to restore the working tree |
| `MERGE_MSG` | proposed commit message; the user can edit before `--continue` |
| `MERGE_PIPELINE.json` | merged pipeline object (only when `start_merge` produced one with no node-level conflicts, **or** every node-level conflict was resolved by `morph merge resolve-node`) |
| `MERGE_SUITE` | hash of the merged eval suite (only when `start_merge` produced one) |

Plus the staging `index.json` carries `unmerged_entries` listing the conflicting paths.

`merge_flow.rs` orchestrates the lifecycle:

```
                    ┌─────────────────┐
                    │  no merge yet   │
                    └─────────────────┘
                              │
                  morph merge <branch>
                              │
                              ▼
   ┌──────────────────────────────────────┐
   │ MERGE_HEAD / ORIG_HEAD / MERGE_MSG   │
   │ (+ MERGE_PIPELINE / MERGE_SUITE)     │
   │ index.unmerged_entries populated     │
   │ workdir has conflict markers         │
   └──────────────────────────────────────┘
                  │                   │
       morph merge --abort     morph merge --continue
                  │                   │
                  │       (after the user runs `morph add`
                  │        on every resolved file; the
                  │        `unmerged_entries` map must now
                  │        be empty)
                  ▼                   ▼
          ┌─────────────┐    ┌─────────────────┐
          │ no merge yet│    │  merge commit   │
          └─────────────┘    │   with both     │
                             │   parents,      │
                             │  evidence_refs  │
                             │     unioned     │
                             └─────────────────┘
```

**`morph merge <branch>`** (`start_merge`) returns a `StartMergeOutcome`:

1. Read HEAD and `<branch>` tips.
2. Compute the LCA via `merge_base` and call `objmerge::merge_commits`.
3. If the relationship is `TrivialOutcome::AlreadyMerged` or `AlreadyAhead`, the CLI prints "Already up to date." and exits.
4. If the relationship is `TrivialOutcome::FastForward`, the CLI moves the local branch ref to `other` and updates the working tree via `checkout_tree`.
5. Otherwise (`TrivialOutcome::Diverged`), apply the engine's planned working-tree writes via `treemerge::apply_workdir_ops` (which writes diff3 conflict markers into the workdir for textual conflicts) and build a `MergePlan`.
6. If `outcome.needs_resolution` is `false`, the CLI immediately calls `continue_merge` to finalize the merge commit.
7. If `outcome.needs_resolution` is `true`, write `MERGE_HEAD` / `ORIG_HEAD` / `MERGE_MSG` (and `MERGE_PIPELINE` / `MERGE_SUITE` when relevant), record `unmerged_entries`, and exit non-zero so the user can resolve the listed conflicts and re-run `morph merge --continue`.

**`morph merge resolve-node <id> --pick ours|theirs|base`** (`resolve_node`):

1. Refuse when no merge is in progress.
2. Look up the named pipeline-node conflict and replace its entry in `MERGE_PIPELINE.json` with the chosen side.
3. Drop the node from the in-progress conflict list. When the list is empty, `--continue` is unblocked.

**`morph merge --continue`** (`continue_merge`):

1. Read state. Refuse if no merge is in progress.
2. Refuse if `unmerged_entries` is still non-empty (the user has not staged every resolution).
3. Refuse if the working tree has uncommitted changes to tracked files (`working_tree_clean`).
4. Build the merge commit with `parents = [HEAD, MERGE_HEAD]`, the merged tree/pipeline/suite, the recorded observed metrics, and `evidence_refs = union_evidence_refs(parents)`.
5. Apply behavioral dominance (`check_dominance`) unless the repo's `merge_policy` is `"none"` — fail loudly if the merged metrics regress.
6. Update the active branch ref, clear all `MERGE_*` state files, drop `unmerged_entries`, and return the new commit hash.

**`morph merge --abort`** (`abort_merge`):

1. Look up `ORIG_HEAD`. Refuse when no merge is in progress so users get a clear signal.
2. Restore the workdir to `ORIG_HEAD` via `checkout_tree`.
3. Clear all `MERGE_*` files and drop `unmerged_entries` from the index.

`morph status` reads this state and prints the same kind of "Unmerged paths" / "All conflicts fixed but you are still merging" hints Git users expect.

---

## 6. User flow

The simplest possible session, no conflicts:

```bash
morph merge feature
# 8d8b287a4c…   ← the new merge commit hash
```

A conflicting session:

```bash
morph merge feature
# Auto-merging failed for 1 path; conflict markers written to disk.
#   CONFLICT (content): src/lib.rs
# Run `morph status` for details, then `morph merge --continue`.

morph status
# Unmerged paths:
#   both modified: src/lib.rs

# Edit src/lib.rs, remove the <<<<<<<, =======, >>>>>>> markers.
morph add src/lib.rs
morph merge --continue
# 8d8b287a4c…
```

If your divergence touches a pipeline node, you'll instead see a node-level conflict:

```bash
morph merge feature
# Pipeline has 1 node-level conflict:
#   CONFLICT (pipeline node): generate
#   resolve with: morph merge resolve-node <id> --pick ours|theirs|base
morph merge resolve-node generate --pick theirs
morph merge --continue
```

If you change your mind:

```bash
morph merge --abort
# Merge aborted; working tree restored to ORIG_HEAD.
```

If a textual merge isn't enough — say `cargo test` fails on the merged tree — you can keep iterating:

```bash
# fix things in src/, run cargo test until clean
morph add src/lib.rs
morph merge --continue
```

If the merge succeeds structurally but **dominance** fails:

```
merge rejected: merged metrics do not dominate both parents
  pass_rate: candidate 0.91 < required 0.95
  mean_latency_ms: candidate 320 > required 280
```

The fix is one of:

- re-run your eval suite on the merged tree until the metrics dominate;
- explicitly retire an obsolete metric with `morph merge … --retire metric_name` (single-shot form) so the merge contract drops it;
- set `merge_policy = "none"` in `.morph/config.json` to opt out of dominance gating during prototyping (see [the policy block in v0-spec.md §11.1](v0-spec.md#111-repository-policy));
- decide this is the wrong merge and run `morph merge --abort`.

---

## 7. What about `morph pull --merge`?

`morph pull` is fast-forward only. When the local branch has diverged from its remote-tracking ref, `pull` raises `Diverged` with the local and remote tips. The same orchestrator is reachable via `morph pull --merge`, which:

1. Fetches the remote.
2. Calls `start_merge` against the remote-tracking ref.
3. Surfaces the same conflicts / fast-forward / merged outcomes.

In other words, **the same merge engine handles local merges, cross-branch merges, and post-fetch merges.** All three flow through `merge_flow.rs`, all three respect dominance, all three union evidence. The transport layer (PR 5) is invisible to the merge layer.

---

## 8. Where to look in the code

| File | Role |
|---|---|
| `morph-core/src/merge.rs` | `merge_base`, `prepare_merge`, `execute_merge`, `check_dominance`, `union_evidence_refs` |
| `morph-core/src/merge_flow.rs` | `start_merge`, `continue_merge`, `abort_merge`, `resolve_node`, `MergeProgress` |
| `morph-core/src/merge_state.rs` | `read_*` / `write_*` / `clear_merge_state` for the `.morph/MERGE_*` files |
| `morph-core/src/treemerge.rs` | 3-way Tree merge, `WorkdirOp`, `apply_workdir_ops`, textual fallback |
| `morph-core/src/text3way.rs` | Wrapper around `git merge-file` (the diff3 textual fallback) |
| `morph-core/src/pipemerge.rs` | Pipeline DAG merge |
| `morph-core/src/objmerge.rs` | EvalSuite case/metric merge, `TrivialOutcome` (`AlreadyMerged` / `AlreadyAhead` / `FastForward` / `Diverged`) |
| `morph-core/src/index.rs` | `StagingIndex.unmerged_entries`, `UnmergedEntry` |
| `morph-core/src/workdir.rs` | `working_tree_clean`, `checkout_tree` (used by `--abort` and fast-forward) |
| `morph-cli/src/main.rs` | `run_merge` dispatch (start / single-shot / `--continue` / `--abort` / `resolve-node`) |
| `morph-cli/tests/specs/merge*.yaml` | End-to-end spec tests for every branch above |

The closest thing to a "single big test" is `merge_flow::tests::continue_merge_writes_evidence_union_from_parents`, which sets up two parents with disjoint evidence and proves the merged commit unions them.
