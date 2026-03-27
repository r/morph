//! Garbage collection: remove unreachable objects from the store.
//!
//! Walks all refs to build a reachability set, then deletes any object
//! not in that set. Also cleans type-index directories (runs/, traces/,
//! prompts/, evals/).

use crate::hash::Hash;
use crate::store::{FsStore, MorphError, Store};
use crate::sync::{collect_reachable_objects, list_refs};
use std::collections::HashSet;
use std::path::Path;

/// Summary returned after a GC pass.
#[derive(Debug)]
pub struct GcResult {
    pub objects_before: usize,
    pub objects_after: usize,
    pub objects_removed: usize,
    pub bytes_freed: u64,
}

/// Run garbage collection on a Morph repository.
///
/// 1. Collects every object hash reachable from any ref (branches, tags, remotes).
/// 2. Walks the objects directory and deletes unreachable objects.
/// 3. Cleans type-index directories of unreachable entries.
pub fn gc(store: &FsStore, morph_dir: &Path) -> Result<GcResult, MorphError> {
    let reachable = collect_all_reachable(store)?;

    let all_hashes = store.all_object_hashes()?;
    let objects_before = all_hashes.len();
    let mut objects_removed = 0usize;
    let mut bytes_freed = 0u64;

    for hash in &all_hashes {
        if reachable.contains(hash) {
            continue;
        }
        let path = store.objects_dir().join(object_relative_path(store, hash));
        if path.exists() {
            bytes_freed += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            std::fs::remove_file(&path)?;
            objects_removed += 1;
        }
    }

    for index_dir_name in &["runs", "traces", "prompts", "evals"] {
        let dir = morph_dir.join(index_dir_name);
        if !dir.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) if s.len() == 64 => s,
                _ => continue,
            };
            if let Ok(hash) = Hash::from_hex(stem) {
                if !reachable.contains(&hash) {
                    bytes_freed += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    std::fs::remove_file(&path)?;
                }
            }
        }
    }

    // Clean up empty fan-out prefix directories.
    let objects_dir = store.objects_dir();
    if objects_dir.is_dir() {
        for entry in std::fs::read_dir(&objects_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let sub = entry.path();
                if std::fs::read_dir(&sub)?.next().is_none() {
                    let _ = std::fs::remove_dir(&sub);
                }
            }
        }
    }

    Ok(GcResult {
        objects_before,
        objects_after: objects_before - objects_removed,
        objects_removed,
        bytes_freed,
    })
}

/// Collect all object hashes reachable from every ref in the store.
fn collect_all_reachable(store: &dyn Store) -> Result<HashSet<Hash>, MorphError> {
    let mut reachable = HashSet::new();
    let refs = list_refs(store)?;

    for (_, tip) in &refs {
        let objs = collect_reachable_objects(store, tip, &|_| Ok(false))?;
        reachable.extend(objs);
    }

    Ok(reachable)
}

fn object_relative_path(store: &FsStore, hash: &Hash) -> String {
    let hex = hash.to_string();
    match store.layout() {
        crate::store::ObjectLayout::Flat => format!("{}.json", hex),
        crate::store::ObjectLayout::Fanout => {
            let (prefix, rest) = hex.split_at(2);
            format!("{}/{}.json", prefix, rest)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::*;
    use crate::repo::init_repo;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn setup_repo() -> (tempfile::TempDir, FsStore) {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        let store = FsStore::new(&morph_dir);
        (dir, store)
    }

    fn make_commit(store: &FsStore, root: &std::path::Path, msg: &str) -> Hash {
        std::fs::write(root.join(format!("{}.txt", msg.replace(' ', "_"))), msg).unwrap();
        crate::add_paths(store, root, &[PathBuf::from(".")]).unwrap();
        crate::create_tree_commit(
            store, root, None, None,
            BTreeMap::new(), msg.to_string(), None, Some("0.3"),
        ).unwrap()
    }

    #[test]
    fn gc_removes_unreachable_objects() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        let morph_dir = root.join(".morph");

        make_commit(&store, root, "keep");

        let orphan = MorphObject::Blob(Blob {
            kind: "orphan".into(),
            content: serde_json::json!({"garbage": true}),
        });
        let orphan_hash = store.put(&orphan).unwrap();
        assert!(store.has(&orphan_hash).unwrap());

        let result = gc(&store, &morph_dir).unwrap();
        assert!(result.objects_removed >= 1);
        assert!(!store.has(&orphan_hash).unwrap(), "orphan should be deleted");
    }

    #[test]
    fn gc_preserves_reachable_objects() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        let morph_dir = root.join(".morph");

        let commit_hash = make_commit(&store, root, "keep-this");

        let before = store.all_object_hashes().unwrap().len();
        let result = gc(&store, &morph_dir).unwrap();
        assert_eq!(result.objects_removed, 0, "no objects should be removed");
        assert_eq!(result.objects_before, before);
        assert!(store.has(&commit_hash).unwrap());
    }

    #[test]
    fn gc_cleans_type_index_dirs() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        let morph_dir = root.join(".morph");

        make_commit(&store, root, "anchor");

        let orphan_run = MorphObject::Run(Run {
            pipeline: "0".repeat(64),
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
            trace: "0".repeat(64),
            agent: AgentInfo { id: "a".into(), version: "1".into(), policy: None, instance_id: None },
            contributors: None,
            morph_version: None,
        });
        let run_hash = store.put(&orphan_run).unwrap();

        let run_index = morph_dir.join("runs").join(format!("{}.json", run_hash));
        assert!(run_index.exists(), "type index entry should exist");

        gc(&store, &morph_dir).unwrap();
        assert!(!run_index.exists(), "orphan type index entry should be cleaned");
    }
}
