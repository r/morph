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

    // Suite / pipeline / tree stages — only run when the merge is non-trivial.
    if matches!(trivial, TrivialOutcome::Diverged) {
        match reconcile_suites(store, head, other, _retire) {
            Ok(s) => union_suite = s,
            Err(c) => conflicts.push(c),
        }

        // Pipeline stub (PR 2 replaces this with real per-node merge).
        let head_commit = load_commit(store, head)?;
        let other_commit = load_commit(store, other)?;
        if head_commit.pipeline != other_commit.pipeline {
            conflicts.push(ObjConflict::Structural {
                kind: StructuralKind::PipelineDivergent,
                message: "pipeline merge not yet implemented (PR 2)".to_string(),
            });
        }

        // Tree stub (PR 3 replaces this with real 3-way tree merge).
        if head_commit.tree != other_commit.tree {
            conflicts.push(ObjConflict::Structural {
                kind: StructuralKind::TreeDivergent,
                message: "tree merge not yet implemented (PR 3)".to_string(),
            });
        }
    }

    Ok(MergeOutcome {
        head: *head,
        other: *other,
        base,
        union_suite,
        conflicts,
        trivial,
    })
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
}
