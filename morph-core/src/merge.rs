//! Merge planning, dominance explanation, and merge execution (Phase 4).
//!
//! Provides a first-class merge workflow:
//! - `MergePlan`: pre-merge inspection state (both parents, union suite, reference bar)
//! - `prepare_merge`: computes the merge plan from both parents
//! - `execute_merge`: creates the merge commit using the plan with detailed failure reporting
//! - `DominanceResult` / `DominanceViolation`: per-metric dominance failure explanations

use crate::commit::{current_branch, resolve_head};
use crate::objects::{
    ActorRef, AttributionEntry, Commit, EvalContract, EvalSuite, MorphObject, PipelineNode,
};
use crate::policy::read_policy;
use crate::store::{MorphError, ObjectType, Store};
use crate::Hash;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::path::Path;

/// Default reason text used when the user did not supply one via
/// `--retire-reason`. Kept short so the synthesized review node still
/// reads cleanly in `morph pipeline show`.
const DEFAULT_RETIRE_REASON: &str = "metric retirement requested at merge time";

/// Ensure the pipeline at `pipeline_hash` carries a `review` node when
/// `retired_metrics` is non-empty (paper §4.3 attribution requirement).
///
/// The user-supplied pipeline is used unchanged when:
/// - no metrics are being retired, or
/// - the pipeline already contains at least one `review` node, or
/// - the hash does not point to a [`MorphObject::Pipeline`] (legacy or
///   mismatched callers — we don't synthesize blindly).
///
/// Otherwise we append a deterministic review node (`id` derived from
/// the retirement context so identical retirements collapse to the same
/// id but different retirements don't collide), record the merge author
/// and `morph_instance` in `attribution`, store the new pipeline, and
/// return its hash. This keeps backward compatibility with pipelines
/// whose authors already hand-wrote a review node and surfaces a
/// machine-readable retirement record for everyone else.
pub(crate) fn ensure_review_node_for_retirement(
    store: &dyn Store,
    pipeline_hash: &Hash,
    retired_metrics: &[String],
    retire_reason: Option<&str>,
    author: &str,
    morph_instance: Option<&str>,
) -> Result<Hash, MorphError> {
    if retired_metrics.is_empty() {
        return Ok(pipeline_hash.clone());
    }
    let mut pipeline = match store.get(pipeline_hash) {
        Ok(MorphObject::Pipeline(p)) => p,
        // Either a non-pipeline object (legacy callers occasionally
        // pass a blob hash) or a missing object — either way, leave
        // the merge alone rather than failing on an opaque hash.
        _ => return Ok(pipeline_hash.clone()),
    };
    if pipeline.graph.nodes.iter().any(|n| n.kind == "review") {
        return Ok(pipeline_hash.clone());
    }

    let mut sorted: Vec<&str> = retired_metrics.iter().map(|s| s.as_str()).collect();
    sorted.sort();
    let reason = retire_reason.unwrap_or(DEFAULT_RETIRE_REASON);

    let mut hasher = Sha256::new();
    hasher.update(b"review-retirement\0");
    hasher.update(sorted.join(",").as_bytes());
    hasher.update(b"\0");
    hasher.update(reason.as_bytes());
    hasher.update(b"\0");
    hasher.update(author.as_bytes());
    let digest = hasher.finalize();
    let id_suffix = digest
        .iter()
        .take(4)
        .map(|b| format!("{:02x}", b))
        .collect::<String>();
    let node_id = format!("review-retirement-{}", id_suffix);

    let mut params: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    params.insert(
        "retired_metrics".into(),
        serde_json::Value::Array(
            sorted
                .iter()
                .map(|s| serde_json::Value::String((*s).to_string()))
                .collect(),
        ),
    );
    params.insert(
        "reason".into(),
        serde_json::Value::String(reason.to_string()),
    );

    pipeline.graph.nodes.push(PipelineNode {
        id: node_id.clone(),
        kind: "review".into(),
        ref_: None,
        params,
        env: None,
    });

    let entry = AttributionEntry {
        agent_id: author.to_string(),
        agent_version: None,
        instance_id: morph_instance.map(String::from),
        actors: Some(vec![ActorRef {
            id: author.to_string(),
            actor_type: "agent".to_string(),
            env_config: None,
        }]),
    };
    let mut attribution = pipeline.attribution.unwrap_or_default();
    attribution.insert(node_id, entry);
    pipeline.attribution = Some(attribution);

    store.put(&MorphObject::Pipeline(pipeline))
}

/// Whether `RepoPolicy.merge_policy` requires dominance for the
/// repo at `repo_root`. Returns `true` when no policy is reachable
/// (no repo root or read failure) so we never weaken the gate by
/// accident — a missing config is treated as "dominance".
fn dominance_required(repo_root: Option<&Path>) -> bool {
    let Some(root) = repo_root else { return true };
    match read_policy(&root.join(".morph")) {
        Ok(p) => p.merge_policy != "none",
        Err(_) => true,
    }
}

/// A single metric that failed dominance during merge.
#[derive(Clone, Debug)]
pub struct DominanceViolation {
    pub metric: String,
    pub direction: String,
    /// The merged candidate's value, or None if the metric was missing entirely.
    pub merged_value: Option<f64>,
    pub parent_value: f64,
    /// Which parent was violated: "current" or "other".
    pub parent_label: String,
}

impl fmt::Display for DominanceViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.merged_value {
            Some(merged) => {
                let op = if self.direction == "minimize" { "<=" } else { ">=" };
                write!(
                    f,
                    "metric '{}': merged {} does not dominate {} branch's {} (direction: {}, need {} {})",
                    self.metric, merged, self.parent_label, self.parent_value,
                    self.direction, op, self.parent_value,
                )
            }
            None => {
                write!(
                    f,
                    "metric '{}': missing from merged metrics, {} branch has {}",
                    self.metric, self.parent_label, self.parent_value,
                )
            }
        }
    }
}

/// Result of a dominance check with detailed explanation.
#[derive(Clone, Debug)]
pub struct DominanceResult {
    pub passed: bool,
    pub violations: Vec<DominanceViolation>,
}

/// Pre-merge inspection state: both parents, union suite, reference bar.
#[derive(Clone, Debug)]
pub struct MergePlan {
    pub head_hash: Hash,
    pub other_hash: Hash,
    pub head_branch: Option<String>,
    pub other_branch: String,
    pub head_metrics: BTreeMap<String, f64>,
    pub other_metrics: BTreeMap<String, f64>,
    pub head_suite_hash: String,
    pub other_suite_hash: String,
    pub union_suite: EvalSuite,
    /// The strictest bar from either parent that the merged candidate must meet.
    pub reference_bar: BTreeMap<String, f64>,
    pub retired_metrics: Vec<String>,
    /// Optional human-readable reason for retiring metrics. Recorded in
    /// the auto-injected `review` node's `params.reason` (paper §4.3).
    /// `None` falls back to a generic placeholder message at injection
    /// time. Set via `morph merge --retire-reason "..."`.
    pub retire_reason: Option<String>,
    /// PR 6 stage C: deduped union of both parents' `evidence_refs`.
    /// `None` when neither parent had any (don't churn hashes by
    /// emitting an empty array on legacy / suite-free histories).
    pub evidence_refs: Option<Vec<String>>,
    /// Phase 6b: acceptance case ids contributed by each branch's
    /// history since the merge base (or since the root, when there
    /// is no shared history). Recorded via `morph commit
    /// --new-cases`. Sorted+deduped per branch.
    pub head_introduces_cases: Vec<String>,
    pub other_introduces_cases: Vec<String>,
    head_commit: Commit,
    other_commit: Commit,
}

impl MergePlan {
    /// Check whether proposed merged metrics would pass dominance against both parents.
    /// Only metrics present in the (post-retirement) union suite are checked.
    pub fn check_dominance(&self, merged: &BTreeMap<String, f64>) -> DominanceResult {
        let mut violations = Vec::new();
        check_parent_dominance(merged, &self.head_metrics, &self.union_suite, "current", &mut violations);
        check_parent_dominance(merged, &self.other_metrics, &self.union_suite, "other", &mut violations);
        DominanceResult {
            passed: violations.is_empty(),
            violations,
        }
    }

    /// Format the merge plan as human-readable text for CLI output.
    pub fn format_plan(&self) -> String {
        let mut out = String::new();
        let head_label = self.head_branch.as_deref().unwrap_or("HEAD (detached)");
        out.push_str(&format!("Merge plan: {} -> {}\n\n", self.other_branch, head_label));

        out.push_str(&format!("Current branch ({}):\n", head_label));
        out.push_str(&format!("  commit: {}\n", self.head_hash));
        out.push_str(&format!("  suite: {}\n", self.head_suite_hash));
        out.push_str(&format!("  metrics: {}\n\n", format_metrics_inline(&self.head_metrics)));

        out.push_str(&format!("Other branch ({}):\n", self.other_branch));
        out.push_str(&format!("  commit: {}\n", self.other_hash));
        out.push_str(&format!("  suite: {}\n", self.other_suite_hash));
        out.push_str(&format!("  metrics: {}\n\n", format_metrics_inline(&self.other_metrics)));

        out.push_str(&format!("Union eval suite ({} metrics):\n", self.union_suite.metrics.len()));
        if self.union_suite.metrics.is_empty() {
            out.push_str("  (none)\n");
        } else {
            for m in &self.union_suite.metrics {
                out.push_str(&format!("  {} {} threshold={}\n", m.name, m.direction, m.threshold));
            }
        }
        out.push('\n');

        out.push_str("Reference bar:\n");
        if self.reference_bar.is_empty() {
            out.push_str("  (none)\n");
        } else {
            for (name, val) in &self.reference_bar {
                let dir = self.union_suite.metrics.iter()
                    .find(|m| m.name == *name)
                    .map(|m| m.direction.as_str())
                    .unwrap_or("maximize");
                let op = if dir == "minimize" { "<=" } else { ">=" };
                out.push_str(&format!("  {} {} {} ({})\n", name, op, val, dir));
            }
        }
        out.push('\n');

        out.push_str("Retired metrics: ");
        if self.retired_metrics.is_empty() {
            out.push_str("none\n");
        } else {
            out.push_str(&self.retired_metrics.join(", "));
            out.push('\n');
        }

        // Phase 6b: surface case provenance so a reviewer can see at
        // a glance which acceptance cases each branch introduced and
        // therefore which cases the merged candidate must satisfy.
        let union: BTreeSet<&String> = self
            .head_introduces_cases
            .iter()
            .chain(self.other_introduces_cases.iter())
            .collect();
        out.push_str("\nCase provenance:\n");
        out.push_str(&format!(
            "  {} introduces {} case(s){}\n",
            head_label,
            self.head_introduces_cases.len(),
            format_case_list(&self.head_introduces_cases),
        ));
        out.push_str(&format!(
            "  {} introduces {} case(s){}\n",
            self.other_branch,
            self.other_introduces_cases.len(),
            format_case_list(&self.other_introduces_cases),
        ));
        out.push_str(&format!(
            "  Merged candidate must pass all {} (union) plus existing suite.\n",
            union.len(),
        ));

        out
    }
}

fn format_case_list(cases: &[String]) -> String {
    if cases.is_empty() {
        String::new()
    } else {
        format!(": {}", cases.join(", "))
    }
}

fn format_metrics_inline(m: &BTreeMap<String, f64>) -> String {
    if m.is_empty() {
        return "(none)".to_string();
    }
    m.iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Check dominance of `merged` against a single parent's metrics.
/// Only checks metrics that exist in the (post-retirement) union suite.
fn check_parent_dominance(
    merged: &BTreeMap<String, f64>,
    parent: &BTreeMap<String, f64>,
    suite: &EvalSuite,
    parent_label: &str,
    violations: &mut Vec<DominanceViolation>,
) {
    let suite_metric_names: std::collections::BTreeSet<&str> =
        suite.metrics.iter().map(|m| m.name.as_str()).collect();
    let directions: BTreeMap<&str, &str> = suite
        .metrics
        .iter()
        .map(|m| (m.name.as_str(), m.direction.as_str()))
        .collect();

    for (k, &parent_val) in parent {
        if !suite_metric_names.contains(k.as_str()) {
            continue;
        }
        let dir = directions.get(k.as_str()).copied().unwrap_or("maximize");
        match merged.get(k) {
            Some(&merged_val) => {
                let ok = if dir == "minimize" {
                    merged_val <= parent_val
                } else {
                    merged_val >= parent_val
                };
                if !ok {
                    violations.push(DominanceViolation {
                        metric: k.clone(),
                        direction: dir.to_string(),
                        merged_value: Some(merged_val),
                        parent_value: parent_val,
                        parent_label: parent_label.to_string(),
                    });
                }
            }
            None => {
                violations.push(DominanceViolation {
                    metric: k.clone(),
                    direction: dir.to_string(),
                    merged_value: None,
                    parent_value: parent_val,
                    parent_label: parent_label.to_string(),
                });
            }
        }
    }
}

/// Compute the reference bar: for each metric in the union suite, the strictest value
/// from either parent that the merged candidate must meet.
/// For "maximize": max of both parents. For "minimize": min of both parents.
fn compute_reference_bar(
    head_metrics: &BTreeMap<String, f64>,
    other_metrics: &BTreeMap<String, f64>,
    suite: &EvalSuite,
) -> BTreeMap<String, f64> {
    let mut bar = BTreeMap::new();
    for m in &suite.metrics {
        let h = head_metrics.get(&m.name);
        let o = other_metrics.get(&m.name);
        let best = match (h, o) {
            (Some(&hv), Some(&ov)) => {
                if m.direction == "minimize" { hv.min(ov) } else { hv.max(ov) }
            }
            (Some(&hv), None) => hv,
            (None, Some(&ov)) => ov,
            (None, None) => continue,
        };
        bar.insert(m.name.clone(), best);
    }
    bar
}


/// Prepare a merge plan: resolve both parents, compute union suite, reference bar.
///
/// If `eval_suite_hash` is Some, uses that suite directly (no auto-union).
/// If None, computes the union of both parents' suites.
/// If `retired_metrics` is provided, retires those metrics from the suite.
pub fn prepare_merge(
    store: &dyn Store,
    other_branch: &str,
    eval_suite_hash: Option<&Hash>,
    retired_metrics: Option<&[String]>,
) -> Result<MergePlan, MorphError> {
    let head_hash = resolve_head(store)?
        .ok_or_else(|| MorphError::Serialization("no HEAD commit".into()))?;

    let other_ref = if other_branch.starts_with("heads/") {
        other_branch.to_string()
    } else {
        format!("heads/{}", other_branch)
    };
    let other_hash = store
        .ref_read(&other_ref)?
        .ok_or_else(|| MorphError::NotFound(other_branch.into()))?;

    let head_commit = match store.get(&head_hash)? {
        MorphObject::Commit(c) => c,
        _ => return Err(MorphError::Serialization("HEAD is not a commit".into())),
    };
    let other_commit = match store.get(&other_hash)? {
        MorphObject::Commit(c) => c,
        _ => return Err(MorphError::Serialization("other ref is not a commit".into())),
    };

    let union = match eval_suite_hash {
        Some(h) => crate::commit::load_eval_suite(store, &h.to_string())?,
        None => {
            let head_suite = crate::commit::load_eval_suite(store, &head_commit.eval_contract.suite)?;
            let other_suite = crate::commit::load_eval_suite(store, &other_commit.eval_contract.suite)?;
            crate::metrics::union_suites(&head_suite, &other_suite)?
        }
    };

    let retired = retired_metrics.unwrap_or(&[]);
    let union = if retired.is_empty() {
        union
    } else {
        crate::metrics::retire_metrics(&union, retired)?
    };

    let reference_bar = compute_reference_bar(
        &head_commit.eval_contract.observed_metrics,
        &other_commit.eval_contract.observed_metrics,
        &union,
    );

    let head_branch = current_branch(store)?;

    // PR 6 stage C cycles 10/12: union of both parents' evidence_refs.
    // Stable order (sorted) so the resulting commit hash doesn't
    // depend on which parent we read first; deduped so a Run that
    // both parents reference shows up exactly once. Stays `None` if
    // neither parent has any evidence — emitting `Some(vec![])`
    // would change canonical hashes for histories that never had
    // evidence in the first place.
    let evidence_refs = union_evidence_refs(
        head_commit.evidence_refs.as_deref(),
        other_commit.evidence_refs.as_deref(),
    );

    // Phase 6b: walk back from each tip, stopping at the merge base
    // when there is one, to collect `introduces_cases` annotations.
    // We deliberately tolerate missing-base (disjoint histories) by
    // walking back to root.
    let base = crate::objmerge::merge_base(store, &head_hash, &other_hash)?;
    let head_introduces_cases =
        collect_introduces_cases(store, &head_hash, base.as_ref())?;
    let other_introduces_cases =
        collect_introduces_cases(store, &other_hash, base.as_ref())?;

    Ok(MergePlan {
        head_hash,
        other_hash,
        head_branch,
        other_branch: other_branch.to_string(),
        head_metrics: head_commit.eval_contract.observed_metrics.clone(),
        other_metrics: other_commit.eval_contract.observed_metrics.clone(),
        head_suite_hash: head_commit.eval_contract.suite.clone(),
        other_suite_hash: other_commit.eval_contract.suite.clone(),
        union_suite: union,
        reference_bar,
        retired_metrics: retired.to_vec(),
        retire_reason: None,
        evidence_refs,
        head_introduces_cases,
        other_introduces_cases,
        head_commit,
        other_commit,
    })
}

/// Phase 6b: collect acceptance case ids from `introduces_cases`
/// annotations recorded along a branch's ancestry. Walks
/// `commit.parents` from `tip` and stops at any commit hash in
/// `stop_at` (typically the merge base). The returned list is
/// sorted+deduped.
pub fn collect_introduces_cases(
    store: &dyn Store,
    tip: &Hash,
    stop_at: Option<&Hash>,
) -> Result<Vec<String>, MorphError> {
    // BFS the ancestry from `tip`, excluding `stop_at` (so its cases
    // remain attributed to shared history). We track hashes as hex
    // strings to avoid requiring `Ord` on the binary `Hash` newtype.
    let stop_str = stop_at.map(|h| h.to_string());
    let mut branch_commits: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<Hash> = VecDeque::new();
    queue.push_back(tip.clone());

    while let Some(h) = queue.pop_front() {
        let h_str = h.to_string();
        if stop_str.as_deref() == Some(h_str.as_str()) || !branch_commits.insert(h_str) {
            continue;
        }
        if let MorphObject::Commit(c) = store.get(&h)? {
            for p in &c.parents {
                if let Ok(ph) = Hash::from_hex(p) {
                    queue.push_back(ph);
                }
            }
        }
    }

    if branch_commits.is_empty() {
        return Ok(Vec::new());
    }

    let mut cases: BTreeSet<String> = BTreeSet::new();
    for ah in store.list(ObjectType::Annotation)? {
        let MorphObject::Annotation(a) = store.get(&ah)? else { continue };
        if a.kind != "introduces_cases" || !branch_commits.contains(&a.target) {
            continue;
        }
        if let Some(arr) = a.data.get("cases").and_then(|v| v.as_array()) {
            for v in arr {
                if let Some(s) = v.as_str() {
                    if !s.is_empty() {
                        cases.insert(s.to_string());
                    }
                }
            }
        }
    }
    Ok(cases.into_iter().collect())
}

/// Pure helper: deduped sorted union of two optional evidence_ref
/// lists. Returns `None` when both inputs are absent or empty so
/// merges of legacy histories don't grow new fields.
pub(crate) fn union_evidence_refs(
    a: Option<&[String]>,
    b: Option<&[String]>,
) -> Option<Vec<String>> {
    use std::collections::BTreeSet;
    let mut set: BTreeSet<String> = BTreeSet::new();
    if let Some(refs) = a {
        set.extend(refs.iter().cloned());
    }
    if let Some(refs) = b {
        set.extend(refs.iter().cloned());
    }
    if set.is_empty() {
        None
    } else {
        Some(set.into_iter().collect())
    }
}

/// Execute a merge: check dominance and create the merge commit.
/// Returns detailed error messages when dominance fails.
///
/// Honors `RepoPolicy.merge_policy` when `repo_root` is supplied:
/// `"dominance"` (default) enforces metric dominance, `"none"`
/// short-circuits the gate so structural-only merges land. Without
/// a `repo_root` we cannot read policy and behave conservatively
/// (always check dominance), matching the historical behavior of
/// every callsite that drives merges from outside a repo.
#[allow(clippy::too_many_arguments)] // mirrors `create_tree_commit`'s shape
pub fn execute_merge(
    store: &dyn Store,
    plan: &MergePlan,
    merged_pipeline_hash: &Hash,
    merged_observed_metrics: BTreeMap<String, f64>,
    message: String,
    author: Option<String>,
    repo_root: Option<&Path>,
    morph_version: Option<&str>,
) -> Result<Hash, MorphError> {
    if dominance_required(repo_root) {
        let dominance = plan.check_dominance(&merged_observed_metrics);
        if !dominance.passed {
            let mut msg = String::from("merge rejected: merged metrics do not dominate both parents\n");
            for v in &dominance.violations {
                msg.push_str(&format!("  {}\n", v));
            }
            return Err(MorphError::Serialization(msg));
        }
    }

    let suite_obj = MorphObject::EvalSuite(plan.union_suite.clone());
    let suite_hash_str = store.put(&suite_obj)?.to_string();

    let tree_hash = if let Some(root) = repo_root {
        let morph_dir = root.join(".morph");
        let index = crate::index::read_index(&morph_dir)?;
        let h = crate::tree::build_tree(store, &index.entries)?;
        crate::index::clear_index(&morph_dir)?;
        Some(h.to_string())
    } else {
        None
    };

    let merged_contributors = crate::commit::merge_contributors(&plan.head_commit, &plan.other_commit);

    let parents = vec![plan.head_hash.to_string(), plan.other_hash.to_string()];
    let timestamp = chrono::Utc::now().to_rfc3339();
    let author = author.unwrap_or_else(|| "morph".to_string());
    let morph_instance = repo_root
        .and_then(|r| crate::agent::read_instance_id(&r.join(".morph")).ok().flatten());
    // Paper §4.3: enforce review-node attribution for any retirement.
    // Auto-injection happens after the dominance check (so we don't
    // mutate the pipeline on a doomed merge) and before commit
    // construction (so the commit's pipeline hash points at the
    // version that includes the review node).
    let merged_pipeline_hash = ensure_review_node_for_retirement(
        store,
        merged_pipeline_hash,
        &plan.retired_metrics,
        plan.retire_reason.as_deref(),
        &author,
        morph_instance.as_deref(),
    )?;
    let commit = MorphObject::Commit(Commit {
        tree: tree_hash,
        pipeline: merged_pipeline_hash.to_string(),
        parents,
        message,
        timestamp,
        author,
        contributors: merged_contributors,
        eval_contract: EvalContract {
            suite: suite_hash_str,
            observed_metrics: merged_observed_metrics,
        },
        env_constraints: None,
        evidence_refs: plan.evidence_refs.clone(),
        morph_version: morph_version.map(String::from),
        morph_instance,
    });
    let hash = store.put(&commit)?;

    let branch = current_branch(store)?.unwrap_or_else(|| crate::commit::DEFAULT_BRANCH.to_string());
    store.ref_write(&format!("heads/{}", branch), &hash)?;

    Ok(hash)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{Blob, EvalMetric};

    fn setup_repo() -> (tempfile::TempDir, Box<dyn Store>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _ = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();
        (dir, store)
    }

    fn make_suite(metrics: Vec<EvalMetric>) -> EvalSuite {
        EvalSuite { cases: vec![], metrics }
    }

    fn setup_two_branches(
        store: &dyn Store,
        root: &Path,
        head_suite: &EvalSuite,
        other_suite: &EvalSuite,
        head_metrics: BTreeMap<String, f64>,
        other_metrics: BTreeMap<String, f64>,
    ) {
        let prog = MorphObject::Blob(Blob { kind: "p".into(), content: serde_json::json!({}) });
        let prog_hash = store.put(&prog).unwrap();

        let suite_a_obj = MorphObject::EvalSuite(head_suite.clone());
        let suite_a_hash = store.put(&suite_a_obj).unwrap();

        let suite_b_obj = MorphObject::EvalSuite(other_suite.clone());
        let suite_b_hash = store.put(&suite_b_obj).unwrap();

        std::fs::write(root.join("a.txt"), "a").unwrap();
        crate::add_paths(store, root, &[std::path::PathBuf::from(".")]).unwrap();
        crate::create_tree_commit(
            store, root, Some(&prog_hash), Some(&suite_a_hash),
            head_metrics, "main commit".into(), None, Some("0.3"),
        ).unwrap();
        let main_hash = crate::resolve_head(store).unwrap().unwrap();

        store.ref_write("heads/feature", &main_hash).unwrap();
        crate::set_head_branch(store, "feature").unwrap();

        std::fs::write(root.join("b.txt"), "b").unwrap();
        crate::add_paths(store, root, &[std::path::PathBuf::from(".")]).unwrap();
        crate::create_tree_commit(
            store, root, Some(&prog_hash), Some(&suite_b_hash),
            other_metrics, "feature commit".into(), None, Some("0.3"),
        ).unwrap();
        let feature_hash = crate::resolve_head(store).unwrap().unwrap();

        store.ref_write("heads/feature", &feature_hash).unwrap();
        store.ref_write("heads/main", &main_hash).unwrap();
        crate::set_head_branch(store, "main").unwrap();
    }

    // PR 6 stage C cycle 10: pure helper round-trip.
    #[test]
    fn union_evidence_refs_dedupes_and_sorts() {
        let a: Vec<String> = vec!["zzz".into(), "aaa".into(), "mmm".into()];
        let b: Vec<String> = vec!["aaa".into(), "bbb".into()];
        let out = union_evidence_refs(Some(&a), Some(&b)).unwrap();
        assert_eq!(out, vec!["aaa".to_string(), "bbb".into(), "mmm".into(), "zzz".into()]);
    }

    #[test]
    fn union_evidence_refs_returns_none_when_both_empty() {
        // Both `None` → None, both `Some(empty)` → None, mixed → None.
        assert_eq!(union_evidence_refs(None, None), None);
        let empty: Vec<String> = vec![];
        assert_eq!(union_evidence_refs(Some(&empty), Some(&empty)), None);
        assert_eq!(union_evidence_refs(None, Some(&empty)), None);
    }

    #[test]
    fn union_evidence_refs_handles_one_sided() {
        let a: Vec<String> = vec!["x".into(), "y".into()];
        let out = union_evidence_refs(Some(&a), None).unwrap();
        assert_eq!(out, vec!["x".to_string(), "y".into()]);
        let out = union_evidence_refs(None, Some(&a)).unwrap();
        assert_eq!(out, vec!["x".to_string(), "y".into()]);
    }

    // Phase 6b: case provenance.
    #[test]
    fn prepare_merge_collects_introduces_cases_per_branch() {
        let (dir, store) = setup_repo();
        let suite = make_suite(vec![EvalMetric::new("acc", "mean", 0.0)]);
        let mut m1 = BTreeMap::new();
        m1.insert("acc".into(), 0.9);
        let mut m2 = BTreeMap::new();
        m2.insert("acc".into(), 0.85);
        setup_two_branches(store.as_ref(), dir.path(), &suite, &suite, m1, m2);

        // Annotate main HEAD (case `alpha`) and feature HEAD (case `beta`).
        let main_hash = store.ref_read("heads/main").unwrap().unwrap();
        let feature_hash = store.ref_read("heads/feature").unwrap().unwrap();
        let mut data_main = BTreeMap::new();
        data_main.insert(
            "cases".to_string(),
            serde_json::Value::Array(vec![serde_json::json!("alpha")]),
        );
        let ann_main = crate::create_annotation(
            &main_hash, None, "introduces_cases".into(), data_main, None,
        );
        store.put(&ann_main).unwrap();

        let mut data_feat = BTreeMap::new();
        data_feat.insert(
            "cases".to_string(),
            serde_json::Value::Array(vec![
                serde_json::json!("beta"),
                serde_json::json!("gamma"),
            ]),
        );
        let ann_feat = crate::create_annotation(
            &feature_hash, None, "introduces_cases".into(), data_feat, None,
        );
        store.put(&ann_feat).unwrap();

        let plan = prepare_merge(store.as_ref(), "feature", None, None).unwrap();
        // Merge base is the shared `main commit` (same hash on
        // both branches before we forked); annotations on `main`
        // before the fork are excluded, but our annotation lives
        // on `main_hash` which IS the merge base. So head_branch
        // (still on `main`) reports zero new cases, and `feature`
        // reports its own [beta, gamma]. This matches the
        // documented contract: cases from the shared base are
        // shared history, not branch-introduced.
        assert_eq!(plan.head_introduces_cases, Vec::<String>::new());
        assert_eq!(
            plan.other_introduces_cases,
            vec!["beta".to_string(), "gamma".into()],
        );

        let txt = plan.format_plan();
        assert!(txt.contains("Case provenance:"), "missing header: {txt}");
        assert!(txt.contains("introduces 0 case(s)"), "head 0: {txt}");
        assert!(txt.contains("introduces 2 case(s): beta, gamma"), "feature 2: {txt}");
        assert!(txt.contains("Merged candidate must pass all 2"), "union: {txt}");
    }

    #[test]
    fn prepare_merge_computes_union_suite() {
        let (dir, store) = setup_repo();
        let suite_a = make_suite(vec![EvalMetric::new("acc", "mean", 0.0)]);
        let suite_b = make_suite(vec![EvalMetric::new("f1", "mean", 0.0)]);
        let mut m1 = BTreeMap::new();
        m1.insert("acc".into(), 0.9);
        let mut m2 = BTreeMap::new();
        m2.insert("f1".into(), 0.85);
        setup_two_branches(store.as_ref(), dir.path(), &suite_a, &suite_b, m1, m2);

        let plan = prepare_merge(store.as_ref(), "feature", None, None).unwrap();
        assert_eq!(plan.union_suite.metrics.len(), 2);
        assert!(plan.union_suite.metrics.iter().any(|m| m.name == "acc"));
        assert!(plan.union_suite.metrics.iter().any(|m| m.name == "f1"));
    }

    #[test]
    fn prepare_merge_computes_reference_bar_maximize() {
        let (dir, store) = setup_repo();
        let suite = make_suite(vec![EvalMetric::new("acc", "mean", 0.0)]);
        let mut m1 = BTreeMap::new();
        m1.insert("acc".into(), 0.9);
        let mut m2 = BTreeMap::new();
        m2.insert("acc".into(), 0.85);
        setup_two_branches(store.as_ref(), dir.path(), &suite, &suite, m1, m2);

        let plan = prepare_merge(store.as_ref(), "feature", None, None).unwrap();
        assert_eq!(*plan.reference_bar.get("acc").unwrap(), 0.9);
    }

    #[test]
    fn prepare_merge_reference_bar_minimize() {
        let (dir, store) = setup_repo();
        let suite = make_suite(vec![EvalMetric {
            name: "latency".into(),
            aggregation: "p95".into(),
            threshold: 5.0,
            direction: "minimize".into(),
        }]);
        let mut m1 = BTreeMap::new();
        m1.insert("latency".into(), 2.0);
        let mut m2 = BTreeMap::new();
        m2.insert("latency".into(), 3.0);
        setup_two_branches(store.as_ref(), dir.path(), &suite, &suite, m1, m2);

        let plan = prepare_merge(store.as_ref(), "feature", None, None).unwrap();
        assert_eq!(*plan.reference_bar.get("latency").unwrap(), 2.0,
            "minimize reference bar should be the min (strictest)");
    }

    #[test]
    fn dominance_explanation_identifies_blocking_metric() {
        let (dir, store) = setup_repo();
        let suite = make_suite(vec![EvalMetric::new("acc", "mean", 0.0)]);
        let mut m1 = BTreeMap::new();
        m1.insert("acc".into(), 0.9);
        let mut m2 = BTreeMap::new();
        m2.insert("acc".into(), 0.85);
        setup_two_branches(store.as_ref(), dir.path(), &suite, &suite, m1, m2);

        let plan = prepare_merge(store.as_ref(), "feature", None, None).unwrap();
        let mut bad = BTreeMap::new();
        bad.insert("acc".into(), 0.87);
        let result = plan.check_dominance(&bad);
        assert!(!result.passed);
        assert!(result.violations.iter().any(|v| v.metric == "acc" && v.parent_label == "current"),
            "should identify acc violation against current branch");
        assert!(!result.violations.iter().any(|v| v.parent_label == "other"),
            "should pass against other branch (0.87 >= 0.85)");
    }

    #[test]
    fn dominance_explanation_direction_aware() {
        let (dir, store) = setup_repo();
        let suite = make_suite(vec![EvalMetric {
            name: "latency".into(),
            aggregation: "p95".into(),
            threshold: 5.0,
            direction: "minimize".into(),
        }]);
        let mut m1 = BTreeMap::new();
        m1.insert("latency".into(), 2.0);
        let mut m2 = BTreeMap::new();
        m2.insert("latency".into(), 3.0);
        setup_two_branches(store.as_ref(), dir.path(), &suite, &suite, m1, m2);

        let plan = prepare_merge(store.as_ref(), "feature", None, None).unwrap();

        let mut good = BTreeMap::new();
        good.insert("latency".into(), 1.5);
        assert!(plan.check_dominance(&good).passed);

        let mut bad = BTreeMap::new();
        bad.insert("latency".into(), 2.5);
        let result = plan.check_dominance(&bad);
        assert!(!result.passed);
        assert!(result.violations.iter().any(|v| v.metric == "latency" && v.direction == "minimize"));
    }

    #[test]
    fn retirement_removes_metric_from_dominance_check() {
        let (dir, store) = setup_repo();
        let suite = make_suite(vec![
            EvalMetric::new("acc", "mean", 0.0),
            EvalMetric::new("old_metric", "mean", 0.0),
        ]);
        let mut m1 = BTreeMap::new();
        m1.insert("acc".into(), 0.9);
        m1.insert("old_metric".into(), 0.8);
        let mut m2 = BTreeMap::new();
        m2.insert("acc".into(), 0.85);
        m2.insert("old_metric".into(), 0.7);
        setup_two_branches(store.as_ref(), dir.path(), &suite, &suite, m1, m2);

        let plan_no_retire = prepare_merge(store.as_ref(), "feature", None, None).unwrap();
        let mut merged = BTreeMap::new();
        merged.insert("acc".into(), 0.92);
        let result = plan_no_retire.check_dominance(&merged);
        assert!(!result.passed, "should fail without retirement (missing old_metric)");

        let plan_retire = prepare_merge(
            store.as_ref(), "feature", None, Some(&["old_metric".to_string()]),
        ).unwrap();
        assert_eq!(plan_retire.union_suite.metrics.len(), 1);
        assert!(plan_retire.check_dominance(&merged).passed,
            "should pass with old_metric retired");
    }

    #[test]
    fn prepare_merge_incompatible_suites_fails() {
        let (dir, store) = setup_repo();
        let suite_a = make_suite(vec![EvalMetric::new("acc", "mean", 0.8)]);
        let suite_b = make_suite(vec![EvalMetric::new("acc", "mean", 0.9)]);
        let mut m1 = BTreeMap::new();
        m1.insert("acc".into(), 0.9);
        let mut m2 = BTreeMap::new();
        m2.insert("acc".into(), 0.95);
        setup_two_branches(store.as_ref(), dir.path(), &suite_a, &suite_b, m1, m2);

        let result = prepare_merge(store.as_ref(), "feature", None, None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("defined differently"),
            "error should mention conflicting metric: {}", msg);
    }

    #[test]
    fn execute_merge_succeeds_when_dominating() {
        let (dir, store) = setup_repo();
        let suite = make_suite(vec![EvalMetric::new("acc", "mean", 0.0)]);
        let mut m1 = BTreeMap::new();
        m1.insert("acc".into(), 0.9);
        let mut m2 = BTreeMap::new();
        m2.insert("acc".into(), 0.85);
        setup_two_branches(store.as_ref(), dir.path(), &suite, &suite, m1, m2);

        let plan = prepare_merge(store.as_ref(), "feature", None, None).unwrap();
        let prog = MorphObject::Blob(Blob { kind: "p".into(), content: serde_json::json!({}) });
        let prog_hash = store.put(&prog).unwrap();
        let mut merged = BTreeMap::new();
        merged.insert("acc".into(), 0.92);

        let hash = execute_merge(
            store.as_ref(), &plan, &prog_hash, merged,
            "merge".into(), None, None, Some("0.3"),
        ).unwrap();

        let commit = match store.get(&hash).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        assert_eq!(commit.parents.len(), 2);
        assert_eq!(commit.parents[0], plan.head_hash.to_string());
        assert_eq!(commit.parents[1], plan.other_hash.to_string());
    }

    #[test]
    fn execute_merge_fails_with_detailed_explanation() {
        let (dir, store) = setup_repo();
        let suite = make_suite(vec![EvalMetric::new("acc", "mean", 0.0)]);
        let mut m1 = BTreeMap::new();
        m1.insert("acc".into(), 0.9);
        let mut m2 = BTreeMap::new();
        m2.insert("acc".into(), 0.85);
        setup_two_branches(store.as_ref(), dir.path(), &suite, &suite, m1, m2);

        let plan = prepare_merge(store.as_ref(), "feature", None, None).unwrap();
        let prog = MorphObject::Blob(Blob { kind: "p".into(), content: serde_json::json!({}) });
        let prog_hash = store.put(&prog).unwrap();
        let mut merged = BTreeMap::new();
        merged.insert("acc".into(), 0.87);

        let result = execute_merge(
            store.as_ref(), &plan, &prog_hash, merged,
            "merge".into(), None, None, None,
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("merge rejected"), "should say 'merge rejected': {}", msg);
        assert!(msg.contains("acc"), "should name the blocking metric: {}", msg);
        assert!(msg.contains("current"), "should identify the parent: {}", msg);
    }

    #[test]
    fn execute_merge_skips_dominance_when_policy_is_none() {
        // `RepoPolicy.merge_policy = "none"` should let a merge land
        // even when the candidate metrics regress against a parent.
        // This pins the documented escape hatch in MERGE.md so it
        // stays wired to the gate.
        let (dir, store) = setup_repo();
        let suite = make_suite(vec![EvalMetric::new("acc", "mean", 0.0)]);
        let mut m1 = BTreeMap::new();
        m1.insert("acc".into(), 0.9);
        let mut m2 = BTreeMap::new();
        m2.insert("acc".into(), 0.85);
        setup_two_branches(store.as_ref(), dir.path(), &suite, &suite, m1, m2);

        let policy = crate::policy::RepoPolicy {
            merge_policy: "none".into(),
            ..Default::default()
        };
        crate::policy::write_policy(&dir.path().join(".morph"), &policy).unwrap();

        let plan = prepare_merge(store.as_ref(), "feature", None, None).unwrap();
        let prog = MorphObject::Blob(Blob { kind: "p".into(), content: serde_json::json!({}) });
        let prog_hash = store.put(&prog).unwrap();
        let mut merged = BTreeMap::new();
        merged.insert("acc".into(), 0.5);

        let hash = execute_merge(
            store.as_ref(), &plan, &prog_hash, merged,
            "merge".into(), None, Some(dir.path()), None,
        ).expect("merge_policy=none must let the regression land");
        assert!(matches!(store.get(&hash).unwrap(), MorphObject::Commit(_)));
    }

    /// Helper: rewrite the commit at `branch_ref` to set its
    /// `evidence_refs` to `refs`. Used by the cycle 11/12 tests
    /// below to construct parent commits with deterministic
    /// evidence without going through the full `record` pipeline.
    fn set_branch_evidence(store: &dyn Store, branch_ref: &str, refs: Option<Vec<String>>) {
        let h = store.ref_read(branch_ref).unwrap().unwrap();
        let mut c = match store.get(&h).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("not a commit"),
        };
        c.evidence_refs = refs;
        let new = MorphObject::Commit(c);
        let new_hash = store.put(&new).unwrap();
        store.ref_write(branch_ref, &new_hash).unwrap();
    }

    /// PR 6 stage C cycle 11: when both parents carry evidence,
    /// the resulting merge commit's `evidence_refs` is the deduped
    /// sorted union.
    #[test]
    fn execute_merge_writes_evidence_union_to_commit() {
        let (dir, store) = setup_repo();
        let suite = make_suite(vec![EvalMetric::new("acc", "mean", 0.0)]);
        let mut m1 = BTreeMap::new();
        m1.insert("acc".into(), 0.9);
        let mut m2 = BTreeMap::new();
        m2.insert("acc".into(), 0.85);
        setup_two_branches(store.as_ref(), dir.path(), &suite, &suite, m1, m2);

        // Inject evidence onto each parent before merging.
        set_branch_evidence(
            store.as_ref(),
            "heads/main",
            Some(vec!["run-A".into(), "shared".into()]),
        );
        set_branch_evidence(
            store.as_ref(),
            "heads/feature",
            Some(vec!["run-B".into(), "shared".into()]),
        );

        let plan = prepare_merge(store.as_ref(), "feature", None, None).unwrap();
        // Sanity: plan reflects the union before we call execute_merge.
        assert_eq!(
            plan.evidence_refs.as_deref(),
            Some(["run-A", "run-B", "shared"].as_slice().iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice())
        );

        let prog = MorphObject::Blob(Blob { kind: "p".into(), content: serde_json::json!({}) });
        let prog_hash = store.put(&prog).unwrap();
        let mut merged = BTreeMap::new();
        merged.insert("acc".into(), 0.92);
        let h = execute_merge(
            store.as_ref(), &plan, &prog_hash, merged,
            "merge".into(), None, None, Some("0.5"),
        ).unwrap();
        let commit = match store.get(&h).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        assert_eq!(
            commit.evidence_refs.as_deref(),
            Some(["run-A", "run-B", "shared"].as_slice().iter().map(|s| s.to_string()).collect::<Vec<_>>().as_slice()),
            "merge commit should carry the deduped sorted union of parent evidence"
        );
    }

    /// PR 6 stage C cycle 12: when neither parent has evidence_refs,
    /// the merge commit must keep `evidence_refs = None` rather than
    /// emitting an empty array — that would change canonical hashes
    /// for legacy histories that never recorded evidence.
    #[test]
    fn execute_merge_keeps_none_when_neither_parent_has_evidence() {
        let (dir, store) = setup_repo();
        let suite = make_suite(vec![EvalMetric::new("acc", "mean", 0.0)]);
        let mut m1 = BTreeMap::new();
        m1.insert("acc".into(), 0.9);
        let mut m2 = BTreeMap::new();
        m2.insert("acc".into(), 0.85);
        setup_two_branches(store.as_ref(), dir.path(), &suite, &suite, m1, m2);
        // Both parents start with evidence_refs = None (default).

        let plan = prepare_merge(store.as_ref(), "feature", None, None).unwrap();
        assert_eq!(plan.evidence_refs, None);

        let prog = MorphObject::Blob(Blob { kind: "p".into(), content: serde_json::json!({}) });
        let prog_hash = store.put(&prog).unwrap();
        let mut merged = BTreeMap::new();
        merged.insert("acc".into(), 0.92);
        let h = execute_merge(
            store.as_ref(), &plan, &prog_hash, merged,
            "merge".into(), None, None, Some("0.5"),
        ).unwrap();
        let commit = match store.get(&h).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        assert_eq!(commit.evidence_refs, None);
    }

    #[test]
    fn format_plan_contains_expected_sections() {
        let (dir, store) = setup_repo();
        let suite = make_suite(vec![EvalMetric::new("acc", "mean", 0.0)]);
        let mut m1 = BTreeMap::new();
        m1.insert("acc".into(), 0.9);
        let mut m2 = BTreeMap::new();
        m2.insert("acc".into(), 0.85);
        setup_two_branches(store.as_ref(), dir.path(), &suite, &suite, m1, m2);

        let plan = prepare_merge(store.as_ref(), "feature", None, None).unwrap();
        let text = plan.format_plan();
        assert!(text.contains("Merge plan"), "should have header");
        assert!(text.contains(&plan.head_hash.to_string()), "should have head hash");
        assert!(text.contains(&plan.other_hash.to_string()), "should have other hash");
        assert!(text.contains("Reference bar"), "should have reference bar");
        assert!(text.contains(">= 0.9"), "should show reference bar value");
        assert!(text.contains("Retired metrics: none"), "should show no retired metrics");
    }
}
