//! Commit creation, HEAD resolution, and ref helpers.

use crate::objects::{Commit, EvalContract, MorphObject};
use crate::store::{MorphError, Store};
use crate::Hash;
use chrono::Utc;
use std::collections::BTreeMap;

const HEAD_REF: &str = "HEAD";
const DEFAULT_BRANCH: &str = "main";

/// Read raw ref file content (e.g. for HEAD which may be "ref: heads/main").
fn ref_read_raw(store: &crate::store::FsStore, name: &str) -> Result<Option<String>, MorphError> {
    let path = store.refs_dir().join(name);
    if !path.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(&path)?.trim().to_string();
    Ok(if s.is_empty() { None } else { Some(s) })
}

/// Resolve HEAD to a commit hash. HEAD may be "ref: heads/main" or a raw hash (detached).
pub fn resolve_head(store: &crate::store::FsStore) -> Result<Option<Hash>, MorphError> {
    let content = match ref_read_raw(store, HEAD_REF)? {
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
pub fn current_branch(store: &crate::store::FsStore) -> Result<Option<String>, MorphError> {
    let content = match ref_read_raw(store, HEAD_REF)? {
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
    store_fs: &crate::store::FsStore,
    program_hash: &Hash,
    eval_suite_hash: &Hash,
    observed_metrics: BTreeMap<String, f64>,
    message: String,
    author: Option<String>,
) -> Result<Hash, MorphError> {
    let parent_list: Vec<String> = resolve_head(store_fs)?
        .map(|h| vec![h.to_string()])
        .unwrap_or_default();
    let timestamp = Utc::now().to_rfc3339();
    let author = author.unwrap_or_else(|| "morph".to_string());
    let commit = MorphObject::Commit(Commit {
        program: program_hash.to_string(),
        parents: parent_list,
        message: message.clone(),
        timestamp: timestamp.clone(),
        author: author.clone(),
        eval_contract: EvalContract {
            suite: eval_suite_hash.to_string(),
            observed_metrics,
        },
    });
    let hash = store.put(&commit)?;

    let branch = current_branch(store_fs)?.unwrap_or_else(|| DEFAULT_BRANCH.to_string());
    store.ref_write(&format!("heads/{}", branch), &hash)?;

    Ok(hash)
}

/// Set HEAD to a branch (symbolic ref).
pub fn set_head_branch(store: &crate::store::FsStore, branch: &str) -> Result<(), MorphError> {
    let path = store.refs_dir().join(HEAD_REF);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("ref: heads/{}\n", branch))?;
    Ok(())
}

/// Set HEAD to a commit hash (detached HEAD).
pub fn set_head_detached(store: &crate::store::FsStore, hash: &Hash) -> Result<(), MorphError> {
    let path = store.refs_dir().join(HEAD_REF);
    std::fs::write(path, format!("{}\n", hash))?;
    Ok(())
}

/// Create a merge commit. Validates that merged_observed_metrics dominate both parents.
/// other_branch: name of branch to merge in (e.g. "feature"). Current HEAD is the other parent.
pub fn create_merge_commit(
    store: &dyn Store,
    store_fs: &crate::store::FsStore,
    other_branch: &str,
    merged_program_hash: &Hash,
    merged_observed_metrics: BTreeMap<String, f64>,
    eval_suite_hash: &Hash,
    message: String,
    author: Option<String>,
) -> Result<Hash, MorphError> {
    let head_hash = resolve_head(store_fs)?.ok_or_else(|| MorphError::Serialization("no HEAD commit".into()))?;
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

    if !crate::check_dominance(&merged_observed_metrics, &head_commit.eval_contract.observed_metrics) {
        return Err(MorphError::Serialization("merge rejected: merged metrics do not dominate current branch".into()));
    }
    if !crate::check_dominance(&merged_observed_metrics, &other_commit.eval_contract.observed_metrics) {
        return Err(MorphError::Serialization("merge rejected: merged metrics do not dominate other branch".into()));
    }

    let parents = vec![head_hash.to_string(), other_hash.to_string()];
    let timestamp = chrono::Utc::now().to_rfc3339();
    let author = author.unwrap_or_else(|| "morph".to_string());
    let commit = MorphObject::Commit(Commit {
        program: merged_program_hash.to_string(),
        parents,
        message,
        timestamp,
        author,
        eval_contract: EvalContract {
            suite: eval_suite_hash.to_string(),
            observed_metrics: merged_observed_metrics,
        },
    });
    let hash = store.put(&commit)?;

    let branch = current_branch(store_fs)?.unwrap_or_else(|| DEFAULT_BRANCH.to_string());
    store.ref_write(&format!("heads/{}", branch), &hash)?;

    Ok(hash)
}

/// Rollup (squash) a range: one new commit with parent = base, program and eval_contract from tip.
pub fn rollup(
    store: &dyn Store,
    store_fs: &crate::store::FsStore,
    base_ref: &str,
    tip_ref: &str,
    message: Option<String>,
) -> Result<Hash, MorphError> {
    let base_path = if base_ref == "HEAD" {
        resolve_head(store_fs)?
    } else {
        let p = if base_ref.starts_with("heads/") { base_ref.to_string() } else { format!("heads/{}", base_ref) };
        store.ref_read(&p)?
    };
    let base_hash = base_path.ok_or_else(|| MorphError::NotFound(base_ref.into()))?;

    let tip_path = if tip_ref == "HEAD" {
        resolve_head(store_fs)?
    } else {
        let p = if tip_ref.starts_with("heads/") { tip_ref.to_string() } else { format!("heads/{}", tip_ref) };
        store.ref_read(&p)?
    };
    let tip_hash = tip_path.ok_or_else(|| MorphError::NotFound(tip_ref.into()))?;

    let tip_commit = match store.get(&tip_hash)? {
        MorphObject::Commit(c) => c,
        _ => return Err(MorphError::Serialization("tip is not a commit".into())),
    };

    let message = message.unwrap_or_else(|| format!("Rollup to {}", tip_hash));
    let timestamp = chrono::Utc::now().to_rfc3339();
    let commit = MorphObject::Commit(Commit {
        program: tip_commit.program.clone(),
        parents: vec![base_hash.to_string()],
        message,
        timestamp,
        author: tip_commit.author.clone(),
        eval_contract: tip_commit.eval_contract.clone(),
    });
    let hash = store.put(&commit)?;

    let branch = current_branch(store_fs)?.unwrap_or_else(|| DEFAULT_BRANCH.to_string());
    store.ref_write(&format!("heads/{}", branch), &hash)?;

    Ok(hash)
}

/// List commit hashes from a starting ref (e.g. HEAD or heads/main), following parents.
pub fn log_from(store: &dyn Store, store_fs: &crate::store::FsStore, start_ref: &str) -> Result<Vec<Hash>, MorphError> {
    let mut current = if start_ref == "HEAD" {
        resolve_head(store_fs)?
    } else {
        let path = if start_ref.starts_with("heads/") {
            start_ref.to_string()
        } else {
            format!("heads/{}", start_ref)
        };
        store.ref_read(&path)?
    };
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
        std::fs::write(store.refs_dir().join("HEAD"), "ref: heads/main\n").unwrap();

        let prog = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({}) });
        let prog_hash = store.put(&prog).unwrap();
        let suite = MorphObject::Blob(Blob { kind: "eval".into(), content: serde_json::json!({}) });
        let suite_hash = store.put(&suite).unwrap();

        let mut metrics = BTreeMap::new();
        metrics.insert("acc".to_string(), 0.9);
        let hash = create_commit(
            &store,
            &store,
            &prog_hash,
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
        std::fs::write(store.refs_dir().join("HEAD"), "ref: heads/main\n").unwrap();

        let prog = MorphObject::Blob(Blob { kind: "p".into(), content: serde_json::json!({}) });
        let prog_hash = store.put(&prog).unwrap();
        let suite = MorphObject::Blob(Blob { kind: "e".into(), content: serde_json::json!({}) });
        let suite_hash = store.put(&suite).unwrap();

        let mut m1 = BTreeMap::new();
        m1.insert("acc".to_string(), 0.9);
        let c1 = create_commit(&store, &store, &prog_hash, &suite_hash, m1.clone(), "main".into(), None).unwrap();

        store.ref_write("heads/feature", &c1).unwrap();
        let mut m2 = BTreeMap::new();
        m2.insert("acc".to_string(), 0.85);
        let c2 = create_commit(&store, &store, &prog_hash, &suite_hash, m2, "feature".into(), None).unwrap();
        store.ref_write("heads/feature", &c2).unwrap();
        store.ref_write("heads/main", &c1).unwrap();
        std::fs::write(store.refs_dir().join("HEAD"), "ref: heads/main\n").unwrap();

        let mut merged_bad = BTreeMap::new();
        merged_bad.insert("acc".to_string(), 0.88);
        let r = create_merge_commit(
            &store,
            &store,
            "feature",
            &prog_hash,
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
            &store,
            "feature",
            &prog_hash,
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
}
