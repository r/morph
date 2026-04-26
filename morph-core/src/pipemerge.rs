//! 3-way structural merge of `Pipeline` objects (multi-machine plan, PR 2).
//!
//! Merge happens per-node by `id`, not by hash. Two nodes with the same id
//! across base / ours / theirs are "the same node" — their bodies are
//! reconciled with standard 3-way logic. Two nodes with different ids are
//! independent regardless of body similarity.
//!
//! Edge merge runs after node reconciliation: an edge survives only if both
//! endpoints exist in the merged graph. Same-`(from, to, kind)` edges from
//! any side de-dup. Prompts merge as a stable-order union.
//!
//! This module is consumed by [`crate::objmerge::merge_commits`]; its
//! `NodeConflict`s are surfaced through `ObjConflict::Structural` in the
//! top-level outcome so callers see one unified conflict stream.

use crate::objects::{Pipeline, PipelineEdge, PipelineGraph, PipelineNode};
use std::collections::{BTreeMap, BTreeSet};

/// Outcome of a 3-way pipeline merge.
///
/// `merged` is always populated, even when conflicts exist, so callers can
/// preview the best-effort merge alongside the conflict list. When
/// `conflicts` is empty, `merged` is the canonical result.
#[derive(Clone, Debug)]
pub struct PipelineMergeOutcome {
    pub merged: Pipeline,
    pub conflicts: Vec<NodeConflict>,
}

/// One conflicting node from the structural pipeline merge. Bodies for the
/// three sides are surfaced verbatim so callers (CLI / UI) can render a
/// resolution prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeConflict {
    pub id: String,
    pub axis: ConflictAxis,
    pub base: Option<PipelineNode>,
    pub ours: Option<PipelineNode>,
    pub theirs: Option<PipelineNode>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictAxis {
    /// Same id added on both sides with different bodies (no base entry).
    AddAdd,
    /// Modified differently on both sides (base + ours + theirs all present).
    ModifyModify,
    /// One side modified, the other deleted (base + one of ours/theirs).
    ModifyDelete,
}

impl std::fmt::Display for ConflictAxis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConflictAxis::AddAdd => write!(f, "add/add"),
            ConflictAxis::ModifyModify => write!(f, "modify/modify"),
            ConflictAxis::ModifyDelete => write!(f, "modify/delete"),
        }
    }
}

/// 3-way merge two pipelines against an optional common base.
///
/// `base = None` means no common ancestor (true criss-cross or first
/// merge between disjoint histories): in that case anything in common
/// between `ours` and `theirs` must match exactly, and side-only entries
/// are unioned.
pub fn merge_pipelines(
    base: Option<&Pipeline>,
    ours: &Pipeline,
    theirs: &Pipeline,
) -> PipelineMergeOutcome {
    let empty: Pipeline = Pipeline {
        graph: PipelineGraph { nodes: vec![], edges: vec![] },
        prompts: vec![],
        eval_suite: None,
        attribution: None,
        provenance: None,
    };
    let base_ref = base.unwrap_or(&empty);

    let (merged_nodes, conflicts) = merge_nodes(base_ref, ours, theirs);
    let merged_node_ids: BTreeSet<&str> = merged_nodes.iter().map(|n| n.id.as_str()).collect();
    let merged_edges = merge_edges(base_ref, ours, theirs, &merged_node_ids);
    let merged_prompts = merge_prompts(base_ref, ours, theirs);

    let merged = Pipeline {
        graph: PipelineGraph { nodes: merged_nodes, edges: merged_edges },
        prompts: merged_prompts,
        eval_suite: ours
            .eval_suite
            .clone()
            .or_else(|| theirs.eval_suite.clone())
            .or_else(|| base_ref.eval_suite.clone()),
        attribution: ours
            .attribution
            .clone()
            .or_else(|| theirs.attribution.clone())
            .or_else(|| base_ref.attribution.clone()),
        provenance: ours
            .provenance
            .clone()
            .or_else(|| theirs.provenance.clone())
            .or_else(|| base_ref.provenance.clone()),
    };

    PipelineMergeOutcome { merged, conflicts }
}

/// 3-way merge of node sets keyed by `id`. Preserves a deterministic order
/// (sorted by id). Returns the merged node list plus any conflicts.
fn merge_nodes(
    base: &Pipeline,
    ours: &Pipeline,
    theirs: &Pipeline,
) -> (Vec<PipelineNode>, Vec<NodeConflict>) {
    let base_map: BTreeMap<&str, &PipelineNode> =
        base.graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let ours_map: BTreeMap<&str, &PipelineNode> =
        ours.graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let theirs_map: BTreeMap<&str, &PipelineNode> =
        theirs.graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let mut all_ids: BTreeSet<&str> = BTreeSet::new();
    all_ids.extend(base_map.keys().copied());
    all_ids.extend(ours_map.keys().copied());
    all_ids.extend(theirs_map.keys().copied());

    let mut merged: Vec<PipelineNode> = Vec::new();
    let mut conflicts: Vec<NodeConflict> = Vec::new();

    for id in all_ids {
        let b = base_map.get(id).copied();
        let o = ours_map.get(id).copied();
        let t = theirs_map.get(id).copied();
        match (b, o, t) {
            (None, None, None) => unreachable!(),
            // Side-only adds.
            (None, Some(n), None) => merged.push(n.clone()),
            (None, None, Some(n)) => merged.push(n.clone()),
            // Deleted on both sides.
            (Some(_), None, None) => {}
            // Both add same id with no base — must agree.
            (None, Some(o_n), Some(t_n)) => {
                if o_n == t_n {
                    merged.push(o_n.clone());
                } else {
                    conflicts.push(NodeConflict {
                        id: id.to_string(),
                        axis: ConflictAxis::AddAdd,
                        base: None,
                        ours: Some(o_n.clone()),
                        theirs: Some(t_n.clone()),
                    });
                }
            }
            // Theirs deleted; ours either unchanged (drop) or modified (conflict).
            (Some(b_n), Some(o_n), None) => {
                if o_n == b_n {
                    // unchanged on our side; theirs's delete wins
                } else {
                    conflicts.push(NodeConflict {
                        id: id.to_string(),
                        axis: ConflictAxis::ModifyDelete,
                        base: Some(b_n.clone()),
                        ours: Some(o_n.clone()),
                        theirs: None,
                    });
                    // preview: keep the modified side
                    merged.push(o_n.clone());
                }
            }
            // Ours deleted; symmetric.
            (Some(b_n), None, Some(t_n)) => {
                if t_n == b_n {
                    // unchanged on their side; ours's delete wins
                } else {
                    conflicts.push(NodeConflict {
                        id: id.to_string(),
                        axis: ConflictAxis::ModifyDelete,
                        base: Some(b_n.clone()),
                        ours: None,
                        theirs: Some(t_n.clone()),
                    });
                    merged.push(t_n.clone());
                }
            }
            // All three present.
            (Some(b_n), Some(o_n), Some(t_n)) => {
                if o_n == t_n {
                    merged.push(o_n.clone());
                } else if o_n == b_n {
                    merged.push(t_n.clone());
                } else if t_n == b_n {
                    merged.push(o_n.clone());
                } else {
                    conflicts.push(NodeConflict {
                        id: id.to_string(),
                        axis: ConflictAxis::ModifyModify,
                        base: Some(b_n.clone()),
                        ours: Some(o_n.clone()),
                        theirs: Some(t_n.clone()),
                    });
                    // preview: keep ours
                    merged.push(o_n.clone());
                }
            }
        }
    }

    (merged, conflicts)
}

/// 3-way merge of edges keyed by `(from, to, kind)`. Edges with endpoints
/// not present in the merged node set are dropped silently — they are
/// derived consequences of node deletes, not conflicts.
fn merge_edges(
    base: &Pipeline,
    ours: &Pipeline,
    theirs: &Pipeline,
    merged_node_ids: &BTreeSet<&str>,
) -> Vec<PipelineEdge> {
    type Key = (String, String, String);
    let to_key = |e: &PipelineEdge| -> Key { (e.from.clone(), e.to.clone(), e.kind.clone()) };
    let in_base: BTreeSet<Key> = base.graph.edges.iter().map(to_key).collect();
    let in_ours: BTreeSet<Key> = ours.graph.edges.iter().map(to_key).collect();
    let in_theirs: BTreeSet<Key> = theirs.graph.edges.iter().map(to_key).collect();

    let mut all_keys: BTreeSet<Key> = BTreeSet::new();
    all_keys.extend(in_base.iter().cloned());
    all_keys.extend(in_ours.iter().cloned());
    all_keys.extend(in_theirs.iter().cloned());

    let mut kept: Vec<PipelineEdge> = Vec::new();
    for k in all_keys {
        let b = in_base.contains(&k);
        let o = in_ours.contains(&k);
        let t = in_theirs.contains(&k);
        let keep = match (b, o, t) {
            (false, false, false) => unreachable!(),
            (false, true, true) | (false, true, false) | (false, false, true) => true,
            (true, true, true) => true,
            (true, true, false) | (true, false, true) => false, // one side deleted
            (true, false, false) => false,                       // both deleted
        };
        if !keep {
            continue;
        }
        // Drop edges whose endpoints didn't survive node merge.
        if merged_node_ids.contains(k.0.as_str()) && merged_node_ids.contains(k.1.as_str()) {
            kept.push(PipelineEdge { from: k.0, to: k.1, kind: k.2 });
        }
    }
    kept
}

/// Stable-order union of prompts: base order first, then ours-only adds,
/// then theirs-only adds. Deduplicated.
fn merge_prompts(base: &Pipeline, ours: &Pipeline, theirs: &Pipeline) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    for p in &base.prompts {
        if seen.insert(p.clone()) {
            out.push(p.clone());
        }
    }
    for p in &ours.prompts {
        if seen.insert(p.clone()) {
            out.push(p.clone());
        }
    }
    for p in &theirs.prompts {
        if seen.insert(p.clone()) {
            out.push(p.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // ── helpers ───────────────────────────────────────────────────────

    fn node(id: &str, kind: &str) -> PipelineNode {
        PipelineNode {
            id: id.to_string(),
            kind: kind.to_string(),
            ref_: None,
            params: BTreeMap::new(),
            env: None,
        }
    }

    fn node_with_param(id: &str, kind: &str, key: &str, val: &str) -> PipelineNode {
        let mut params = BTreeMap::new();
        params.insert(key.to_string(), serde_json::json!(val));
        PipelineNode {
            id: id.to_string(),
            kind: kind.to_string(),
            ref_: None,
            params,
            env: None,
        }
    }

    fn edge(from: &str, to: &str, kind: &str) -> PipelineEdge {
        PipelineEdge {
            from: from.to_string(),
            to: to.to_string(),
            kind: kind.to_string(),
        }
    }

    fn pipe(nodes: Vec<PipelineNode>, edges: Vec<PipelineEdge>, prompts: Vec<&str>) -> Pipeline {
        Pipeline {
            graph: PipelineGraph { nodes, edges },
            prompts: prompts.into_iter().map(|s| s.to_string()).collect(),
            eval_suite: None,
            attribution: None,
            provenance: None,
        }
    }

    // ── cycles ────────────────────────────────────────────────────────

    #[test]
    fn merge_pipelines_identical_returns_same() {
        let p = pipe(vec![node("a", "prompt_call")], vec![], vec!["p1"]);
        let outcome = merge_pipelines(Some(&p), &p, &p);
        assert!(outcome.conflicts.is_empty(), "got: {:?}", outcome.conflicts);
        assert_eq!(outcome.merged, p);
    }

    #[test]
    fn merge_pipelines_only_ours_changed_takes_ours() {
        let base = pipe(vec![node("a", "prompt_call")], vec![], vec![]);
        let ours = pipe(
            vec![node("a", "prompt_call"), node("b", "tool_call")],
            vec![],
            vec![],
        );
        let theirs = base.clone();
        let outcome = merge_pipelines(Some(&base), &ours, &theirs);
        assert!(outcome.conflicts.is_empty(), "got: {:?}", outcome.conflicts);
        assert_eq!(outcome.merged, ours);
    }

    #[test]
    fn merge_pipelines_only_theirs_changed_takes_theirs() {
        let base = pipe(vec![node("a", "prompt_call")], vec![], vec![]);
        let ours = base.clone();
        let theirs = pipe(
            vec![node("a", "prompt_call"), node("b", "tool_call")],
            vec![],
            vec![],
        );
        let outcome = merge_pipelines(Some(&base), &ours, &theirs);
        assert!(outcome.conflicts.is_empty(), "got: {:?}", outcome.conflicts);
        assert_eq!(outcome.merged, theirs);
    }

    // ── node add ──────────────────────────────────────────────────────

    #[test]
    fn merge_pipelines_disjoint_node_adds_unioned() {
        let base = pipe(vec![node("a", "prompt_call")], vec![], vec![]);
        let ours = pipe(vec![node("a", "prompt_call"), node("b", "tool_call")], vec![], vec![]);
        let theirs = pipe(vec![node("a", "prompt_call"), node("c", "transform")], vec![], vec![]);
        let outcome = merge_pipelines(Some(&base), &ours, &theirs);
        assert!(outcome.conflicts.is_empty(), "got: {:?}", outcome.conflicts);
        let ids: Vec<_> = outcome.merged.graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"a"));
        assert!(ids.contains(&"b"));
        assert!(ids.contains(&"c"));
    }

    #[test]
    fn merge_pipelines_same_id_same_body_added_both_no_conflict() {
        let base = pipe(vec![node("a", "prompt_call")], vec![], vec![]);
        let added = node_with_param("x", "tool_call", "k", "v");
        let ours = pipe(vec![node("a", "prompt_call"), added.clone()], vec![], vec![]);
        let theirs = pipe(vec![node("a", "prompt_call"), added.clone()], vec![], vec![]);
        let outcome = merge_pipelines(Some(&base), &ours, &theirs);
        assert!(outcome.conflicts.is_empty(), "got: {:?}", outcome.conflicts);
        assert!(outcome.merged.graph.nodes.iter().any(|n| n.id == "x"));
    }

    #[test]
    fn merge_pipelines_same_id_diff_body_added_both_conflicts() {
        let base = pipe(vec![node("a", "prompt_call")], vec![], vec![]);
        let ours = pipe(
            vec![node("a", "prompt_call"), node_with_param("x", "tool_call", "k", "v1")],
            vec![],
            vec![],
        );
        let theirs = pipe(
            vec![node("a", "prompt_call"), node_with_param("x", "tool_call", "k", "v2")],
            vec![],
            vec![],
        );
        let outcome = merge_pipelines(Some(&base), &ours, &theirs);
        assert_eq!(outcome.conflicts.len(), 1);
        assert_eq!(outcome.conflicts[0].id, "x");
        assert_eq!(outcome.conflicts[0].axis, ConflictAxis::AddAdd);
        // Conflicting node dropped from merged so callers see no spurious node.
        assert!(!outcome.merged.graph.nodes.iter().any(|n| n.id == "x"));
    }

    // ── node modify ───────────────────────────────────────────────────

    #[test]
    fn merge_pipelines_node_modified_one_side() {
        let base = pipe(vec![node_with_param("a", "prompt_call", "k", "old")], vec![], vec![]);
        let ours = pipe(vec![node_with_param("a", "prompt_call", "k", "new")], vec![], vec![]);
        let theirs = base.clone();
        let outcome = merge_pipelines(Some(&base), &ours, &theirs);
        assert!(outcome.conflicts.is_empty(), "got: {:?}", outcome.conflicts);
        let a = outcome.merged.graph.nodes.iter().find(|n| n.id == "a").unwrap();
        assert_eq!(a.params.get("k").unwrap(), &serde_json::json!("new"));
    }

    #[test]
    fn merge_pipelines_node_modified_both_sides_same_way() {
        let base = pipe(vec![node_with_param("a", "prompt_call", "k", "old")], vec![], vec![]);
        let modified = pipe(vec![node_with_param("a", "prompt_call", "k", "new")], vec![], vec![]);
        let outcome = merge_pipelines(Some(&base), &modified, &modified);
        assert!(outcome.conflicts.is_empty(), "got: {:?}", outcome.conflicts);
        let a = outcome.merged.graph.nodes.iter().find(|n| n.id == "a").unwrap();
        assert_eq!(a.params.get("k").unwrap(), &serde_json::json!("new"));
    }

    #[test]
    fn merge_pipelines_node_modified_both_sides_differently_conflicts() {
        let base = pipe(vec![node_with_param("a", "prompt_call", "k", "old")], vec![], vec![]);
        let ours = pipe(vec![node_with_param("a", "prompt_call", "k", "v1")], vec![], vec![]);
        let theirs = pipe(vec![node_with_param("a", "prompt_call", "k", "v2")], vec![], vec![]);
        let outcome = merge_pipelines(Some(&base), &ours, &theirs);
        assert_eq!(outcome.conflicts.len(), 1);
        assert_eq!(outcome.conflicts[0].id, "a");
        assert_eq!(outcome.conflicts[0].axis, ConflictAxis::ModifyModify);
    }

    // ── node delete ───────────────────────────────────────────────────

    #[test]
    fn merge_pipelines_node_deleted_one_side_unchanged_other() {
        let base = pipe(vec![node("a", "prompt_call"), node("b", "tool_call")], vec![], vec![]);
        let ours = pipe(vec![node("a", "prompt_call")], vec![], vec![]);
        let theirs = base.clone();
        let outcome = merge_pipelines(Some(&base), &ours, &theirs);
        assert!(outcome.conflicts.is_empty(), "got: {:?}", outcome.conflicts);
        assert!(!outcome.merged.graph.nodes.iter().any(|n| n.id == "b"));
    }

    #[test]
    fn merge_pipelines_node_deleted_both_sides() {
        let base = pipe(vec![node("a", "prompt_call"), node("b", "tool_call")], vec![], vec![]);
        let ours = pipe(vec![node("a", "prompt_call")], vec![], vec![]);
        let theirs = pipe(vec![node("a", "prompt_call")], vec![], vec![]);
        let outcome = merge_pipelines(Some(&base), &ours, &theirs);
        assert!(outcome.conflicts.is_empty(), "got: {:?}", outcome.conflicts);
        assert!(!outcome.merged.graph.nodes.iter().any(|n| n.id == "b"));
    }

    #[test]
    fn merge_pipelines_modify_delete_conflicts() {
        let base = pipe(vec![node_with_param("a", "prompt_call", "k", "old")], vec![], vec![]);
        let ours = pipe(vec![node_with_param("a", "prompt_call", "k", "new")], vec![], vec![]);
        let theirs = pipe(vec![], vec![], vec![]);
        let outcome = merge_pipelines(Some(&base), &ours, &theirs);
        assert_eq!(outcome.conflicts.len(), 1);
        assert_eq!(outcome.conflicts[0].id, "a");
        assert_eq!(outcome.conflicts[0].axis, ConflictAxis::ModifyDelete);
        // Modified side is kept in `merged` for preview.
        assert!(outcome.merged.graph.nodes.iter().any(|n| n.id == "a"));
    }

    // ── edges ─────────────────────────────────────────────────────────

    #[test]
    fn merge_pipelines_edges_unioned_disjoint() {
        let base = pipe(
            vec![node("a", "prompt_call"), node("b", "tool_call"), node("c", "transform")],
            vec![],
            vec![],
        );
        let ours = pipe(
            base.graph.nodes.clone(),
            vec![edge("a", "b", "data")],
            vec![],
        );
        let theirs = pipe(
            base.graph.nodes.clone(),
            vec![edge("a", "c", "data")],
            vec![],
        );
        let outcome = merge_pipelines(Some(&base), &ours, &theirs);
        assert!(outcome.conflicts.is_empty(), "got: {:?}", outcome.conflicts);
        let edges = &outcome.merged.graph.edges;
        assert!(edges.iter().any(|e| e.from == "a" && e.to == "b"));
        assert!(edges.iter().any(|e| e.from == "a" && e.to == "c"));
    }

    #[test]
    fn merge_pipelines_edges_orphaned_dropped() {
        let base = pipe(
            vec![node("a", "prompt_call"), node("b", "tool_call")],
            vec![],
            vec![],
        );
        let ours = pipe(vec![node("a", "prompt_call")], vec![], vec![]); // deletes b
        let theirs = pipe(
            vec![node("a", "prompt_call"), node("b", "tool_call")],
            vec![edge("a", "b", "data")],
            vec![],
        );
        let outcome = merge_pipelines(Some(&base), &ours, &theirs);
        assert!(
            outcome.conflicts.is_empty(),
            "orphaned edge is a derived consequence, not a conflict; got: {:?}",
            outcome.conflicts
        );
        assert!(outcome.merged.graph.edges.is_empty(),
            "edge a→b must be dropped because b was deleted; got: {:?}",
            outcome.merged.graph.edges);
    }

    // ── prompts ───────────────────────────────────────────────────────

    #[test]
    fn merge_pipelines_prompts_unioned() {
        let base = pipe(vec![], vec![], vec!["p1"]);
        let ours = pipe(vec![], vec![], vec!["p1", "p2"]);
        let theirs = pipe(vec![], vec![], vec!["p1", "p3"]);
        let outcome = merge_pipelines(Some(&base), &ours, &theirs);
        assert!(outcome.conflicts.is_empty(), "got: {:?}", outcome.conflicts);
        assert_eq!(outcome.merged.prompts, vec!["p1", "p2", "p3"]);
    }
}
