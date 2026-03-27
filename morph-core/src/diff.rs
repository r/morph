//! Tree diffing: compare two tree snapshots and produce a list of changes.

use crate::objects::MorphObject;
use crate::store::{MorphError, Store};
use crate::tree::flatten_tree;
use crate::Hash;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffStatus {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    pub path: String,
    pub status: DiffStatus,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
}

/// Diff two trees by hash, returning a sorted list of changes.
/// Either tree hash can be None (treated as empty tree).
pub fn diff_trees(
    store: &dyn Store,
    old_tree: Option<&Hash>,
    new_tree: Option<&Hash>,
) -> Result<Vec<DiffEntry>, MorphError> {
    let old_files = match old_tree {
        Some(h) => flatten_tree(store, h)?,
        None => BTreeMap::new(),
    };
    let new_files = match new_tree {
        Some(h) => flatten_tree(store, h)?,
        None => BTreeMap::new(),
    };
    Ok(diff_file_maps(&old_files, &new_files))
}

/// Diff two flat file maps (path -> blob hash).
pub fn diff_file_maps(
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, String>,
) -> Vec<DiffEntry> {
    let mut entries = Vec::new();

    for (path, old_hash) in old {
        match new.get(path) {
            Some(new_hash) if new_hash != old_hash => {
                entries.push(DiffEntry {
                    path: path.clone(),
                    status: DiffStatus::Modified,
                    old_hash: Some(old_hash.clone()),
                    new_hash: Some(new_hash.clone()),
                });
            }
            None => {
                entries.push(DiffEntry {
                    path: path.clone(),
                    status: DiffStatus::Deleted,
                    old_hash: Some(old_hash.clone()),
                    new_hash: None,
                });
            }
            _ => {}
        }
    }

    for (path, new_hash) in new {
        if !old.contains_key(path) {
            entries.push(DiffEntry {
                path: path.clone(),
                status: DiffStatus::Added,
                old_hash: None,
                new_hash: Some(new_hash.clone()),
            });
        }
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

/// Diff two commits by hash. Loads each commit's tree and diffs them.
pub fn diff_commits(
    store: &dyn Store,
    old_commit: &Hash,
    new_commit: &Hash,
) -> Result<Vec<DiffEntry>, MorphError> {
    let old_tree = commit_tree_hash(store, old_commit)?;
    let new_tree = commit_tree_hash(store, new_commit)?;
    diff_trees(store, old_tree.as_ref(), new_tree.as_ref())
}

fn commit_tree_hash(store: &dyn Store, commit_hash: &Hash) -> Result<Option<Hash>, MorphError> {
    let obj = store.get(commit_hash)?;
    match obj {
        MorphObject::Commit(c) => match c.tree {
            Some(ref h) => Ok(Some(Hash::from_hex(h)?)),
            None => Ok(None),
        },
        _ => Err(MorphError::Serialization(format!(
            "object {} is not a commit",
            commit_hash
        ))),
    }
}

impl std::fmt::Display for DiffStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffStatus::Added => write!(f, "A"),
            DiffStatus::Modified => write!(f, "M"),
            DiffStatus::Deleted => write!(f, "D"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{Blob, MorphObject};
    use crate::store::FsStore;
    use crate::tree::build_tree;

    fn make_store() -> (tempfile::TempDir, FsStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        std::fs::create_dir_all(path.join("objects")).unwrap();
        (dir, FsStore::new(path))
    }

    fn store_blob(store: &FsStore, content: &str) -> Hash {
        let blob = MorphObject::Blob(Blob {
            kind: "blob".into(),
            content: serde_json::json!({ "body": content }),
        });
        store.put(&blob).unwrap()
    }

    #[test]
    fn diff_empty_trees_produces_no_changes() {
        let entries = diff_file_maps(&BTreeMap::new(), &BTreeMap::new());
        assert!(entries.is_empty());
    }

    #[test]
    fn diff_added_files() {
        let old = BTreeMap::new();
        let mut new = BTreeMap::new();
        new.insert("a.txt".into(), "h1".into());
        new.insert("b.txt".into(), "h2".into());

        let entries = diff_file_maps(&old, &new);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "a.txt");
        assert_eq!(entries[0].status, DiffStatus::Added);
        assert_eq!(entries[0].old_hash, None);
        assert_eq!(entries[0].new_hash, Some("h1".into()));
    }

    #[test]
    fn diff_deleted_files() {
        let mut old = BTreeMap::new();
        old.insert("removed.txt".into(), "h1".into());
        let new = BTreeMap::new();

        let entries = diff_file_maps(&old, &new);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, DiffStatus::Deleted);
        assert_eq!(entries[0].old_hash, Some("h1".into()));
        assert_eq!(entries[0].new_hash, None);
    }

    #[test]
    fn diff_modified_files() {
        let mut old = BTreeMap::new();
        old.insert("file.txt".into(), "hash_v1".into());
        let mut new = BTreeMap::new();
        new.insert("file.txt".into(), "hash_v2".into());

        let entries = diff_file_maps(&old, &new);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, DiffStatus::Modified);
        assert_eq!(entries[0].old_hash, Some("hash_v1".into()));
        assert_eq!(entries[0].new_hash, Some("hash_v2".into()));
    }

    #[test]
    fn diff_unchanged_files_omitted() {
        let mut old = BTreeMap::new();
        old.insert("same.txt".into(), "hash".into());
        let mut new = BTreeMap::new();
        new.insert("same.txt".into(), "hash".into());

        let entries = diff_file_maps(&old, &new);
        assert!(entries.is_empty());
    }

    #[test]
    fn diff_mixed_changes() {
        let mut old = BTreeMap::new();
        old.insert("kept.txt".into(), "h1".into());
        old.insert("modified.txt".into(), "old".into());
        old.insert("removed.txt".into(), "h3".into());

        let mut new = BTreeMap::new();
        new.insert("kept.txt".into(), "h1".into());
        new.insert("modified.txt".into(), "new".into());
        new.insert("added.txt".into(), "h4".into());

        let entries = diff_file_maps(&old, &new);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "added.txt");
        assert_eq!(entries[0].status, DiffStatus::Added);
        assert_eq!(entries[1].path, "modified.txt");
        assert_eq!(entries[1].status, DiffStatus::Modified);
        assert_eq!(entries[2].path, "removed.txt");
        assert_eq!(entries[2].status, DiffStatus::Deleted);
    }

    #[test]
    fn diff_trees_with_store() {
        let (_dir, store) = make_store();
        let h1 = store_blob(&store, "content_a");
        let h2 = store_blob(&store, "content_b");
        let h3 = store_blob(&store, "content_c");

        let mut old_entries = BTreeMap::new();
        old_entries.insert("a.txt".into(), h1.to_string());
        old_entries.insert("b.txt".into(), h2.to_string());
        let old_tree = build_tree(&store, &old_entries).unwrap();

        let mut new_entries = BTreeMap::new();
        new_entries.insert("a.txt".into(), h1.to_string());
        new_entries.insert("c.txt".into(), h3.to_string());
        let new_tree = build_tree(&store, &new_entries).unwrap();

        let diff = diff_trees(&store, Some(&old_tree), Some(&new_tree)).unwrap();
        assert_eq!(diff.len(), 2);
        assert_eq!(diff[0].path, "b.txt");
        assert_eq!(diff[0].status, DiffStatus::Deleted);
        assert_eq!(diff[1].path, "c.txt");
        assert_eq!(diff[1].status, DiffStatus::Added);
    }

    #[test]
    fn diff_trees_none_to_tree_is_all_added() {
        let (_dir, store) = make_store();
        let h = store_blob(&store, "content");
        let mut entries = BTreeMap::new();
        entries.insert("file.txt".into(), h.to_string());
        let tree = build_tree(&store, &entries).unwrap();

        let diff = diff_trees(&store, None, Some(&tree)).unwrap();
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].status, DiffStatus::Added);
    }

    #[test]
    fn diff_trees_tree_to_none_is_all_deleted() {
        let (_dir, store) = make_store();
        let h = store_blob(&store, "content");
        let mut entries = BTreeMap::new();
        entries.insert("file.txt".into(), h.to_string());
        let tree = build_tree(&store, &entries).unwrap();

        let diff = diff_trees(&store, Some(&tree), None).unwrap();
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].status, DiffStatus::Deleted);
    }

    #[test]
    fn diff_nested_tree_changes() {
        let (_dir, store) = make_store();
        let h1 = store_blob(&store, "v1");
        let h2 = store_blob(&store, "v2");
        let h3 = store_blob(&store, "new_file");

        let mut old_entries = BTreeMap::new();
        old_entries.insert("src/main.rs".into(), h1.to_string());
        let old_tree = build_tree(&store, &old_entries).unwrap();

        let mut new_entries = BTreeMap::new();
        new_entries.insert("src/main.rs".into(), h2.to_string());
        new_entries.insert("src/lib.rs".into(), h3.to_string());
        let new_tree = build_tree(&store, &new_entries).unwrap();

        let diff = diff_trees(&store, Some(&old_tree), Some(&new_tree)).unwrap();
        assert_eq!(diff.len(), 2);
        assert_eq!(diff[0].path, "src/lib.rs");
        assert_eq!(diff[0].status, DiffStatus::Added);
        assert_eq!(diff[1].path, "src/main.rs");
        assert_eq!(diff[1].status, DiffStatus::Modified);
    }

    #[test]
    fn diff_status_display() {
        assert_eq!(format!("{}", DiffStatus::Added), "A");
        assert_eq!(format!("{}", DiffStatus::Modified), "M");
        assert_eq!(format!("{}", DiffStatus::Deleted), "D");
    }
}
