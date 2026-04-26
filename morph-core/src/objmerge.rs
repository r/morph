//! Structural merge of Morph objects (multi-machine plan, PR 1).
//!
//! Merge in Morph is a structural reconciliation of typed objects:
//! - `EvalSuite` — auto-union or structural conflict on incompatible thresholds
//! - `Pipeline` — per-node 3-way merge (PR 2)
//! - `Tree` — per-path 3-way merge with text leaf (PR 3)
//! - `observed_metrics` — dominance check (existing, runs at `--continue`)
//! - `evidence_refs` — set union (PR 6)
//!
//! This module is the top-level dispatcher and exposes:
//! - [`merge_base`] — lowest common ancestor over `commit.parents`
//! - [`merge_commits`] — structural merge planning, returning conflicts
//! - [`MergeOutcome`], [`ObjConflict`], [`StructuralKind`], [`TrivialOutcome`]
//!
//! In PR 1 only the suite stage is implemented; pipeline and tree differences
//! are surfaced as `Structural` conflicts pointing to "not yet implemented"
//! and replaced by real merge logic in PR 2 / PR 3.

use crate::objects::MorphObject;
use crate::store::{MorphError, Store};
use crate::Hash;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

/// Categories of structural conflicts surfaced during merge planning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructuralKind {
    /// EvalSuites disagree on a metric's threshold/aggregation/direction.
    SuiteIncompatible,
    /// Pipeline graphs diverge in a way the auto-merger cannot resolve
    /// (e.g. node identity collision with conflicting bodies).
    PipelineDivergent,
    /// Trees diverge and the auto-merger cannot reconcile them. (PR 1
    /// uses this as a stub; PR 3 replaces it with real tree merge.)
    TreeDivergent,
}

impl std::fmt::Display for StructuralKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StructuralKind::SuiteIncompatible => write!(f, "suite-incompatible"),
            StructuralKind::PipelineDivergent => write!(f, "pipeline-divergent"),
            StructuralKind::TreeDivergent => write!(f, "tree-divergent"),
        }
    }
}

/// A single conflict surfaced during structural merge.
#[derive(Clone, Debug)]
pub enum ObjConflict {
    /// Suite or pipeline level — must be resolved before tree merge.
    Structural { kind: StructuralKind, message: String },
    /// File-level text or binary conflict in the working tree.
    Textual {
        path: PathBuf,
        base: Option<Hash>,
        ours: Option<Hash>,
        theirs: Option<Hash>,
    },
    /// Merged metrics fail dominance against one or both parents.
    Behavioral { violations: Vec<crate::merge::DominanceViolation> },
}

/// Coarse classification of how two commits relate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrivialOutcome {
    /// `head == other` — nothing to merge.
    AlreadyMerged,
    /// `other` is a proper ancestor of `head` — caller already has it.
    AlreadyAhead,
    /// `head` is a proper ancestor of `other` — caller can fast-forward.
    FastForward,
    /// Genuinely diverged — structural merge is required.
    Diverged,
}

/// Result of structural merge planning. Always returned (even for trivial
/// cases) so the caller can decide whether to fast-forward, no-op, or run
/// the full merge state machine.
///
/// In PR 1 the suite stage is implemented; pipeline and tree differences
/// are surfaced as `Structural { kind: PipelineDivergent, ... }` conflicts
/// and replaced by real merge logic in PR 2 / PR 3.
#[derive(Clone, Debug)]
pub struct MergeOutcome {
    pub head: Hash,
    pub other: Hash,
    pub base: Option<Hash>,
    /// Effective union eval suite (post-retirement) when reconciliation
    /// succeeded. `None` for trivial outcomes or when suites conflict.
    pub union_suite: Option<crate::objects::EvalSuite>,
    /// Effective merged pipeline when 3-way pipeline merge succeeded.
    /// `None` for trivial outcomes, when pipelines are identical, or
    /// when pipemerge surfaced node-level conflicts.
    pub union_pipeline: Option<crate::objects::Pipeline>,
    /// Effective merged tree hash when 3-way tree merge produced a result.
    /// May be set even when conflicts exist (treemerge always returns a
    /// best-effort preview); the CLI must check `conflicts` before using
    /// it as the merge commit's tree.
    pub union_tree: Option<Hash>,
    /// Working-tree operations planned by the tree merger. Applied by
    /// the CLI after dominance gating in PR 4.
    pub working_writes: Vec<crate::treemerge::WorkdirOp>,
    /// Pipeline-node conflicts surfaced by the structural pipeline
    /// merger. Each entry is mirrored as an `ObjConflict::Structural {
    /// kind: PipelineDivergent }` in `conflicts`, but kept here in
    /// typed form so the CLI can drive `morph merge resolve-node`
    /// without parsing a string.
    pub pipeline_node_conflicts: Vec<crate::pipemerge::NodeConflict>,
    /// Conflicts that must be resolved before `morph merge --continue`.
    pub conflicts: Vec<ObjConflict>,
    pub trivial: TrivialOutcome,
}

impl std::fmt::Display for ObjConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObjConflict::Structural { kind, message } => {
                write!(f, "structural conflict ({}): {}", kind, message)
            }
            ObjConflict::Textual { path, .. } => {
                write!(f, "textual conflict in {}", path.display())
            }
            ObjConflict::Behavioral { violations } => {
                write!(f, "behavioral conflict: {} dominance violation(s)", violations.len())
            }
        }
    }
}

/// Lowest common ancestor of two commits, walking `commit.parents`.
/// Returns `Ok(None)` if the histories are disjoint.
///
/// Algorithm: parallel BFS from `a` and `b`. Each side records the set of
/// commits it has reached. The first commit popped from one side that is
/// already in the other side's reached set is the lowest common ancestor.
/// Ties are broken by BFS depth (shallowest first).
pub fn merge_base(store: &dyn Store, a: &Hash, b: &Hash) -> Result<Option<Hash>, MorphError> {
    let mut reached_a: HashSet<Hash> = HashSet::new();
    let mut reached_b: HashSet<Hash> = HashSet::new();
    let mut queue_a: VecDeque<Hash> = VecDeque::new();
    let mut queue_b: VecDeque<Hash> = VecDeque::new();

    reached_a.insert(*a);
    reached_b.insert(*b);
    queue_a.push_back(*a);
    queue_b.push_back(*b);

    while !queue_a.is_empty() || !queue_b.is_empty() {
        if let Some(h) = queue_a.pop_front() {
            if reached_b.contains(&h) {
                return Ok(Some(h));
            }
            push_parents(store, &h, &mut reached_a, &mut queue_a)?;
        }
        if let Some(h) = queue_b.pop_front() {
            if reached_a.contains(&h) {
                return Ok(Some(h));
            }
            push_parents(store, &h, &mut reached_b, &mut queue_b)?;
        }
    }
    Ok(None)
}

/// Top-level structural merge dispatcher. Plans the merge of `other` into
/// `head` and returns the conflicts (if any) that block `--continue`.
///
/// PR 1 scope: trivial outcomes (`AlreadyMerged`, `AlreadyAhead`,
/// `FastForward`) and the EvalSuite reconciliation stage. Pipeline and
/// tree merges land in PR 2 and PR 3.
pub fn merge_commits(
    store: &dyn Store,
    head: &Hash,
    other: &Hash,
    _retire: Option<&[String]>,
) -> Result<MergeOutcome, MorphError> {
    if head == other {
        return Ok(MergeOutcome {
            head: *head,
            other: *other,
            base: Some(*head),
            union_suite: None,
            union_pipeline: None,
            union_tree: None,
            working_writes: vec![],
            pipeline_node_conflicts: vec![],
            conflicts: vec![],
            trivial: TrivialOutcome::AlreadyMerged,
        });
    }
    let base = merge_base(store, head, other)?;
    let trivial = match base {
        Some(b) if b == *head => TrivialOutcome::FastForward,
        Some(b) if b == *other => TrivialOutcome::AlreadyAhead,
        _ => TrivialOutcome::Diverged,
    };

    let mut conflicts: Vec<ObjConflict> = Vec::new();
    let mut union_suite: Option<crate::objects::EvalSuite> = None;
    let mut union_pipeline: Option<crate::objects::Pipeline> = None;
    let mut union_tree: Option<Hash> = None;
    let mut working_writes: Vec<crate::treemerge::WorkdirOp> = Vec::new();
    let mut pipeline_node_conflicts: Vec<crate::pipemerge::NodeConflict> = Vec::new();

    // Suite / pipeline / tree stages — only run when the merge is non-trivial.
    if matches!(trivial, TrivialOutcome::Diverged) {
        match reconcile_suites(store, head, other, _retire) {
            Ok(s) => union_suite = s,
            Err(c) => conflicts.push(c),
        }

        let head_commit = load_commit(store, head)?;
        let other_commit = load_commit(store, other)?;

        // Pipeline stage: 3-way structural merge via pipemerge. If pipelines
        // are identical there's nothing to do. If either side's pipeline
        // hash doesn't resolve to a stored Pipeline (e.g. placeholder hash
        // in tests) we fall back to the legacy stub conflict.
        if head_commit.pipeline != other_commit.pipeline {
            match resolve_pipeline_merge(store, &head_commit, &other_commit, base.as_ref()) {
                Ok(out) => {
                    if out.conflicts.is_empty() {
                        union_pipeline = Some(out.merged);
                    } else {
                        for nc in &out.conflicts {
                            conflicts.push(ObjConflict::Structural {
                                kind: StructuralKind::PipelineDivergent,
                                message: format!("node '{}': {}", nc.id, nc.axis),
                            });
                        }
                        pipeline_node_conflicts.extend(out.conflicts.into_iter());
                    }
                }
                Err(_) => {
                    conflicts.push(ObjConflict::Structural {
                        kind: StructuralKind::PipelineDivergent,
                        message: "pipeline objects unavailable for structural merge".to_string(),
                    });
                }
            }
        }

        // Tree stage: 3-way structural merge via treemerge. If trees are
        // identical there is nothing to do. If either side's tree hash
        // can't be resolved (e.g. placeholder hashes in legacy tests) we
        // fall back to the legacy stub conflict so prior tests keep
        // passing.
        if head_commit.tree != other_commit.tree {
            match resolve_tree_merge(store, &head_commit, &other_commit, base.as_ref()) {
                Ok(out) => {
                    union_tree = out.merged_tree;
                    working_writes = out.working_writes;
                    for c in out.conflicts {
                        conflicts.push(c);
                    }
                }
                Err(_) => {
                    conflicts.push(ObjConflict::Structural {
                        kind: StructuralKind::TreeDivergent,
                        message: "tree objects unavailable for structural merge".to_string(),
                    });
                }
            }
        }
    }

    Ok(MergeOutcome {
        head: *head,
        other: *other,
        base,
        union_suite,
        union_pipeline,
        union_tree,
        working_writes,
        pipeline_node_conflicts,
        conflicts,
        trivial,
    })
}

/// Load the Tree hashes for `head`, `other`, and (optionally) the base
/// commit, then run the 3-way `treemerge::merge_trees`. Returns `Err`
/// when any tree hash fails to resolve, so the caller can fall back to
/// a stub conflict.
fn resolve_tree_merge(
    store: &dyn Store,
    head_commit: &crate::objects::Commit,
    other_commit: &crate::objects::Commit,
    base_hash: Option<&Hash>,
) -> Result<crate::treemerge::TreeMergeOutcome, MorphError> {
    let head_tree = parse_tree_hash(&head_commit.tree)?;
    let other_tree = parse_tree_hash(&other_commit.tree)?;
    let base_tree = match base_hash {
        Some(b) => {
            let bc = load_commit(store, b)?;
            parse_tree_hash(&bc.tree).ok()
        }
        None => None,
    };
    crate::treemerge::merge_trees(store, base_tree.as_ref(), &head_tree, &other_tree)
}

fn parse_tree_hash(opt_tree: &Option<String>) -> Result<Hash, MorphError> {
    let s = opt_tree
        .as_ref()
        .ok_or_else(|| MorphError::NotFound("commit has no tree".into()))?;
    Hash::from_hex(s).map_err(|_| MorphError::InvalidHash(s.clone()))
}

/// Load the Pipeline objects for `head`, `other`, and (optionally) the base
/// commit, then run the 3-way `pipemerge::merge_pipelines`. Returns `Err`
/// when any pipeline hash fails to resolve, so the caller can fall back to
/// a stub conflict.
fn resolve_pipeline_merge(
    store: &dyn Store,
    head_commit: &crate::objects::Commit,
    other_commit: &crate::objects::Commit,
    base_hash: Option<&Hash>,
) -> Result<crate::pipemerge::PipelineMergeOutcome, MorphError> {
    let head_pipe = load_pipeline_by_hex(store, &head_commit.pipeline)?;
    let other_pipe = load_pipeline_by_hex(store, &other_commit.pipeline)?;
    let base_pipe = match base_hash {
        Some(b) => {
            let bc = load_commit(store, b)?;
            load_pipeline_by_hex(store, &bc.pipeline).ok()
        }
        None => None,
    };
    Ok(crate::pipemerge::merge_pipelines(
        base_pipe.as_ref(),
        &head_pipe,
        &other_pipe,
    ))
}

fn load_pipeline_by_hex(store: &dyn Store, hex: &str) -> Result<crate::objects::Pipeline, MorphError> {
    let hash = Hash::from_hex(hex).map_err(|_| MorphError::InvalidHash(hex.to_string()))?;
    match store.get(&hash)? {
        MorphObject::Pipeline(p) => Ok(p),
        _ => Err(MorphError::Serialization(format!("expected Pipeline at {}", hash))),
    }
}

/// Suite-stage reconciliation. Loads each commit's `eval_contract.suite`,
/// retires the requested metrics from each side, then unions the result.
/// Returns `Ok(Some(suite))` on success, `Ok(None)` when neither side has
/// a suite, or `Err(ObjConflict::Structural)` on incompatibility.
fn reconcile_suites(
    store: &dyn Store,
    head: &Hash,
    other: &Hash,
    retire: Option<&[String]>,
) -> Result<Option<crate::objects::EvalSuite>, ObjConflict> {
    let head_suite = load_suite_for_commit(store, head)
        .map_err(|e| structural(StructuralKind::SuiteIncompatible, format!("head: {}", e)))?;
    let other_suite = load_suite_for_commit(store, other)
        .map_err(|e| structural(StructuralKind::SuiteIncompatible, format!("other: {}", e)))?;

    let head_suite = match head_suite {
        Some(s) => s,
        None => return Ok(other_suite),
    };
    let other_suite = match other_suite {
        Some(s) => s,
        None => return Ok(Some(head_suite)),
    };

    // Apply retirement first so that a retired metric never produces a
    // SuiteIncompatible conflict for differing thresholds.
    let head_retired = match retire {
        Some(r) if !r.is_empty() => {
            let r_vec: Vec<String> = r.to_vec();
            // Only retire metrics that actually exist in this side's suite.
            let to_retire: Vec<String> = r_vec
                .iter()
                .filter(|name| head_suite.metrics.iter().any(|m| &m.name == *name))
                .cloned()
                .collect();
            if to_retire.is_empty() {
                head_suite
            } else {
                crate::metrics::retire_metrics(&head_suite, &to_retire).map_err(|e| {
                    structural(StructuralKind::SuiteIncompatible, format!("retire (head): {}", e))
                })?
            }
        }
        _ => head_suite,
    };
    let other_retired = match retire {
        Some(r) if !r.is_empty() => {
            let r_vec: Vec<String> = r.to_vec();
            let to_retire: Vec<String> = r_vec
                .iter()
                .filter(|name| other_suite.metrics.iter().any(|m| &m.name == *name))
                .cloned()
                .collect();
            if to_retire.is_empty() {
                other_suite
            } else {
                crate::metrics::retire_metrics(&other_suite, &to_retire).map_err(|e| {
                    structural(StructuralKind::SuiteIncompatible, format!("retire (other): {}", e))
                })?
            }
        }
        _ => other_suite,
    };

    let union = crate::metrics::union_suites(&head_retired, &other_retired)
        .map_err(|e| structural(StructuralKind::SuiteIncompatible, e.to_string()))?;
    Ok(Some(union))
}

fn structural(kind: StructuralKind, message: String) -> ObjConflict {
    ObjConflict::Structural { kind, message }
}

fn load_commit(store: &dyn Store, commit_hash: &Hash) -> Result<crate::objects::Commit, MorphError> {
    match store.get(commit_hash)? {
        MorphObject::Commit(c) => Ok(c),
        _ => Err(MorphError::Serialization(format!("not a commit: {}", commit_hash))),
    }
}

/// Load the EvalSuite referenced by `commit.eval_contract.suite`, returning
/// `None` if the commit references the all-zero placeholder hash (used by
/// raw test commits) or if the suite hash does not resolve to an EvalSuite.
fn load_suite_for_commit(
    store: &dyn Store,
    commit_hash: &Hash,
) -> Result<Option<crate::objects::EvalSuite>, MorphError> {
    let commit = match store.get(commit_hash)? {
        MorphObject::Commit(c) => c,
        _ => return Err(MorphError::Serialization(format!("not a commit: {}", commit_hash))),
    };
    let suite_hex = commit.eval_contract.suite;
    if suite_hex.chars().all(|c| c == '0') {
        return Ok(None);
    }
    let suite_hash = Hash::from_hex(&suite_hex)
        .map_err(|_| MorphError::InvalidHash(suite_hex.clone()))?;
    match store.get(&suite_hash)? {
        MorphObject::EvalSuite(s) => Ok(Some(s)),
        _ => Err(MorphError::Serialization(format!(
            "expected EvalSuite at {}",
            suite_hash
        ))),
    }
}

fn push_parents(
    store: &dyn Store,
    commit_hash: &Hash,
    reached: &mut HashSet<Hash>,
    queue: &mut VecDeque<Hash>,
) -> Result<(), MorphError> {
    if let Ok(MorphObject::Commit(c)) = store.get(commit_hash) {
        for p in &c.parents {
            if let Ok(ph) = Hash::from_hex(p) {
                if reached.insert(ph) {
                    queue.push_back(ph);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::Hash;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    fn setup_repo() -> (tempfile::TempDir, Box<dyn Store>) {
        let dir = tempfile::tempdir().unwrap();
        let _ = crate::repo::init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        let store = crate::repo::open_store(&morph_dir).unwrap();
        (dir, store)
    }

    fn make_commit(store: &dyn Store, root: &Path, msg: &str) -> Hash {
        std::fs::write(root.join(format!("{}.txt", msg.replace(' ', "_"))), msg).unwrap();
        crate::add_paths(store, root, &[PathBuf::from(".")]).unwrap();
        crate::create_tree_commit(
            store,
            root,
            None,
            None,
            BTreeMap::new(),
            msg.to_string(),
            None,
            Some("0.3"),
        )
        .unwrap()
    }

    /// Reset HEAD to `target` so the next `make_commit` will be a sibling.
    fn detach_to(store: &dyn Store, target: &Hash) {
        crate::set_head_detached(store, target).unwrap();
    }

    /// Construct a raw commit with explicit parents, bypassing HEAD/working
    /// tree. Useful for building disjoint chains and criss-cross histories.
    fn raw_commit(store: &dyn Store, parents: &[Hash], msg: &str) -> Hash {
        raw_commit_full(store, parents, msg, None, None, None)
    }

    /// Like `raw_commit` but lets the test set explicit suite/pipeline/tree
    /// hashes so we can drive the reconciliation stages.
    fn raw_commit_full(
        store: &dyn Store,
        parents: &[Hash],
        msg: &str,
        suite: Option<&Hash>,
        pipeline: Option<&Hash>,
        tree: Option<&Hash>,
    ) -> Hash {
        use crate::objects::{Commit, EvalContract, MorphObject};
        let zero = Hash::from_hex(&"0".repeat(64)).unwrap();
        let commit = Commit {
            tree: tree.map(|h| h.to_string()),
            pipeline: pipeline.unwrap_or(&zero).to_string(),
            parents: parents.iter().map(|h| h.to_string()).collect(),
            message: msg.to_string(),
            timestamp: format!("2026-01-01T00:00:00Z#{}", msg),
            author: "test".to_string(),
            contributors: None,
            eval_contract: EvalContract {
                suite: suite.unwrap_or(&zero).to_string(),
                observed_metrics: BTreeMap::new(),
            },
            env_constraints: None,
            evidence_refs: None,
            morph_version: Some("0.3".to_string()),
            morph_instance: None,
        };
        store.put(&MorphObject::Commit(commit)).unwrap()
    }

    /// Store an EvalSuite object and return its hash.
    fn put_suite(store: &dyn Store, metrics: Vec<crate::objects::EvalMetric>) -> Hash {
        use crate::objects::{EvalSuite, MorphObject};
        store
            .put(&MorphObject::EvalSuite(EvalSuite { cases: vec![], metrics }))
            .unwrap()
    }

    /// Store a Pipeline object and return its hash.
    fn put_pipeline(
        store: &dyn Store,
        nodes: Vec<crate::objects::PipelineNode>,
        edges: Vec<crate::objects::PipelineEdge>,
    ) -> Hash {
        use crate::objects::{MorphObject, Pipeline, PipelineGraph};
        let p = Pipeline {
            graph: PipelineGraph { nodes, edges },
            prompts: vec![],
            eval_suite: None,
            attribution: None,
            provenance: None,
        };
        store.put(&MorphObject::Pipeline(p)).unwrap()
    }

    fn pnode(id: &str, kind: &str) -> crate::objects::PipelineNode {
        crate::objects::PipelineNode {
            id: id.to_string(),
            kind: kind.to_string(),
            ref_: None,
            params: BTreeMap::new(),
            env: None,
        }
    }

    fn pnode_param(id: &str, kind: &str, key: &str, val: &str) -> crate::objects::PipelineNode {
        let mut params = BTreeMap::new();
        params.insert(key.to_string(), serde_json::json!(val));
        crate::objects::PipelineNode {
            id: id.to_string(),
            kind: kind.to_string(),
            ref_: None,
            params,
            env: None,
        }
    }

    fn metric(name: &str, threshold: f64) -> crate::objects::EvalMetric {
        crate::objects::EvalMetric {
            name: name.to_string(),
            aggregation: "mean".to_string(),
            threshold,
            direction: "maximize".to_string(),
        }
    }

    // ── merge_base (LCA) ─────────────────────────────────────────────

    #[test]
    fn merge_base_self_returns_self() {
        let (dir, store) = setup_repo();
        let c = make_commit(store.as_ref(), dir.path(), "first");
        assert_eq!(merge_base(store.as_ref(), &c, &c).unwrap(), Some(c));
    }

    #[test]
    fn merge_base_ancestor_returns_ancestor() {
        let (dir, store) = setup_repo();
        let c1 = make_commit(store.as_ref(), dir.path(), "first");
        let c2 = make_commit(store.as_ref(), dir.path(), "second");
        assert_eq!(merge_base(store.as_ref(), &c2, &c1).unwrap(), Some(c1));
    }

    #[test]
    fn merge_base_symmetric() {
        let (dir, store) = setup_repo();
        let c1 = make_commit(store.as_ref(), dir.path(), "first");
        let c2 = make_commit(store.as_ref(), dir.path(), "second");
        assert_eq!(merge_base(store.as_ref(), &c1, &c2).unwrap(), Some(c1));
    }

    #[test]
    fn merge_base_two_siblings() {
        // c0 ─┬─> ca
        //     └─> cb     LCA(ca, cb) == c0
        let (dir, store) = setup_repo();
        let c0 = make_commit(store.as_ref(), dir.path(), "base");
        let ca = make_commit(store.as_ref(), dir.path(), "left");
        detach_to(store.as_ref(), &c0);
        let cb = make_commit(store.as_ref(), dir.path(), "right");
        assert_eq!(merge_base(store.as_ref(), &ca, &cb).unwrap(), Some(c0));
        assert_eq!(merge_base(store.as_ref(), &cb, &ca).unwrap(), Some(c0));
    }

    #[test]
    fn merge_base_disjoint_returns_none() {
        // x1 → x2     y1 → y2     no shared history
        let (_dir, store) = setup_repo();
        let x1 = raw_commit(store.as_ref(), &[], "x1");
        let x2 = raw_commit(store.as_ref(), &[x1], "x2");
        let y1 = raw_commit(store.as_ref(), &[], "y1");
        let y2 = raw_commit(store.as_ref(), &[y1], "y2");
        assert_eq!(merge_base(store.as_ref(), &x2, &y2).unwrap(), None);
    }

    #[test]
    fn merge_base_criss_cross_deterministic() {
        //   r ─┬─> a ─┐
        //      └─> b ─┴─> ma (parents: a, b)
        //                 └─> mb (parents: b, a)
        // Both ma and mb have two valid LCAs; we just want a stable result.
        let (_dir, store) = setup_repo();
        let r = raw_commit(store.as_ref(), &[], "r");
        let a = raw_commit(store.as_ref(), &[r], "a");
        let b = raw_commit(store.as_ref(), &[r], "b");
        let ma = raw_commit(store.as_ref(), &[a, b], "ma");
        let mb = raw_commit(store.as_ref(), &[b, a], "mb");
        let base = merge_base(store.as_ref(), &ma, &mb).unwrap();
        assert!(base == Some(a) || base == Some(b),
            "merge_base must pick one of the two valid LCAs, got {:?}", base);
    }

    // ── ObjConflict / display ────────────────────────────────────────

    #[test]
    fn obj_conflict_structural_display() {
        let c = ObjConflict::Structural {
            kind: StructuralKind::SuiteIncompatible,
            message: "metric 'acc' has different thresholds".into(),
        };
        let s = format!("{}", c);
        assert!(s.contains("structural"), "got: {}", s);
        assert!(s.contains("acc"), "got: {}", s);
    }

    // ── merge_commits: trivial outcomes ──────────────────────────────

    #[test]
    fn merge_outcome_already_merged() {
        let (dir, store) = setup_repo();
        let c = make_commit(store.as_ref(), dir.path(), "first");
        let outcome = merge_commits(store.as_ref(), &c, &c, None).unwrap();
        assert_eq!(outcome.trivial, TrivialOutcome::AlreadyMerged);
        assert_eq!(outcome.head, c);
        assert_eq!(outcome.other, c);
        assert!(outcome.conflicts.is_empty());
    }

    #[test]
    fn merge_outcome_fast_forward() {
        // head is ancestor of other → caller can fast-forward.
        let (dir, store) = setup_repo();
        let c1 = make_commit(store.as_ref(), dir.path(), "first");
        let c2 = make_commit(store.as_ref(), dir.path(), "second");
        let outcome = merge_commits(store.as_ref(), &c1, &c2, None).unwrap();
        assert_eq!(outcome.trivial, TrivialOutcome::FastForward);
        assert_eq!(outcome.base, Some(c1));
        assert!(outcome.conflicts.is_empty());
    }

    #[test]
    fn merge_outcome_diverged() {
        let (dir, store) = setup_repo();
        let c0 = make_commit(store.as_ref(), dir.path(), "base");
        let ca = make_commit(store.as_ref(), dir.path(), "left");
        detach_to(store.as_ref(), &c0);
        let cb = make_commit(store.as_ref(), dir.path(), "right");
        let outcome = merge_commits(store.as_ref(), &ca, &cb, None).unwrap();
        assert_eq!(outcome.trivial, TrivialOutcome::Diverged);
        assert_eq!(outcome.base, Some(c0));
    }

    // ── merge_commits: suite stage ───────────────────────────────────

    #[test]
    fn merge_commits_unions_compatible_suites() {
        let (_dir, store) = setup_repo();
        let s_a = put_suite(store.as_ref(), vec![metric("acc", 0.8)]);
        let s_b = put_suite(store.as_ref(), vec![metric("speed", 0.5)]);
        let r = raw_commit_full(store.as_ref(), &[], "r", Some(&s_a), None, None);
        let a = raw_commit_full(store.as_ref(), &[r], "a", Some(&s_a), None, None);
        let b = raw_commit_full(store.as_ref(), &[r], "b", Some(&s_b), None, None);

        let outcome = merge_commits(store.as_ref(), &a, &b, None).unwrap();
        assert_eq!(outcome.trivial, TrivialOutcome::Diverged);
        assert!(outcome.conflicts.is_empty(), "got conflicts: {:?}", outcome.conflicts);
        let suite = outcome.union_suite.expect("union_suite must be populated");
        let names: Vec<_> = suite.metrics.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"acc"));
        assert!(names.contains(&"speed"));
    }

    #[test]
    fn merge_commits_returns_structural_conflict_incompatible_thresholds() {
        let (_dir, store) = setup_repo();
        let s_a = put_suite(store.as_ref(), vec![metric("acc", 0.8)]);
        let s_b = put_suite(store.as_ref(), vec![metric("acc", 0.95)]);
        let r = raw_commit_full(store.as_ref(), &[], "r", Some(&s_a), None, None);
        let a = raw_commit_full(store.as_ref(), &[r], "a", Some(&s_a), None, None);
        let b = raw_commit_full(store.as_ref(), &[r], "b", Some(&s_b), None, None);

        let outcome = merge_commits(store.as_ref(), &a, &b, None).unwrap();
        assert!(outcome.union_suite.is_none(), "union_suite must be None on conflict");
        let kinds: Vec<_> = outcome
            .conflicts
            .iter()
            .filter_map(|c| match c {
                ObjConflict::Structural { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect();
        assert!(
            kinds.contains(&StructuralKind::SuiteIncompatible),
            "expected SuiteIncompatible, got: {:?}",
            outcome.conflicts
        );
    }

    #[test]
    fn merge_commits_respects_retired_metrics() {
        // Both branches define `old` with mismatched thresholds, plus `acc`
        // (consistent). With `retire = ["old"]`, the conflict must vanish
        // and `union_suite` must contain only `acc`.
        let (_dir, store) = setup_repo();
        let s_a = put_suite(
            store.as_ref(),
            vec![metric("acc", 0.8), metric("old", 0.5)],
        );
        let s_b = put_suite(
            store.as_ref(),
            vec![metric("acc", 0.8), metric("old", 0.9)],
        );
        let r = raw_commit_full(store.as_ref(), &[], "r", Some(&s_a), None, None);
        let a = raw_commit_full(store.as_ref(), &[r], "a", Some(&s_a), None, None);
        let b = raw_commit_full(store.as_ref(), &[r], "b", Some(&s_b), None, None);

        let retire = vec!["old".to_string()];
        let outcome = merge_commits(store.as_ref(), &a, &b, Some(&retire)).unwrap();
        assert!(
            outcome.conflicts.is_empty(),
            "retire should suppress conflict, got: {:?}",
            outcome.conflicts
        );
        let suite = outcome.union_suite.expect("union_suite must be populated");
        let names: Vec<_> = suite.metrics.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["acc"]);
    }

    #[test]
    fn merge_commits_uses_explicit_suite_when_compatible() {
        // Both commits already point at the same suite hash — reconciliation
        // is a pass-through.
        let (_dir, store) = setup_repo();
        let s = put_suite(store.as_ref(), vec![metric("acc", 0.8)]);
        let r = raw_commit_full(store.as_ref(), &[], "r", Some(&s), None, None);
        let a = raw_commit_full(store.as_ref(), &[r], "a", Some(&s), None, None);
        let b = raw_commit_full(store.as_ref(), &[r], "b", Some(&s), None, None);

        let outcome = merge_commits(store.as_ref(), &a, &b, None).unwrap();
        assert!(outcome.conflicts.is_empty(), "got: {:?}", outcome.conflicts);
        let suite = outcome.union_suite.expect("union_suite must be populated");
        assert_eq!(suite.metrics.len(), 1);
        assert_eq!(suite.metrics[0].name, "acc");
    }

    // ── merge_commits: pipeline 3-way merge integration (PR 2) ───────

    #[test]
    fn merge_commits_resolves_pipeline_when_pipemerge_clean() {
        // Both branches add a disjoint node on top of the base pipeline.
        // pipemerge unions cleanly → no PipelineDivergent conflict and
        // outcome.union_pipeline is populated.
        let (_dir, store) = setup_repo();
        let s = put_suite(store.as_ref(), vec![metric("acc", 0.8)]);
        let p_base = put_pipeline(store.as_ref(), vec![pnode("a", "prompt_call")], vec![]);
        let p_ours = put_pipeline(
            store.as_ref(),
            vec![pnode("a", "prompt_call"), pnode("b", "tool_call")],
            vec![],
        );
        let p_theirs = put_pipeline(
            store.as_ref(),
            vec![pnode("a", "prompt_call"), pnode("c", "transform")],
            vec![],
        );

        let r = raw_commit_full(store.as_ref(), &[], "r", Some(&s), Some(&p_base), None);
        let a = raw_commit_full(store.as_ref(), &[r], "a", Some(&s), Some(&p_ours), None);
        let b = raw_commit_full(store.as_ref(), &[r], "b", Some(&s), Some(&p_theirs), None);

        let outcome = merge_commits(store.as_ref(), &a, &b, None).unwrap();
        let pipeline_conflicts: Vec<_> = outcome
            .conflicts
            .iter()
            .filter(|c| matches!(c, ObjConflict::Structural { kind: StructuralKind::PipelineDivergent, .. }))
            .collect();
        assert!(
            pipeline_conflicts.is_empty(),
            "expected no PipelineDivergent conflicts, got: {:?}",
            pipeline_conflicts
        );
        let merged = outcome
            .union_pipeline
            .as_ref()
            .expect("union_pipeline must be populated when pipemerge is clean");
        let ids: Vec<_> = merged.graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"a") && ids.contains(&"b") && ids.contains(&"c"));
    }

    #[test]
    fn merge_commits_surfaces_node_conflicts_as_structural() {
        // Both branches modify the same node 'a' differently → pipemerge
        // produces a ModifyModify NodeConflict, which the dispatcher must
        // surface as one ObjConflict::Structural { kind: PipelineDivergent }.
        let (_dir, store) = setup_repo();
        let s = put_suite(store.as_ref(), vec![metric("acc", 0.8)]);
        let p_base = put_pipeline(
            store.as_ref(),
            vec![pnode_param("a", "prompt_call", "k", "old")],
            vec![],
        );
        let p_ours = put_pipeline(
            store.as_ref(),
            vec![pnode_param("a", "prompt_call", "k", "v1")],
            vec![],
        );
        let p_theirs = put_pipeline(
            store.as_ref(),
            vec![pnode_param("a", "prompt_call", "k", "v2")],
            vec![],
        );

        let r = raw_commit_full(store.as_ref(), &[], "r", Some(&s), Some(&p_base), None);
        let a = raw_commit_full(store.as_ref(), &[r], "a", Some(&s), Some(&p_ours), None);
        let b = raw_commit_full(store.as_ref(), &[r], "b", Some(&s), Some(&p_theirs), None);

        let outcome = merge_commits(store.as_ref(), &a, &b, None).unwrap();
        let pipeline_conflicts: Vec<_> = outcome
            .conflicts
            .iter()
            .filter(|c| matches!(c, ObjConflict::Structural { kind: StructuralKind::PipelineDivergent, .. }))
            .collect();
        assert_eq!(
            pipeline_conflicts.len(),
            1,
            "expected one PipelineDivergent per NodeConflict, got: {:?}",
            outcome.conflicts
        );
        // Conflict message must mention the node id so callers can render it.
        let msg = match pipeline_conflicts[0] {
            ObjConflict::Structural { message, .. } => message,
            _ => unreachable!(),
        };
        assert!(msg.contains("'a'") || msg.contains("\"a\"") || msg.contains(": a "),
            "expected message to mention node 'a', got: {}", msg);
        assert!(outcome.union_pipeline.is_none(), "union_pipeline must be None on conflict");
    }

    // ── merge_commits: pipeline / tree stage stubs ───────────────────

    #[test]
    fn merge_commits_emits_pipeline_stub_when_pipelines_differ() {
        let (_dir, store) = setup_repo();
        let s = put_suite(store.as_ref(), vec![metric("acc", 0.8)]);
        let p_a = Hash::from_hex(&format!("{:0>64}", "a")).unwrap();
        let p_b = Hash::from_hex(&format!("{:0>64}", "b")).unwrap();
        let r = raw_commit_full(store.as_ref(), &[], "r", Some(&s), Some(&p_a), None);
        let a = raw_commit_full(store.as_ref(), &[r], "a", Some(&s), Some(&p_a), None);
        let b = raw_commit_full(store.as_ref(), &[r], "b", Some(&s), Some(&p_b), None);

        let outcome = merge_commits(store.as_ref(), &a, &b, None).unwrap();
        let kinds: Vec<_> = outcome
            .conflicts
            .iter()
            .filter_map(|c| match c {
                ObjConflict::Structural { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect();
        assert!(
            kinds.contains(&StructuralKind::PipelineDivergent),
            "expected PipelineDivergent stub, got: {:?}",
            outcome.conflicts
        );
    }

    #[test]
    fn merge_commits_emits_tree_stub_when_trees_differ() {
        let (_dir, store) = setup_repo();
        let s = put_suite(store.as_ref(), vec![metric("acc", 0.8)]);
        let t_a = Hash::from_hex(&format!("{:0>64}", "a")).unwrap();
        let t_b = Hash::from_hex(&format!("{:0>64}", "b")).unwrap();
        let r = raw_commit_full(store.as_ref(), &[], "r", Some(&s), None, Some(&t_a));
        let a = raw_commit_full(store.as_ref(), &[r], "a", Some(&s), None, Some(&t_a));
        let b = raw_commit_full(store.as_ref(), &[r], "b", Some(&s), None, Some(&t_b));

        let outcome = merge_commits(store.as_ref(), &a, &b, None).unwrap();
        let kinds: Vec<_> = outcome
            .conflicts
            .iter()
            .filter_map(|c| match c {
                ObjConflict::Structural { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect();
        assert!(
            kinds.contains(&StructuralKind::TreeDivergent),
            "expected TreeDivergent stub, got: {:?}",
            outcome.conflicts
        );
    }

    #[test]
    fn merge_outcome_already_ahead() {
        // other is ancestor of head → nothing to merge.
        let (dir, store) = setup_repo();
        let c1 = make_commit(store.as_ref(), dir.path(), "first");
        let c2 = make_commit(store.as_ref(), dir.path(), "second");
        let outcome = merge_commits(store.as_ref(), &c2, &c1, None).unwrap();
        assert_eq!(outcome.trivial, TrivialOutcome::AlreadyAhead);
        assert_eq!(outcome.base, Some(c1));
        assert!(outcome.conflicts.is_empty());
    }

    #[test]
    fn obj_conflict_textual_display() {
        let c = ObjConflict::Textual {
            path: PathBuf::from("src/lib.rs"),
            base: None,
            ours: None,
            theirs: None,
        };
        let s = format!("{}", c);
        assert!(s.contains("textual"), "got: {}", s);
        assert!(s.contains("src/lib.rs"), "got: {}", s);
    }

    #[test]
    fn merge_base_walks_both_parents_of_merge() {
        // r → a → m (parents: a, b)
        //   └─> b ──┘
        // ancestors of m include r, a, b — LCA(m, b) must be b.
        let (_dir, store) = setup_repo();
        let r = raw_commit(store.as_ref(), &[], "r");
        let a = raw_commit(store.as_ref(), &[r], "a");
        let b = raw_commit(store.as_ref(), &[r], "b");
        let m = raw_commit(store.as_ref(), &[a, b], "m");
        assert_eq!(merge_base(store.as_ref(), &m, &b).unwrap(), Some(b));
    }

    // ── Stage F: tree merge integration (PR 3) ───────────────────────

    /// Build a real tree from a list of (path, content) pairs, storing
    /// each blob and the resulting Tree object.
    fn put_real_tree(store: &dyn Store, files: &[(&str, &str)]) -> Hash {
        use crate::objects::{Blob, MorphObject};
        let mut entries: BTreeMap<String, String> = BTreeMap::new();
        for (path, content) in files {
            let blob = MorphObject::Blob(Blob {
                kind: "blob".into(),
                content: serde_json::json!({ "body": content }),
            });
            let h = store.put(&blob).unwrap();
            entries.insert((*path).to_string(), h.to_string());
        }
        crate::tree::build_tree(store, &entries).unwrap()
    }

    #[test]
    fn merge_commits_resolves_tree_when_clean() {
        // base has a.txt and b.txt; ours edits a.txt; theirs edits b.txt.
        // The tree merger must resolve cleanly with no Textual conflicts,
        // populate union_tree, and plan working writes for both files.
        let (_dir, store) = setup_repo();
        let s = put_suite(store.as_ref(), vec![metric("acc", 0.8)]);
        let base_tree = put_real_tree(&store, &[("a.txt", "old\n"), ("b.txt", "old\n")]);
        let ours_tree = put_real_tree(&store, &[("a.txt", "OURS\n"), ("b.txt", "old\n")]);
        let theirs_tree = put_real_tree(&store, &[("a.txt", "old\n"), ("b.txt", "THEIRS\n")]);

        let r = raw_commit_full(store.as_ref(), &[], "r", Some(&s), None, Some(&base_tree));
        let a = raw_commit_full(store.as_ref(), &[r], "a", Some(&s), None, Some(&ours_tree));
        let b = raw_commit_full(store.as_ref(), &[r], "b", Some(&s), None, Some(&theirs_tree));

        let outcome = merge_commits(store.as_ref(), &a, &b, None).unwrap();
        assert!(
            !outcome.conflicts.iter().any(|c| matches!(c, ObjConflict::Structural { .. })),
            "no structural conflicts expected, got: {:?}",
            outcome.conflicts
        );
        assert!(
            !outcome.conflicts.iter().any(|c| matches!(c, ObjConflict::Textual { .. })),
            "no textual conflicts expected, got: {:?}",
            outcome.conflicts
        );
        let union = outcome
            .union_tree
            .expect("union_tree must be set for clean tree merge");
        let flat = crate::tree::flatten_tree(store.as_ref(), &union).unwrap();
        assert_eq!(flat.len(), 2);
        // The working tree is assumed to start at `head` (== ours), so the
        // walker plans writes only for paths whose merged content differs
        // from ours. a.txt was modified only on our side and matches ours
        // in the merged tree, so no working write is needed for it.
        // b.txt was modified by theirs, so a write IS planned.
        assert!(
            outcome.working_writes.iter().any(|op| matches!(
                op,
                crate::treemerge::WorkdirOp::Write { path, bytes }
                    if path.to_string_lossy() == "b.txt" && bytes == b"THEIRS\n"
            )),
            "expected a working write for b.txt with theirs's content, got: {:?}",
            outcome.working_writes
        );
    }

    #[test]
    fn merge_commits_emits_textual_for_overlapping_edits() {
        // Both ours and theirs edit the same line of a.txt.
        let (_dir, store) = setup_repo();
        let s = put_suite(store.as_ref(), vec![metric("acc", 0.8)]);
        let base_tree = put_real_tree(&store, &[("a.txt", "line1\nline2\nline3\n")]);
        let ours_tree = put_real_tree(&store, &[("a.txt", "line1\nOURS\nline3\n")]);
        let theirs_tree = put_real_tree(&store, &[("a.txt", "line1\nTHEIRS\nline3\n")]);

        let r = raw_commit_full(store.as_ref(), &[], "r", Some(&s), None, Some(&base_tree));
        let a = raw_commit_full(store.as_ref(), &[r], "a", Some(&s), None, Some(&ours_tree));
        let b = raw_commit_full(store.as_ref(), &[r], "b", Some(&s), None, Some(&theirs_tree));

        let outcome = merge_commits(store.as_ref(), &a, &b, None).unwrap();
        let textual_count = outcome
            .conflicts
            .iter()
            .filter(|c| matches!(c, ObjConflict::Textual { path, .. } if path.to_string_lossy() == "a.txt"))
            .count();
        assert_eq!(
            textual_count, 1,
            "expected one Textual conflict on a.txt, got: {:?}",
            outcome.conflicts
        );
        // Working writes must include a.txt with conflict markers so the
        // user can resolve.
        let bytes = outcome
            .working_writes
            .iter()
            .find_map(|op| match op {
                crate::treemerge::WorkdirOp::Write { path, bytes } if path.to_string_lossy() == "a.txt" => {
                    Some(bytes.as_slice())
                }
                _ => None,
            })
            .expect("conflict must plan a working write for a.txt");
        let s = String::from_utf8_lossy(bytes);
        assert!(s.contains("<<<<<<<"), "expected conflict markers, got:\n{}", s);
    }

    #[test]
    fn merge_commits_modify_delete_emits_tree_divergent() {
        // Ours modifies a.txt; theirs deletes it. → TreeDivergent (no
        // longer the legacy stub message).
        let (_dir, store) = setup_repo();
        let s = put_suite(store.as_ref(), vec![metric("acc", 0.8)]);
        let base_tree = put_real_tree(&store, &[("a.txt", "x\n")]);
        let ours_tree = put_real_tree(&store, &[("a.txt", "MODIFIED\n")]);
        let theirs_tree = put_real_tree(&store, &[]);

        let r = raw_commit_full(store.as_ref(), &[], "r", Some(&s), None, Some(&base_tree));
        let a = raw_commit_full(store.as_ref(), &[r], "a", Some(&s), None, Some(&ours_tree));
        let b = raw_commit_full(store.as_ref(), &[r], "b", Some(&s), None, Some(&theirs_tree));

        let outcome = merge_commits(store.as_ref(), &a, &b, None).unwrap();
        let modify_delete = outcome.conflicts.iter().any(|c| matches!(
            c,
            ObjConflict::Structural { kind: StructuralKind::TreeDivergent, message }
                if message.contains("modify/delete: a.txt")
        ));
        assert!(
            modify_delete,
            "expected TreeDivergent modify/delete conflict, got: {:?}",
            outcome.conflicts
        );
    }
}
