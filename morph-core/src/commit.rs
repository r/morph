//! Commit creation, HEAD resolution, and ref helpers.

use crate::objects::{Commit, CommitContributor, EvalContract, EvalSuite, MorphObject};
use crate::store::{MorphError, Store};
use crate::Hash;
use chrono::Utc;
use std::collections::BTreeMap;
use std::path::Path;

/// Provenance derived from a Run, to be attached to a commit.
/// Groups evidence_refs, env_constraints, and contributors into a single value
/// so callers don't thread three unrelated optionals through every layer.
#[derive(Clone, Debug)]
pub struct CommitProvenance {
    pub evidence_refs: Vec<String>,
    pub env_constraints: BTreeMap<String, serde_json::Value>,
    pub contributors: Vec<CommitContributor>,
}

/// Load a stored Run by hash and derive commit provenance from it.
///
/// Validates that the object is a Run and that its trace resolves to a Trace.
/// Returns deterministic evidence_refs (run hash, then trace hash),
/// env_constraints mapped from Run.environment, and contributors from
/// Run.agent + Run.contributors.
pub fn resolve_provenance_from_run(
    store: &dyn Store,
    run_hash: &Hash,
) -> Result<CommitProvenance, MorphError> {
    let obj = store.get(run_hash)?;
    let run = match obj {
        MorphObject::Run(r) => r,
        _ => return Err(MorphError::Serialization(format!(
            "object {} is not a Run", run_hash
        ))),
    };

    let trace_hash = Hash::from_hex(&run.trace)
        .map_err(|_| MorphError::InvalidHash(run.trace.clone()))?;
    match store.get(&trace_hash)? {
        MorphObject::Trace(_) => {}
        _ => return Err(MorphError::Serialization(format!(
            "object {} (referenced by run.trace) is not a Trace", run.trace
        ))),
    }

    let mut evidence_refs = vec![run_hash.to_string(), trace_hash.to_string()];
    evidence_refs.dedup();

    let mut env_constraints = BTreeMap::new();
    env_constraints.insert("model".into(), serde_json::Value::String(run.environment.model.clone()));
    env_constraints.insert("version".into(), serde_json::Value::String(run.environment.version.clone()));
    env_constraints.insert("parameters".into(), serde_json::to_value(&run.environment.parameters).unwrap_or_default());
    env_constraints.insert("toolchain".into(), serde_json::to_value(&run.environment.toolchain).unwrap_or_default());

    let mut seen = std::collections::BTreeSet::new();
    let mut contributors = Vec::new();

    seen.insert(run.agent.id.clone());
    contributors.push(CommitContributor {
        id: run.agent.id.clone(),
        role: Some("primary".into()),
    });

    if let Some(run_contribs) = &run.contributors {
        for c in run_contribs {
            if seen.insert(c.id.clone()) {
                contributors.push(CommitContributor {
                    id: c.id.clone(),
                    role: c.role.clone(),
                });
            }
        }
    }

    Ok(CommitProvenance {
        evidence_refs,
        env_constraints,
        contributors,
    })
}

const HEAD_REF: &str = "HEAD";
pub const DEFAULT_BRANCH: &str = "main";

/// Resolve HEAD to a commit hash. HEAD may be "ref: heads/main" or a raw hash (detached).
pub fn resolve_head(store: &dyn Store) -> Result<Option<Hash>, MorphError> {
    let content = match store.ref_read_raw(HEAD_REF)? {
        Some(c) => c,
        None => return Ok(None),
    };
    let content = content.trim();
    if let Some(rest) = content.strip_prefix("ref: ") {
        let ref_path = rest.trim();
        return store.ref_read(ref_path);
    }
    Hash::from_hex(content).map(Some).map_err(|_| MorphError::InvalidHash("HEAD".into()))
}

/// Current branch name if HEAD is symbolic, else None.
pub fn current_branch(store: &dyn Store) -> Result<Option<String>, MorphError> {
    let content = match store.ref_read_raw(HEAD_REF)? {
        Some(c) => c,
        None => return Ok(None),
    };
    let content = content.trim();
    if let Some(rest) = content.strip_prefix("ref: ") {
        let path = rest.trim();
        if let Some(name) = path.strip_prefix("heads/") {
            return Ok(Some(name.to_string()));
        }
    }
    Ok(None)
}

/// Create a commit and update the current branch (or create refs/heads/main if first commit).
pub fn create_commit(
    store: &dyn Store,
    pipeline_hash: &Hash,
    eval_suite_hash: &Hash,
    observed_metrics: BTreeMap<String, f64>,
    message: String,
    author: Option<String>,
) -> Result<Hash, MorphError> {
    let parent_list: Vec<String> = resolve_head(store)?
        .map(|h| vec![h.to_string()])
        .unwrap_or_default();
    let timestamp = Utc::now().to_rfc3339();
    let author = author.unwrap_or_else(|| "morph".to_string());
    let commit = MorphObject::Commit(Commit {
        tree: None,
        pipeline: pipeline_hash.to_string(),
        parents: parent_list,
        message: message.clone(),
        timestamp: timestamp.clone(),
        author: author.clone(),
        contributors: None,
        eval_contract: EvalContract {
            suite: eval_suite_hash.to_string(),
            observed_metrics,
        },
        env_constraints: None,
        evidence_refs: None,
        morph_version: None,
        morph_instance: None,
    });
    let hash = store.put(&commit)?;

    let branch = current_branch(store)?.unwrap_or_else(|| DEFAULT_BRANCH.to_string());
    store.ref_write(&format!("heads/{}", branch), &hash)?;

    Ok(hash)
}

/// Create a commit with tree built from the staging index.
/// `pipeline_hash` and `eval_suite_hash` are optional: defaults to identity pipeline / empty eval suite.
/// `provenance` is optional: when provided, populates evidence_refs, env_constraints, and contributors.
/// Clears the staging index after commit.
#[allow(clippy::too_many_arguments)] // commits naturally carry many fields;
                                     // a builder felt heavier than an
                                     // ordered keyword list at the call site
pub fn create_tree_commit(
    store: &dyn Store,
    repo_root: &Path,
    pipeline_hash: Option<&Hash>,
    eval_suite_hash: Option<&Hash>,
    observed_metrics: BTreeMap<String, f64>,
    message: String,
    author: Option<String>,
    morph_version: Option<&str>,
) -> Result<Hash, MorphError> {
    create_tree_commit_with_provenance(
        store, repo_root, pipeline_hash, eval_suite_hash,
        observed_metrics, message, author, morph_version, None,
    )
}

/// Create a tree commit with optional run-backed provenance.
#[allow(clippy::too_many_arguments)] // see `create_tree_commit`
pub fn create_tree_commit_with_provenance(
    store: &dyn Store,
    repo_root: &Path,
    pipeline_hash: Option<&Hash>,
    eval_suite_hash: Option<&Hash>,
    observed_metrics: BTreeMap<String, f64>,
    message: String,
    author: Option<String>,
    morph_version: Option<&str>,
    provenance: Option<&CommitProvenance>,
) -> Result<Hash, MorphError> {
    let morph_dir = repo_root.join(".morph");
    let index = crate::index::read_index(&morph_dir)?;
    let canonical_root = repo_root.canonicalize().unwrap_or_else(|_| repo_root.to_path_buf());
    let ignore_rules = crate::morphignore::load_ignore_rules(&canonical_root);
    let filtered: std::collections::BTreeMap<String, String> = index
        .entries
        .into_iter()
        .filter(|(rel, _)| !crate::morphignore::is_rel_path_ignored(ignore_rules.as_ref(), rel, false))
        .collect();
    let tree_hash = crate::tree::build_tree(store, &filtered)?;

    let prog_hash = match pipeline_hash {
        Some(h) => h.to_string(),
        None => {
            let identity = crate::identity::identity_pipeline();
            store.put(&identity)?.to_string()
        }
    };
    let suite_hash = match eval_suite_hash {
        Some(h) => h.to_string(),
        None => {
            let empty_suite = MorphObject::EvalSuite(EvalSuite {
                cases: vec![],
                metrics: vec![],
            });
            store.put(&empty_suite)?.to_string()
        }
    };

    let parent_list: Vec<String> = resolve_head(store)?
        .map(|h| vec![h.to_string()])
        .unwrap_or_default();
    let timestamp = Utc::now().to_rfc3339();
    let author = author.unwrap_or_else(|| "morph".to_string());

    let (contributors, env_constraints, evidence_refs) = match provenance {
        Some(p) => (
            Some(p.contributors.clone()),
            Some(p.env_constraints.clone()),
            Some(p.evidence_refs.clone()),
        ),
        None => (None, None, None),
    };

    let commit = MorphObject::Commit(Commit {
        tree: Some(tree_hash.to_string()),
        pipeline: prog_hash,
        parents: parent_list,
        message,
        timestamp,
        author,
        contributors,
        eval_contract: EvalContract {
            suite: suite_hash,
            observed_metrics,
        },
        env_constraints,
        evidence_refs,
        morph_version: morph_version.map(String::from),
        morph_instance: crate::agent::read_instance_id(&morph_dir)?,
    });
    let hash = store.put(&commit)?;

    let branch = current_branch(store)?.unwrap_or_else(|| DEFAULT_BRANCH.to_string());
    store.ref_write(&format!("heads/{}", branch), &hash)?;

    crate::index::clear_index(&morph_dir)?;

    Ok(hash)
}

/// Set HEAD to a branch (symbolic ref).
pub fn set_head_branch(store: &dyn Store, branch: &str) -> Result<(), MorphError> {
    store.ref_write_raw(HEAD_REF, &format!("ref: heads/{}", branch))
}

/// Set HEAD to a commit hash (detached HEAD).
pub fn set_head_detached(store: &dyn Store, hash: &Hash) -> Result<(), MorphError> {
    store.ref_write_raw(HEAD_REF, &hash.to_string())
}

/// Checkout a branch or commit: set HEAD and restore the working tree from the commit's tree.
/// If the commit has no tree (pre-0.3), only sets HEAD without touching the working tree.
/// Files from the previous commit's tree that are absent in the target tree are removed.
pub fn checkout_tree(
    store: &dyn Store,
    repo_root: &Path,
    ref_name: &str,
) -> Result<(Hash, bool), MorphError> {
    let old_tree_hash_str = resolve_head(store)?
        .and_then(|h| {
            if let Ok(MorphObject::Commit(c)) = store.get(&h) {
                c.tree.clone()
            } else {
                None
            }
        });

    let (hash, is_branch) = if ref_name.len() == 64 && ref_name.chars().all(|c| c.is_ascii_hexdigit()) {
        let h = Hash::from_hex(ref_name)?;
        set_head_detached(store, &h)?;
        (h, false)
    } else {
        let ref_path = if ref_name.starts_with("heads/") {
            ref_name.to_string()
        } else {
            format!("heads/{}", ref_name)
        };
        let h = store
            .ref_read(&ref_path)?
            .ok_or_else(|| MorphError::NotFound(ref_name.into()))?;
        let branch_name = ref_name.trim_start_matches("heads/");
        set_head_branch(store, branch_name)?;
        (h, true)
    };

    let commit = match store.get(&hash)? {
        MorphObject::Commit(c) => c,
        _ => return Err(MorphError::Serialization("not a commit".into())),
    };

    let canonical_root = repo_root.canonicalize().unwrap_or_else(|_| repo_root.to_path_buf());
    let ignore_rules = crate::morphignore::load_ignore_rules(&canonical_root);

    let tree_restored = if let Some(tree_hash_str) = &commit.tree {
        let tree_hash = Hash::from_hex(tree_hash_str)?;

        if let Some(old_hash_str) = &old_tree_hash_str {
            if let Ok(old_hash) = Hash::from_hex(old_hash_str) {
                if let Ok(old_files) = crate::tree::flatten_tree(store, &old_hash) {
                    let new_files = crate::tree::flatten_tree(store, &tree_hash)?;
                    for path in old_files.keys() {
                        if crate::morphignore::is_rel_path_ignored(ignore_rules.as_ref(), path, false) {
                            continue;
                        }
                        if !new_files.contains_key(path) {
                            let full = repo_root.join(path);
                            if full.exists() {
                                let _ = std::fs::remove_file(&full);
                            }
                        }
                    }
                }
            }
        }

        crate::tree::restore_tree_filtered(store, &tree_hash, repo_root, ignore_rules.as_ref())?;
        true
    } else {
        false
    };

    let _is_branch = is_branch;
    Ok((hash, tree_restored))
}

/// Create a merge commit with full theory compliance:
/// - Computes the union eval suite from both parents (THEORY.md §13.1)
/// - Checks direction-aware dominance against both parents (THEORY.md §13.3)
/// - Builds a tree from the staging index if `repo_root` is provided
/// - Falls back to the old single-suite path when `eval_suite_hash` is given explicitly
///
/// `other_branch`: name of branch to merge in (e.g. "feature"). Current HEAD is the other parent.
/// `repo_root`: if provided, builds tree from staging index and clears index after commit.
/// `eval_suite_hash`: if None, auto-computes union of both parents' suites.
#[allow(clippy::too_many_arguments)] // see `create_tree_commit`
pub fn create_merge_commit(
    store: &dyn Store,
    other_branch: &str,
    merged_pipeline_hash: &Hash,
    merged_observed_metrics: BTreeMap<String, f64>,
    eval_suite_hash: &Hash,
    message: String,
    author: Option<String>,
) -> Result<Hash, MorphError> {
    create_merge_commit_full(store, other_branch, merged_pipeline_hash, merged_observed_metrics, Some(eval_suite_hash), message, author, None, None)
}

/// Full merge commit with optional tree, auto-computed union suite, and metric retirement.
/// `retired_metrics`: optional list of metric names to remove from the union suite (paper §5.3).
#[allow(clippy::too_many_arguments)] // see `create_tree_commit`
pub fn create_merge_commit_full(
    store: &dyn Store,
    other_branch: &str,
    merged_pipeline_hash: &Hash,
    merged_observed_metrics: BTreeMap<String, f64>,
    eval_suite_hash: Option<&Hash>,
    message: String,
    author: Option<String>,
    repo_root: Option<&Path>,
    morph_version: Option<&str>,
) -> Result<Hash, MorphError> {
    create_merge_commit_with_retirement(store, other_branch, merged_pipeline_hash, merged_observed_metrics, eval_suite_hash, message, author, repo_root, morph_version, None)
}

/// Merge commit with full metric retirement support (paper §5.3).
#[allow(clippy::too_many_arguments)] // see `create_tree_commit`
pub fn create_merge_commit_with_retirement(
    store: &dyn Store,
    other_branch: &str,
    merged_pipeline_hash: &Hash,
    merged_observed_metrics: BTreeMap<String, f64>,
    eval_suite_hash: Option<&Hash>,
    message: String,
    author: Option<String>,
    repo_root: Option<&Path>,
    morph_version: Option<&str>,
    retired_metrics: Option<&[String]>,
) -> Result<Hash, MorphError> {
    let head_hash = resolve_head(store)?.ok_or_else(|| MorphError::Serialization("no HEAD commit".into()))?;
    let other_ref = if other_branch.starts_with("heads/") {
        other_branch.to_string()
    } else {
        format!("heads/{}", other_branch)
    };
    let other_hash = store.ref_read(&other_ref)?.ok_or_else(|| MorphError::NotFound(other_branch.into()))?;

    let head_commit = match store.get(&head_hash)? {
        MorphObject::Commit(c) => c,
        _ => return Err(MorphError::Serialization("HEAD is not a commit".into())),
    };
    let other_commit = match store.get(&other_hash)? {
        MorphObject::Commit(c) => c,
        _ => return Err(MorphError::Serialization("other ref is not a commit".into())),
    };

    let suite_hash_str = match eval_suite_hash {
        Some(h) => {
            if !crate::check_dominance(&merged_observed_metrics, &head_commit.eval_contract.observed_metrics) {
                return Err(MorphError::Serialization("merge rejected: merged metrics do not dominate current branch".into()));
            }
            if !crate::check_dominance(&merged_observed_metrics, &other_commit.eval_contract.observed_metrics) {
                return Err(MorphError::Serialization("merge rejected: merged metrics do not dominate other branch".into()));
            }
            h.to_string()
        }
        None => {
            let head_suite = load_eval_suite(store, &head_commit.eval_contract.suite)?;
            let other_suite = load_eval_suite(store, &other_commit.eval_contract.suite)?;
            let raw_union = crate::metrics::union_suites(&head_suite, &other_suite)?;

            let union = match retired_metrics {
                Some(retired) if !retired.is_empty() => {
                    crate::metrics::retire_metrics(&raw_union, retired)?
                }
                _ => raw_union,
            };

            if !crate::metrics::check_dominance_with_suite(
                &merged_observed_metrics,
                &head_commit.eval_contract.observed_metrics,
                &union,
            ) {
                return Err(MorphError::Serialization("merge rejected: merged metrics do not dominate current branch".into()));
            }
            if !crate::metrics::check_dominance_with_suite(
                &merged_observed_metrics,
                &other_commit.eval_contract.observed_metrics,
                &union,
            ) {
                return Err(MorphError::Serialization("merge rejected: merged metrics do not dominate other branch".into()));
            }

            let union_obj = MorphObject::EvalSuite(union);
            store.put(&union_obj)?.to_string()
        }
    };

    let tree_hash = if let Some(root) = repo_root {
        let morph_dir = root.join(".morph");
        let index = crate::index::read_index(&morph_dir)?;
        let cr = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let ig = crate::morphignore::load_ignore_rules(&cr);
        let filtered: std::collections::BTreeMap<String, String> = index
            .entries
            .into_iter()
            .filter(|(rel, _)| !crate::morphignore::is_rel_path_ignored(ig.as_ref(), rel, false))
            .collect();
        let h = crate::tree::build_tree(store, &filtered)?;
        crate::index::clear_index(&morph_dir)?;
        Some(h.to_string())
    } else {
        None
    };

    let merged_contributors = merge_contributors(&head_commit, &other_commit);

    let parents = vec![head_hash.to_string(), other_hash.to_string()];
    let timestamp = chrono::Utc::now().to_rfc3339();
    let author = author.unwrap_or_else(|| "morph".to_string());
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
        evidence_refs: None,
        morph_version: morph_version.map(String::from),
        morph_instance: repo_root
            .and_then(|r| crate::agent::read_instance_id(&r.join(".morph")).ok().flatten()),
    });
    let hash = store.put(&commit)?;

    let branch = current_branch(store)?.unwrap_or_else(|| DEFAULT_BRANCH.to_string());
    store.ref_write(&format!("heads/{}", branch), &hash)?;

    Ok(hash)
}

/// Collect unique contributors from both parent commits' authors and contributor lists.
pub fn merge_contributors(head: &Commit, other: &Commit) -> Option<Vec<CommitContributor>> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();

    for commit in [head, other] {
        if !seen.contains(&commit.author) {
            seen.insert(commit.author.clone());
            out.push(CommitContributor { id: commit.author.clone(), role: None });
        }
        if let Some(contribs) = &commit.contributors {
            for c in contribs {
                if !seen.contains(&c.id) {
                    seen.insert(c.id.clone());
                    out.push(c.clone());
                }
            }
        }
    }

    if out.is_empty() { None } else { Some(out) }
}

pub fn load_eval_suite(store: &dyn Store, suite_hash_str: &str) -> Result<EvalSuite, MorphError> {
    let h = Hash::from_hex(suite_hash_str)?;
    match store.get(&h)? {
        MorphObject::EvalSuite(s) => Ok(s),
        _ => Ok(EvalSuite { cases: vec![], metrics: vec![] }),
    }
}

/// Rollup (squash) a range: one new commit with parent = base, pipeline and eval_contract from tip.
/// Preserves the tip commit's tree hash if it has one.
pub fn rollup(
    store: &dyn Store,
    base_ref: &str,
    tip_ref: &str,
    message: Option<String>,
) -> Result<Hash, MorphError> {
    let base_hash = resolve_ref(store, base_ref)?
        .ok_or_else(|| MorphError::NotFound(base_ref.into()))?;

    let tip_hash = resolve_ref(store, tip_ref)?
        .ok_or_else(|| MorphError::NotFound(tip_ref.into()))?;

    let tip_commit = match store.get(&tip_hash)? {
        MorphObject::Commit(c) => c,
        _ => return Err(MorphError::Serialization("tip is not a commit".into())),
    };

    let message = message.unwrap_or_else(|| format!("Rollup to {}", tip_hash));
    let timestamp = chrono::Utc::now().to_rfc3339();
    let commit = MorphObject::Commit(Commit {
        tree: tip_commit.tree.clone(),
        pipeline: tip_commit.pipeline.clone(),
        parents: vec![base_hash.to_string()],
        message,
        timestamp,
        author: tip_commit.author.clone(),
        contributors: tip_commit.contributors.clone(),
        eval_contract: tip_commit.eval_contract.clone(),
        env_constraints: tip_commit.env_constraints.clone(),
        evidence_refs: tip_commit.evidence_refs.clone(),
        morph_version: tip_commit.morph_version.clone(),
        morph_instance: tip_commit.morph_instance.clone(),
    });
    let hash = store.put(&commit)?;

    let branch = current_branch(store)?.unwrap_or_else(|| DEFAULT_BRANCH.to_string());
    store.ref_write(&format!("heads/{}", branch), &hash)?;

    Ok(hash)
}

fn resolve_ref(store: &dyn Store, ref_str: &str) -> Result<Option<Hash>, MorphError> {
    if ref_str == "HEAD" {
        resolve_head(store)
    } else if ref_str.len() == 64 && ref_str.chars().all(|c| c.is_ascii_hexdigit()) {
        let h = Hash::from_hex(ref_str)?;
        if store.has(&h)? {
            Ok(Some(h))
        } else {
            Ok(None)
        }
    } else {
        let p = if ref_str.starts_with("heads/") {
            ref_str.to_string()
        } else {
            format!("heads/{}", ref_str)
        };
        store.ref_read(&p)
    }
}

/// List commit hashes from a starting ref (e.g. HEAD or heads/main), following parents.
pub fn log_from(store: &dyn Store, start_ref: &str) -> Result<Vec<Hash>, MorphError> {
    let mut current = resolve_ref(store, start_ref)?;
    let mut out = Vec::new();
    while let Some(h) = current {
        let obj = store.get(&h)?;
        let commit = match &obj {
            MorphObject::Commit(c) => c,
            _ => return Err(MorphError::Serialization("not a commit".into())),
        };
        out.push(h);
        current = commit.parents.first().and_then(|s| Hash::from_hex(s).ok());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::FsStore;
    use crate::objects::Blob;
    use crate::objects::MorphObject;

    #[test]
    fn create_commit_updates_ref() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        std::fs::create_dir_all(store.objects_dir()).unwrap();
        std::fs::create_dir_all(store.refs_dir()).unwrap();
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();

        let pipeline = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({}) });
        let pipeline_hash = store.put(&pipeline).unwrap();
        let suite = MorphObject::Blob(Blob { kind: "eval".into(), content: serde_json::json!({}) });
        let suite_hash = store.put(&suite).unwrap();

        let mut metrics = BTreeMap::new();
        metrics.insert("acc".to_string(), 0.9);
        let hash = create_commit(
            &store,
            &pipeline_hash,
            &suite_hash,
            metrics,
            "first".into(),
            None,
        ).unwrap();
        assert!(store.has(&hash).unwrap());
        let head = resolve_head(&store).unwrap();
        assert_eq!(head, Some(hash));
        let branch_ref = store.ref_read("heads/main").unwrap();
        assert_eq!(branch_ref, Some(hash));
    }

    #[test]
    fn merge_requires_dominance() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        std::fs::create_dir_all(store.objects_dir()).unwrap();
        std::fs::create_dir_all(store.refs_dir()).unwrap();
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();

        let pipeline = MorphObject::Blob(Blob { kind: "p".into(), content: serde_json::json!({}) });
        let pipeline_hash = store.put(&pipeline).unwrap();
        let suite = MorphObject::Blob(Blob { kind: "e".into(), content: serde_json::json!({}) });
        let suite_hash = store.put(&suite).unwrap();

        let mut m1 = BTreeMap::new();
        m1.insert("acc".to_string(), 0.9);
        let c1 = create_commit(&store, &pipeline_hash, &suite_hash, m1.clone(), "main".into(), None).unwrap();

        store.ref_write("heads/feature", &c1).unwrap();
        let mut m2 = BTreeMap::new();
        m2.insert("acc".to_string(), 0.85);
        let c2 = create_commit(&store, &pipeline_hash, &suite_hash, m2, "feature".into(), None).unwrap();
        store.ref_write("heads/feature", &c2).unwrap();
        store.ref_write("heads/main", &c1).unwrap();
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();

        let mut merged_bad = BTreeMap::new();
        merged_bad.insert("acc".to_string(), 0.88);
        let r = create_merge_commit(
            &store,
            "feature",
            &pipeline_hash,
            merged_bad,
            &suite_hash,
            "merge".into(),
            None,
        );
        assert!(r.is_err());

        let mut merged_good = BTreeMap::new();
        merged_good.insert("acc".to_string(), 0.92);
        let merge_hash = create_merge_commit(
            &store,
            "feature",
            &pipeline_hash,
            merged_good,
            &suite_hash,
            "merge".into(),
            None,
        ).unwrap();
        let merge_commit = match store.get(&merge_hash).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!(),
        };
        assert_eq!(merge_commit.parents.len(), 2);
    }

    #[test]
    fn create_tree_commit_stores_tree_hash() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _store = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();

        std::fs::write(root.join("file.txt"), "content").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();

        let hash = create_tree_commit(
            store.as_ref(),
            root,
            None,
            None,
            BTreeMap::new(),
            "test commit".into(),
            None,
            Some("0.3"),
        )
        .unwrap();

        let obj = store.get(&hash).unwrap();
        let commit = match &obj {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        assert!(commit.tree.is_some(), "commit should have tree");
        assert_eq!(commit.morph_version.as_deref(), Some("0.3"));

        let tree_hash = Hash::from_hex(commit.tree.as_ref().unwrap()).unwrap();
        let flat = crate::tree::flatten_tree(store.as_ref(), &tree_hash).unwrap();
        assert!(flat.contains_key("file.txt"), "tree should contain file.txt");
    }

    #[test]
    fn create_tree_commit_defaults_pipeline_and_eval() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _store = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();

        std::fs::write(root.join("x.txt"), "x").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();

        let hash = create_tree_commit(
            store.as_ref(),
            root,
            None,
            None,
            BTreeMap::new(),
            "defaults".into(),
            None,
            None,
        )
        .unwrap();

        let commit = match store.get(&hash).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        let prog_hash = Hash::from_hex(&commit.pipeline).unwrap();
        let prog = store.get(&prog_hash).unwrap();
        assert!(matches!(prog, MorphObject::Pipeline(_)));

        let suite_hash = Hash::from_hex(&commit.eval_contract.suite).unwrap();
        let suite = store.get(&suite_hash).unwrap();
        assert!(matches!(suite, MorphObject::EvalSuite(_)));
    }

    #[test]
    fn create_tree_commit_clears_index() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _store = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();

        std::fs::write(root.join("f.txt"), "data").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();

        let index_before = crate::index::read_index(&morph_dir).unwrap();
        assert!(!index_before.is_empty(), "index should have entries before commit");

        create_tree_commit(
            store.as_ref(),
            root,
            None,
            None,
            BTreeMap::new(),
            "commit".into(),
            None,
            None,
        )
        .unwrap();

        let index_after = crate::index::read_index(&morph_dir).unwrap();
        assert!(index_after.is_empty(), "index should be cleared after commit");
    }

    // ── merge with auto union suite and tree ─────────────────────────

    #[test]
    fn merge_full_auto_union_suite() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _fs = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();

        let pipeline = MorphObject::Blob(Blob { kind: "p".into(), content: serde_json::json!({}) });
        let pipeline_hash = store.put(&pipeline).unwrap();

        let suite_a = MorphObject::EvalSuite(crate::objects::EvalSuite {
            cases: vec![],
            metrics: vec![crate::objects::EvalMetric::new("acc", "mean", 0.0)],
        });
        let suite_a_hash = store.put(&suite_a).unwrap();

        let suite_b = MorphObject::EvalSuite(crate::objects::EvalSuite {
            cases: vec![],
            metrics: vec![crate::objects::EvalMetric::new("f1", "mean", 0.0)],
        });
        let suite_b_hash = store.put(&suite_b).unwrap();

        std::fs::write(root.join("a.txt"), "a").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();
        let mut m1 = BTreeMap::new();
        m1.insert("acc".to_string(), 0.9);
        create_tree_commit(store.as_ref(), root, Some(&pipeline_hash), Some(&suite_a_hash), m1, "c1".into(), None, Some("0.3")).unwrap();

        let c1 = resolve_head(store.as_ref()).unwrap().unwrap();
        store.ref_write("heads/feature", &c1).unwrap();

        std::fs::write(root.join("b.txt"), "b").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();
        let mut m2 = BTreeMap::new();
        m2.insert("f1".to_string(), 0.85);
        create_tree_commit(store.as_ref(), root, Some(&pipeline_hash), Some(&suite_b_hash), m2, "c2".into(), None, Some("0.3")).unwrap();

        store.ref_write("heads/feature", &resolve_head(store.as_ref()).unwrap().unwrap()).unwrap();
        store.ref_write("heads/main", &c1).unwrap();
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();

        std::fs::write(root.join("merged.txt"), "merged").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();

        let mut merged_metrics = BTreeMap::new();
        merged_metrics.insert("acc".to_string(), 0.92);
        merged_metrics.insert("f1".to_string(), 0.88);

        let merge_hash = create_merge_commit_full(
            store.as_ref(),
            "feature",
            &pipeline_hash,
            merged_metrics,
            None,
            "merge".into(),
            None,
            Some(root),
            Some("0.3"),
        ).unwrap();

        let merge_commit = match store.get(&merge_hash).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        assert_eq!(merge_commit.parents.len(), 2);
        assert!(merge_commit.tree.is_some(), "merge commit should have tree");

        let suite_obj = store.get(&Hash::from_hex(&merge_commit.eval_contract.suite).unwrap()).unwrap();
        let suite = match suite_obj {
            MorphObject::EvalSuite(s) => s,
            _ => panic!("expected eval suite"),
        };
        assert_eq!(suite.metrics.len(), 2, "union suite should have both metrics");
    }

    #[test]
    fn merge_full_rejects_when_not_dominating() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _fs = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();

        let pipeline = MorphObject::Blob(Blob { kind: "p".into(), content: serde_json::json!({}) });
        let pipeline_hash = store.put(&pipeline).unwrap();

        let suite = MorphObject::EvalSuite(crate::objects::EvalSuite {
            cases: vec![],
            metrics: vec![crate::objects::EvalMetric::new("acc", "mean", 0.0)],
        });
        let suite_hash = store.put(&suite).unwrap();

        std::fs::write(root.join("a.txt"), "a").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();
        let mut m1 = BTreeMap::new();
        m1.insert("acc".to_string(), 0.9);
        create_tree_commit(store.as_ref(), root, Some(&pipeline_hash), Some(&suite_hash), m1, "c1".into(), None, None).unwrap();

        let c1 = resolve_head(store.as_ref()).unwrap().unwrap();
        store.ref_write("heads/feature", &c1).unwrap();

        std::fs::write(root.join("b.txt"), "b").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();
        let mut m2 = BTreeMap::new();
        m2.insert("acc".to_string(), 0.85);
        create_tree_commit(store.as_ref(), root, Some(&pipeline_hash), Some(&suite_hash), m2, "c2".into(), None, None).unwrap();

        store.ref_write("heads/feature", &resolve_head(store.as_ref()).unwrap().unwrap()).unwrap();
        store.ref_write("heads/main", &c1).unwrap();
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();

        let mut bad_metrics = BTreeMap::new();
        bad_metrics.insert("acc".to_string(), 0.87);

        let result = create_merge_commit_full(
            store.as_ref(),
            "feature",
            &pipeline_hash,
            bad_metrics,
            None,
            "merge".into(),
            None,
            None,
            None,
        );
        assert!(result.is_err(), "should reject merge when not dominating HEAD's acc=0.9");
    }

    // ── rollup preserves tree ────────────────────────────────────────

    #[test]
    fn rollup_preserves_tip_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _fs = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();

        std::fs::write(root.join("a.txt"), "aaa").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();
        let c1 = create_tree_commit(store.as_ref(), root, None, None, BTreeMap::new(), "c1".into(), None, Some("0.3")).unwrap();

        store.ref_write("heads/base", &c1).unwrap();

        std::fs::write(root.join("b.txt"), "bbb").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();
        let c2 = create_tree_commit(store.as_ref(), root, None, None, BTreeMap::new(), "c2".into(), None, Some("0.3")).unwrap();

        let tip_commit = match store.get(&c2).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!(),
        };
        assert!(tip_commit.tree.is_some());

        let rollup_hash = rollup(store.as_ref(), "base", "HEAD", Some("squashed".into())).unwrap();
        let rollup_commit = match store.get(&rollup_hash).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!(),
        };
        assert_eq!(rollup_commit.tree, tip_commit.tree, "rollup should preserve tip's tree");
        assert_eq!(rollup_commit.morph_version.as_deref(), Some("0.3"), "rollup should preserve morph_version");
    }

    // ── checkout_tree error paths ────────────────────────────────────

    #[test]
    fn checkout_tree_nonexistent_branch_returns_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _fs = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();

        let result = checkout_tree(store.as_ref(), root, "nosuchbranch");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, MorphError::NotFound(_)));
    }

    #[test]
    fn checkout_tree_detached_head() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _fs = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();

        std::fs::write(root.join("x.txt"), "x").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();
        let c1 = create_tree_commit(store.as_ref(), root, None, None, BTreeMap::new(), "c1".into(), None, Some("0.3")).unwrap();

        let (hash, tree_restored) = checkout_tree(store.as_ref(), root, &c1.to_string()).unwrap();
        assert_eq!(hash, c1);
        assert!(tree_restored);

        let branch = current_branch(store.as_ref()).unwrap();
        assert!(branch.is_none(), "HEAD should be detached");
    }

    // ── log_from edge cases ──────────────────────────────────────────

    #[test]
    fn log_from_empty_repo_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _fs = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();

        let log = log_from(store.as_ref(), "HEAD").unwrap();
        assert!(log.is_empty());
    }

    #[test]
    fn log_from_invalid_ref_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _fs = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();

        let log = log_from(store.as_ref(), "nosuchbranch").unwrap();
        assert!(log.is_empty(), "non-existent branch should return empty log");
    }

    // ── create_tree_commit with empty index ──────────────────────────

    #[test]
    fn create_tree_commit_empty_index_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _fs = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();

        let hash = create_tree_commit(
            store.as_ref(), root, None, None, BTreeMap::new(), "empty".into(), None, Some("0.3"),
        ).unwrap();

        let commit = match store.get(&hash).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        assert!(commit.tree.is_some(), "should still have a tree (empty tree)");
    }

    // ── rollup edge case: base == tip ────────────────────────────────

    #[test]
    fn rollup_base_equals_tip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _fs = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();

        std::fs::write(root.join("a.txt"), "aaa").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();
        let c1 = create_tree_commit(store.as_ref(), root, None, None, BTreeMap::new(), "only".into(), None, Some("0.3")).unwrap();

        let rollup_hash = rollup(store.as_ref(), &c1.to_string(), &c1.to_string(), Some("self-rollup".into())).unwrap();

        let rollup_commit = match store.get(&rollup_hash).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        assert_eq!(rollup_commit.parents.len(), 1);
        assert_eq!(rollup_commit.parents[0], c1.to_string());
        assert_eq!(rollup_commit.message, "self-rollup");
    }

    // ── provenance tests ─────────────────────────────────────────────

    fn setup_repo() -> (tempfile::TempDir, Box<dyn crate::store::Store>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _ = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();
        (dir, store)
    }

    fn store_test_run(store: &dyn crate::store::Store) -> (Hash, Hash) {
        use crate::objects::*;

        let trace = MorphObject::Trace(Trace {
            events: vec![TraceEvent {
                id: "evt_1".into(),
                seq: 0,
                ts: "2025-01-01T00:00:00Z".into(),
                kind: "prompt".into(),
                payload: BTreeMap::new(),
            }],
        });
        let trace_hash = store.put(&trace).unwrap();

        let identity = crate::identity::identity_pipeline();
        let pipeline_hash = store.put(&identity).unwrap();

        let mut params = BTreeMap::new();
        params.insert("temperature".into(), serde_json::json!(0.7));
        let mut toolchain = BTreeMap::new();
        toolchain.insert("rust".into(), serde_json::json!("1.75"));

        let run = MorphObject::Run(Run {
            pipeline: pipeline_hash.to_string(),
            commit: None,
            environment: RunEnvironment {
                model: "gpt-4o".into(),
                version: "2025-01-01".into(),
                parameters: params,
                toolchain,
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: BTreeMap::new(),
            trace: trace_hash.to_string(),
            agent: AgentInfo {
                id: "agent-1".into(),
                version: "1.0".into(),
                policy: None,
                instance_id: None,
            },
            contributors: Some(vec![
                ContributorInfo {
                    id: "agent-2".into(),
                    version: "2.0".into(),
                    policy: None,
                    instance_id: None,
                    role: Some("review".into()),
                },
            ]),
            morph_version: None,
        });
        let run_hash = store.put(&run).unwrap();
        (run_hash, trace_hash)
    }

    #[test]
    fn tree_commit_without_from_run_leaves_provenance_absent() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("f.txt"), "data").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();

        let hash = create_tree_commit(
            store.as_ref(), root, None, None, BTreeMap::new(),
            "plain".into(), None, Some("0.3"),
        ).unwrap();

        let commit = match store.get(&hash).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        assert!(commit.evidence_refs.is_none(), "evidence_refs should be absent");
        assert!(commit.env_constraints.is_none(), "env_constraints should be absent");
        assert!(commit.contributors.is_none(), "contributors should be absent");
    }

    #[test]
    fn tree_commit_from_run_persists_evidence_refs() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        let (run_hash, trace_hash) = store_test_run(store.as_ref());

        std::fs::write(root.join("f.txt"), "data").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();

        let provenance = resolve_provenance_from_run(store.as_ref(), &run_hash).unwrap();
        let hash = create_tree_commit_with_provenance(
            store.as_ref(), root, None, None, BTreeMap::new(),
            "with-run".into(), None, Some("0.3"), Some(&provenance),
        ).unwrap();

        let commit = match store.get(&hash).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        let refs = commit.evidence_refs.as_ref().expect("evidence_refs should be present");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0], run_hash.to_string());
        assert_eq!(refs[1], trace_hash.to_string());
    }

    #[test]
    fn tree_commit_from_run_persists_env_constraints() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        let (run_hash, _) = store_test_run(store.as_ref());

        std::fs::write(root.join("f.txt"), "data").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();

        let provenance = resolve_provenance_from_run(store.as_ref(), &run_hash).unwrap();
        let hash = create_tree_commit_with_provenance(
            store.as_ref(), root, None, None, BTreeMap::new(),
            "env".into(), None, Some("0.3"), Some(&provenance),
        ).unwrap();

        let commit = match store.get(&hash).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        let env = commit.env_constraints.as_ref().expect("env_constraints should be present");
        assert_eq!(env.get("model").and_then(|v| v.as_str()), Some("gpt-4o"));
        assert_eq!(env.get("version").and_then(|v| v.as_str()), Some("2025-01-01"));
        assert!(env.contains_key("parameters"));
        assert!(env.contains_key("toolchain"));
    }

    #[test]
    fn tree_commit_from_run_persists_contributors_deduped() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        let (run_hash, _) = store_test_run(store.as_ref());

        std::fs::write(root.join("f.txt"), "data").unwrap();
        crate::add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();

        let provenance = resolve_provenance_from_run(store.as_ref(), &run_hash).unwrap();
        let hash = create_tree_commit_with_provenance(
            store.as_ref(), root, None, None, BTreeMap::new(),
            "contribs".into(), None, Some("0.3"), Some(&provenance),
        ).unwrap();

        let commit = match store.get(&hash).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        let contribs = commit.contributors.as_ref().expect("contributors should be present");
        assert_eq!(contribs.len(), 2);
        assert_eq!(contribs[0].id, "agent-1");
        assert_eq!(contribs[0].role.as_deref(), Some("primary"));
        assert_eq!(contribs[1].id, "agent-2");
        assert_eq!(contribs[1].role.as_deref(), Some("review"));
    }

    #[test]
    fn resolve_provenance_fails_on_missing_object() {
        let (_, store) = setup_repo();
        let fake_hash = Hash::from_hex(&"a".repeat(64)).unwrap();
        let result = resolve_provenance_from_run(store.as_ref(), &fake_hash);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_provenance_fails_on_non_run_object() {
        let (_, store) = setup_repo();
        let blob = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({}) });
        let blob_hash = store.put(&blob).unwrap();
        let result = resolve_provenance_from_run(store.as_ref(), &blob_hash);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not a Run"), "error should mention 'not a Run': {}", err_msg);
    }

    #[test]
    fn resolve_provenance_fails_on_missing_trace() {
        use crate::objects::*;
        let (_, store) = setup_repo();

        let identity = crate::identity::identity_pipeline();
        let pipeline_hash = store.put(&identity).unwrap();

        let run = MorphObject::Run(Run {
            pipeline: pipeline_hash.to_string(),
            commit: None,
            environment: RunEnvironment {
                model: "test".into(),
                version: "1".into(),
                parameters: BTreeMap::new(),
                toolchain: BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: BTreeMap::new(),
            trace: "b".repeat(64),
            agent: AgentInfo { id: "a".into(), version: "1".into(), policy: None, instance_id: None },
            contributors: None,
            morph_version: None,
        });
        let run_hash = store.put(&run).unwrap();
        let result = resolve_provenance_from_run(store.as_ref(), &run_hash);
        assert!(result.is_err());
    }
}
