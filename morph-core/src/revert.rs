//! Revert: create a new commit that undoes a previous commit's tree changes.
//!
//! The revert commit's tree matches the parent of the reverted commit.
//! For root commits (no parent), the revert produces an empty tree.

use crate::commit::resolve_head;
use crate::objects::{Commit, EvalContract, MorphObject};
use crate::store::{MorphError, Store};
use crate::tree::empty_tree_hash;
use crate::Hash;
use chrono::Utc;
use std::collections::BTreeMap;

/// Create a revert commit that undoes the given commit's changes.
/// The new commit's tree is the parent's tree (or empty for root commits).
pub fn revert_commit(
    store: &dyn Store,
    target_hash: &Hash,
    author: Option<String>,
) -> Result<Hash, MorphError> {
    let obj = store.get(target_hash)?;
    let target = match obj {
        MorphObject::Commit(c) => c,
        _ => {
            return Err(MorphError::Serialization(format!(
                "object {} is not a commit",
                target_hash
            )))
        }
    };

    let parent_tree = if let Some(parent_hash_str) = target.parents.first() {
        let parent_hash = Hash::from_hex(parent_hash_str)?;
        let parent_obj = store.get(&parent_hash)?;
        match parent_obj {
            MorphObject::Commit(pc) => pc.tree.clone(),
            _ => None,
        }
    } else {
        Some(empty_tree_hash(store)?.to_string())
    };

    let head = resolve_head(store)?
        .ok_or_else(|| MorphError::Serialization("no HEAD to revert onto".into()))?;

    let revert = MorphObject::Commit(Commit {
        tree: parent_tree,
        pipeline: target.pipeline.clone(),
        parents: vec![head.to_string()],
        message: format!("Revert \"{}\"", target.message),
        timestamp: Utc::now().to_rfc3339(),
        author: author.unwrap_or_else(|| target.author.clone()),
        contributors: None,
        eval_contract: EvalContract {
            suite: target.eval_contract.suite.clone(),
            observed_metrics: BTreeMap::new(),
        },
        env_constraints: None,
        evidence_refs: None,
        morph_version: None,
    });

    let revert_hash = store.put(&revert)?;

    let branch = crate::commit::current_branch(store)?;
    match branch {
        Some(name) => store.ref_write(&format!("heads/{}", name), &revert_hash)?,
        None => store.ref_write("HEAD", &revert_hash)?,
    }

    Ok(revert_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{Blob, Commit, EvalContract, EvalSuite, MorphObject};

    fn setup_repo() -> (tempfile::TempDir, Box<dyn Store>) {
        let dir = tempfile::tempdir().unwrap();
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        (dir, store)
    }

    fn make_commit(store: &dyn Store, message: &str, parent: Option<&Hash>, tree_hash: Option<&str>) -> Hash {
        let suite = MorphObject::EvalSuite(EvalSuite {
            cases: vec![],
            metrics: vec![],
        });
        let suite_hash = store.put(&suite).unwrap();
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"x": 1}),
        });
        let prog_hash = store.put(&blob).unwrap();

        let commit = MorphObject::Commit(Commit {
            tree: tree_hash.map(|s| s.to_string()),
            pipeline: prog_hash.to_string(),
            parents: parent.map(|h| vec![h.to_string()]).unwrap_or_default(),
            message: message.into(),
            timestamp: "2025-01-01T00:00:00Z".into(),
            author: "test".into(),
            contributors: None,
            eval_contract: EvalContract {
                suite: suite_hash.to_string(),
                observed_metrics: BTreeMap::new(),
            },
            env_constraints: None,
            evidence_refs: None,
            morph_version: None,
        });
        let hash = store.put(&commit).unwrap();
        store.ref_write("heads/main", &hash).unwrap();
        hash
    }

    #[test]
    fn revert_creates_commit_with_parent_tree() {
        let (_dir, store) = setup_repo();
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();

        let blob_a = MorphObject::Blob(Blob {
            kind: "blob".into(),
            content: serde_json::json!({"body": "v1"}),
        });
        let ha = store.put(&blob_a).unwrap();
        let mut entries = BTreeMap::new();
        entries.insert("file.txt".into(), ha.to_string());
        let tree_a = crate::tree::build_tree(store.as_ref(), &entries).unwrap();

        let c1 = make_commit(store.as_ref(), "first", None, Some(&tree_a.to_string()));

        let blob_b = MorphObject::Blob(Blob {
            kind: "blob".into(),
            content: serde_json::json!({"body": "v2"}),
        });
        let hb = store.put(&blob_b).unwrap();
        let mut entries2 = BTreeMap::new();
        entries2.insert("file.txt".into(), hb.to_string());
        let tree_b = crate::tree::build_tree(store.as_ref(), &entries2).unwrap();

        let c2 = make_commit(store.as_ref(), "second", Some(&c1), Some(&tree_b.to_string()));

        let revert_hash = revert_commit(store.as_ref(), &c2, None).unwrap();
        let obj = store.get(&revert_hash).unwrap();
        if let MorphObject::Commit(c) = obj {
            assert!(c.message.contains("Revert"));
            assert!(c.message.contains("second"));
            assert_eq!(c.tree, Some(tree_a.to_string()));
            assert_eq!(c.parents, vec![c2.to_string()]);
        } else {
            panic!("expected commit");
        }
    }

    #[test]
    fn revert_root_commit_produces_empty_tree() {
        let (_dir, store) = setup_repo();
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();

        let blob = MorphObject::Blob(Blob {
            kind: "blob".into(),
            content: serde_json::json!({"body": "content"}),
        });
        let h = store.put(&blob).unwrap();
        let mut entries = BTreeMap::new();
        entries.insert("file.txt".into(), h.to_string());
        let tree = crate::tree::build_tree(store.as_ref(), &entries).unwrap();

        let c1 = make_commit(store.as_ref(), "root", None, Some(&tree.to_string()));

        let revert_hash = revert_commit(store.as_ref(), &c1, None).unwrap();
        let obj = store.get(&revert_hash).unwrap();
        if let MorphObject::Commit(c) = obj {
            let empty = crate::tree::empty_tree_hash(store.as_ref()).unwrap();
            assert_eq!(c.tree, Some(empty.to_string()));
        } else {
            panic!("expected commit");
        }
    }

    #[test]
    fn revert_non_commit_fails() {
        let (_dir, store) = setup_repo();
        let blob = MorphObject::Blob(Blob {
            kind: "test".into(),
            content: serde_json::json!({}),
        });
        let hash = store.put(&blob).unwrap();
        let result = revert_commit(store.as_ref(), &hash, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not a commit"));
    }

    #[test]
    fn revert_updates_branch_ref() {
        let (_dir, store) = setup_repo();
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();

        let c1 = make_commit(store.as_ref(), "first", None, None);
        let c2 = make_commit(store.as_ref(), "second", Some(&c1), None);

        let revert_hash = revert_commit(store.as_ref(), &c2, Some("reverter".into())).unwrap();
        let head = resolve_head(store.as_ref()).unwrap().unwrap();
        assert_eq!(head, revert_hash);
    }
}
