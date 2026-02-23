//! Commit creation, HEAD resolution, and ref helpers.

use crate::objects::{Commit, EvalContract, EvalSuite, MorphObject};
use crate::store::{MorphError, Store};
use crate::Hash;
use chrono::Utc;
use std::collections::BTreeMap;
use std::path::Path;

const HEAD_REF: &str = "HEAD";
const DEFAULT_BRANCH: &str = "main";

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
    program_hash: &Hash,
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
        program: program_hash.to_string(),
        parents: parent_list,
        message: message.clone(),
        timestamp: timestamp.clone(),
        author: author.clone(),
        eval_contract: EvalContract {
            suite: eval_suite_hash.to_string(),
            observed_metrics,
        },
        morph_version: None,
    });
    let hash = store.put(&commit)?;

    let branch = current_branch(store)?.unwrap_or_else(|| DEFAULT_BRANCH.to_string());
    store.ref_write(&format!("heads/{}", branch), &hash)?;

    Ok(hash)
}

/// Create a commit with tree built from the staging index.
/// `program_hash` and `eval_suite_hash` are optional: defaults to identity program / empty eval suite.
/// Clears the staging index after commit.
pub fn create_tree_commit(
    store: &dyn Store,
    repo_root: &Path,
    program_hash: Option<&Hash>,
    eval_suite_hash: Option<&Hash>,
    observed_metrics: BTreeMap<String, f64>,
    message: String,
    author: Option<String>,
    morph_version: Option<&str>,
) -> Result<Hash, MorphError> {
    let morph_dir = repo_root.join(".morph");
    let index = crate::index::read_index(&morph_dir)?;
    let tree_hash = crate::tree::build_tree(store, &index.entries)?;

    let prog_hash = match program_hash {
        Some(h) => h.to_string(),
        None => {
            let identity = crate::identity::identity_program();
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

    let commit = MorphObject::Commit(Commit {
        tree: Some(tree_hash.to_string()),
        program: prog_hash,
        parents: parent_list,
        message,
        timestamp,
        author,
        eval_contract: EvalContract {
            suite: suite_hash,
            observed_metrics,
        },
        morph_version: morph_version.map(String::from),
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
pub fn checkout_tree(
    store: &dyn Store,
    repo_root: &Path,
    ref_name: &str,
) -> Result<(Hash, bool), MorphError> {
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

    let tree_restored = if let Some(tree_hash_str) = &commit.tree {
        let tree_hash = Hash::from_hex(tree_hash_str)?;
        crate::tree::restore_tree(store, &tree_hash, repo_root)?;
        true
    } else {
        false
    };

    let _ = is_branch;
    Ok((hash, tree_restored))
}

/// Create a merge commit. Validates that merged_observed_metrics dominate both parents.
/// other_branch: name of branch to merge in (e.g. "feature"). Current HEAD is the other parent.
pub fn create_merge_commit(
    store: &dyn Store,
    other_branch: &str,
    merged_program_hash: &Hash,
    merged_observed_metrics: BTreeMap<String, f64>,
    eval_suite_hash: &Hash,
    message: String,
    author: Option<String>,
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
        tree: None,
        program: merged_program_hash.to_string(),
        parents,
        message,
        timestamp,
        author,
        eval_contract: EvalContract {
            suite: eval_suite_hash.to_string(),
            observed_metrics: merged_observed_metrics,
        },
        morph_version: None,
    });
    let hash = store.put(&commit)?;

    let branch = current_branch(store)?.unwrap_or_else(|| DEFAULT_BRANCH.to_string());
    store.ref_write(&format!("heads/{}", branch), &hash)?;

    Ok(hash)
}

/// Rollup (squash) a range: one new commit with parent = base, program and eval_contract from tip.
pub fn rollup(
    store: &dyn Store,
    base_ref: &str,
    tip_ref: &str,
    message: Option<String>,
) -> Result<Hash, MorphError> {
    let base_path = if base_ref == "HEAD" {
        resolve_head(store)?
    } else {
        let p = if base_ref.starts_with("heads/") { base_ref.to_string() } else { format!("heads/{}", base_ref) };
        store.ref_read(&p)?
    };
    let base_hash = base_path.ok_or_else(|| MorphError::NotFound(base_ref.into()))?;

    let tip_path = if tip_ref == "HEAD" {
        resolve_head(store)?
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
        tree: None,
        program: tip_commit.program.clone(),
        parents: vec![base_hash.to_string()],
        message,
        timestamp,
        author: tip_commit.author.clone(),
        eval_contract: tip_commit.eval_contract.clone(),
        morph_version: None,
    });
    let hash = store.put(&commit)?;

    let branch = current_branch(store)?.unwrap_or_else(|| DEFAULT_BRANCH.to_string());
    store.ref_write(&format!("heads/{}", branch), &hash)?;

    Ok(hash)
}

/// List commit hashes from a starting ref (e.g. HEAD or heads/main), following parents.
pub fn log_from(store: &dyn Store, start_ref: &str) -> Result<Vec<Hash>, MorphError> {
    let mut current = if start_ref == "HEAD" {
        resolve_head(store)?
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
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();

        let prog = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({}) });
        let prog_hash = store.put(&prog).unwrap();
        let suite = MorphObject::Blob(Blob { kind: "eval".into(), content: serde_json::json!({}) });
        let suite_hash = store.put(&suite).unwrap();

        let mut metrics = BTreeMap::new();
        metrics.insert("acc".to_string(), 0.9);
        let hash = create_commit(
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
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();

        let prog = MorphObject::Blob(Blob { kind: "p".into(), content: serde_json::json!({}) });
        let prog_hash = store.put(&prog).unwrap();
        let suite = MorphObject::Blob(Blob { kind: "e".into(), content: serde_json::json!({}) });
        let suite_hash = store.put(&suite).unwrap();

        let mut m1 = BTreeMap::new();
        m1.insert("acc".to_string(), 0.9);
        let c1 = create_commit(&store, &prog_hash, &suite_hash, m1.clone(), "main".into(), None).unwrap();

        store.ref_write("heads/feature", &c1).unwrap();
        let mut m2 = BTreeMap::new();
        m2.insert("acc".to_string(), 0.85);
        let c2 = create_commit(&store, &prog_hash, &suite_hash, m2, "feature".into(), None).unwrap();
        store.ref_write("heads/feature", &c2).unwrap();
        store.ref_write("heads/main", &c1).unwrap();
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();

        let mut merged_bad = BTreeMap::new();
        merged_bad.insert("acc".to_string(), 0.88);
        let r = create_merge_commit(
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
    fn create_tree_commit_defaults_program_and_eval() {
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
        let prog_hash = Hash::from_hex(&commit.program).unwrap();
        let prog = store.get(&prog_hash).unwrap();
        assert!(matches!(prog, MorphObject::Program(_)));

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
}
