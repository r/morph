//! Tree building and restoration.
//!
//! Builds recursive Tree objects from a flat staging index (path -> blob hash),
//! and restores a working directory from a stored tree.

use crate::objects::{MorphObject, Tree, TreeEntry};
use crate::store::{MorphError, Store};
use crate::Hash;
use std::collections::BTreeMap;
use std::path::Path;

/// Build a Tree hierarchy from a flat index (relative_path -> blob_hash).
/// Stores all intermediate Tree objects in the store. Returns the root tree hash.
pub fn build_tree(
    store: &dyn Store,
    entries: &BTreeMap<String, String>,
) -> Result<Hash, MorphError> {
    build_tree_recursive(store, entries, "")
}

fn build_tree_recursive(
    store: &dyn Store,
    entries: &BTreeMap<String, String>,
    prefix: &str,
) -> Result<Hash, MorphError> {
    let mut tree_entries: Vec<TreeEntry> = Vec::new();
    let mut subdirs: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();

    for (path, hash) in entries {
        let relative = if prefix.is_empty() {
            path.as_str()
        } else if let Some(rest) = path.strip_prefix(prefix) {
            rest.strip_prefix('/').unwrap_or(rest)
        } else {
            continue;
        };

        if relative.is_empty() {
            continue;
        }

        match relative.split_once('/') {
            None => {
                tree_entries.push(TreeEntry {
                    name: relative.to_string(),
                    hash: hash.clone(),
                    entry_type: "blob".to_string(),
                });
            }
            Some((dir, _rest)) => {
                subdirs
                    .entry(dir.to_string())
                    .or_default()
                    .insert(path.clone(), hash.clone());
            }
        }
    }

    for (dir_name, sub_entries) in &subdirs {
        let sub_prefix = if prefix.is_empty() {
            dir_name.clone()
        } else {
            format!("{}/{}", prefix, dir_name)
        };
        let sub_hash = build_tree_recursive(store, sub_entries, &sub_prefix)?;
        tree_entries.push(TreeEntry {
            name: dir_name.clone(),
            hash: sub_hash.to_string(),
            entry_type: "tree".to_string(),
        });
    }

    tree_entries.sort_by(|a, b| a.name.cmp(&b.name));

    let tree = MorphObject::Tree(Tree {
        entries: tree_entries,
    });
    store.put(&tree)
}

/// Flatten a tree into a map of relative_path -> blob_hash. Inverse of build_tree.
pub fn flatten_tree(
    store: &dyn Store,
    root_hash: &Hash,
) -> Result<BTreeMap<String, String>, MorphError> {
    let mut out = BTreeMap::new();
    flatten_recursive(store, root_hash, "", &mut out)?;
    Ok(out)
}

fn flatten_recursive(
    store: &dyn Store,
    hash: &Hash,
    prefix: &str,
    out: &mut BTreeMap<String, String>,
) -> Result<(), MorphError> {
    let obj = store.get(hash)?;
    let tree = match &obj {
        MorphObject::Tree(t) => t,
        _ => return Err(MorphError::Serialization("expected tree object".into())),
    };
    for entry in &tree.entries {
        let full_path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{}/{}", prefix, entry.name)
        };
        match entry.entry_type.as_str() {
            "tree" => {
                let sub_hash = Hash::from_hex(&entry.hash)?;
                flatten_recursive(store, &sub_hash, &full_path, out)?;
            }
            _ => {
                out.insert(full_path, entry.hash.clone());
            }
        }
    }
    Ok(())
}

/// Restore a tree to a directory: walk the tree, materialize each blob to its path.
pub fn restore_tree(
    store: &dyn Store,
    root_hash: &Hash,
    dest_dir: &Path,
) -> Result<(), MorphError> {
    restore_tree_filtered(store, root_hash, dest_dir, None)
}

/// Restore a tree, skipping entries that match the ignore rules.
/// Protects against old commits that contain `.git/`, `.venv/`, etc.
pub fn restore_tree_filtered(
    store: &dyn Store,
    root_hash: &Hash,
    dest_dir: &Path,
    ignore_matcher: Option<&ignore::gitignore::Gitignore>,
) -> Result<(), MorphError> {
    let flat = flatten_tree(store, root_hash)?;
    for (rel_path, blob_hash) in &flat {
        if crate::morphignore::is_rel_path_ignored(ignore_matcher, rel_path, false) {
            continue;
        }
        let hash = Hash::from_hex(blob_hash)?;
        let dest = dest_dir.join(rel_path);
        crate::working::materialize_blob(store, &hash, &dest)?;
    }
    Ok(())
}

/// Compute and return the hash of an empty tree (zero entries).
pub fn empty_tree_hash(store: &dyn Store) -> Result<Hash, MorphError> {
    let tree = MorphObject::Tree(Tree {
        entries: vec![],
    });
    store.put(&tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{Blob, MorphObject};
    use crate::store::FsStore;

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
    fn build_tree_empty_index() {
        let (_dir, store) = make_store();
        let entries = BTreeMap::new();
        let hash = build_tree(&store, &entries).unwrap();
        let obj = store.get(&hash).unwrap();
        let tree = match &obj {
            MorphObject::Tree(t) => t,
            _ => panic!("expected tree"),
        };
        assert!(tree.entries.is_empty());
    }

    #[test]
    fn build_tree_single_file() {
        let (_dir, store) = make_store();
        let blob_hash = store_blob(&store, "hello");
        let mut entries = BTreeMap::new();
        entries.insert("README.md".into(), blob_hash.to_string());

        let hash = build_tree(&store, &entries).unwrap();
        let obj = store.get(&hash).unwrap();
        let tree = match &obj {
            MorphObject::Tree(t) => t,
            _ => panic!("expected tree"),
        };
        assert_eq!(tree.entries.len(), 1);
        assert_eq!(tree.entries[0].name, "README.md");
        assert_eq!(tree.entries[0].entry_type, "blob");
        assert_eq!(tree.entries[0].hash, blob_hash.to_string());
    }

    #[test]
    fn build_tree_nested_dirs() {
        let (_dir, store) = make_store();
        let h1 = store_blob(&store, "main");
        let h2 = store_blob(&store, "lib");
        let h3 = store_blob(&store, "readme");

        let mut entries = BTreeMap::new();
        entries.insert("src/main.rs".into(), h1.to_string());
        entries.insert("src/lib.rs".into(), h2.to_string());
        entries.insert("README.md".into(), h3.to_string());

        let hash = build_tree(&store, &entries).unwrap();
        let obj = store.get(&hash).unwrap();
        let root = match &obj {
            MorphObject::Tree(t) => t,
            _ => panic!("expected tree"),
        };
        assert_eq!(root.entries.len(), 2);
        assert_eq!(root.entries[0].name, "README.md");
        assert_eq!(root.entries[0].entry_type, "blob");
        assert_eq!(root.entries[1].name, "src");
        assert_eq!(root.entries[1].entry_type, "tree");

        let src_hash = Hash::from_hex(&root.entries[1].hash).unwrap();
        let src_obj = store.get(&src_hash).unwrap();
        let src_tree = match &src_obj {
            MorphObject::Tree(t) => t,
            _ => panic!("expected tree"),
        };
        assert_eq!(src_tree.entries.len(), 2);
        assert_eq!(src_tree.entries[0].name, "lib.rs");
        assert_eq!(src_tree.entries[1].name, "main.rs");
    }

    #[test]
    fn flatten_tree_roundtrip() {
        let (_dir, store) = make_store();
        let h1 = store_blob(&store, "aaa");
        let h2 = store_blob(&store, "bbb");
        let h3 = store_blob(&store, "ccc");

        let mut entries = BTreeMap::new();
        entries.insert("a.txt".into(), h1.to_string());
        entries.insert("dir/b.txt".into(), h2.to_string());
        entries.insert("dir/sub/c.txt".into(), h3.to_string());

        let root_hash = build_tree(&store, &entries).unwrap();
        let flat = flatten_tree(&store, &root_hash).unwrap();
        assert_eq!(flat, entries);
    }

    #[test]
    fn restore_tree_writes_files() {
        let (dir, store) = make_store();
        let h1 = store_blob(&store, "content_a");
        let h2 = store_blob(&store, "content_b");

        let mut entries = BTreeMap::new();
        entries.insert("file.txt".into(), h1.to_string());
        entries.insert("sub/nested.txt".into(), h2.to_string());

        let root_hash = build_tree(&store, &entries).unwrap();

        let dest = dir.path().join("restored");
        std::fs::create_dir_all(&dest).unwrap();
        restore_tree(&store, &root_hash, &dest).unwrap();

        assert_eq!(
            std::fs::read_to_string(dest.join("file.txt")).unwrap(),
            "content_a"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("sub/nested.txt")).unwrap(),
            "content_b"
        );
    }

    #[test]
    fn empty_tree_hash_deterministic() {
        let (_dir, store) = make_store();
        let h1 = empty_tree_hash(&store).unwrap();
        let h2 = empty_tree_hash(&store).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn build_tree_deeply_nested() {
        let (_dir, store) = make_store();
        let h = store_blob(&store, "deep");
        let mut entries = BTreeMap::new();
        entries.insert("a/b/c/d/e.txt".into(), h.to_string());

        let root = build_tree(&store, &entries).unwrap();
        let flat = flatten_tree(&store, &root).unwrap();
        assert_eq!(flat.len(), 1);
        assert_eq!(flat["a/b/c/d/e.txt"], h.to_string());
    }

    #[test]
    fn restore_tree_filtered_skips_ignored_entries() {
        let (dir, store) = make_store();
        let h_good = store_blob(&store, "good_content");
        let h_git = store_blob(&store, "git_config");
        let h_venv = store_blob(&store, "venv_python");

        let mut entries = BTreeMap::new();
        entries.insert("app.py".into(), h_good.to_string());
        entries.insert(".git/config".into(), h_git.to_string());
        entries.insert(".venv/bin/python".into(), h_venv.to_string());

        let root_hash = build_tree(&store, &entries).unwrap();

        let dest = dir.path().join("restored");
        std::fs::create_dir_all(&dest).unwrap();

        let matcher = crate::morphignore::load_ignore_rules(&dest).unwrap();
        restore_tree_filtered(&store, &root_hash, &dest, Some(&matcher)).unwrap();

        assert!(dest.join("app.py").exists(), "app.py should be restored");
        assert!(!dest.join(".git/config").exists(), ".git/config should NOT be restored");
        assert!(!dest.join(".venv/bin/python").exists(), ".venv/bin/python should NOT be restored");
    }
}
