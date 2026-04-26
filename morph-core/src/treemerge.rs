//! 3-way structural merge of `Tree` objects (multi-machine plan, PR 3).
//!
//! Implementation strategy: rather than walking nested trees recursively,
//! we [`flatten_tree`] each side into a flat `path → blob_hash` map and run
//! the standard 3-way reconciliation per path. Re-nesting is handled
//! by [`build_tree`], which already exists and is well-tested.
//!
//! Per-path resolution rules:
//!
//! | base | ours | theirs | resolution                                   |
//! |------|------|--------|----------------------------------------------|
//! | —    | —    | —      | unreachable                                  |
//! | x    | —    | —      | deleted on both sides → drop, plan a delete  |
//! | —    | x    | —      | added by us → take, plan a write             |
//! | —    | —    | x      | added by them → take, plan a write           |
//! | —    | x    | y      | both added: equal=take; else text-merge no-base |
//! | x    | y    | —      | theirs deleted: ours unchanged=delete; else conflict |
//! | x    | —    | y      | symmetric of above                           |
//! | x    | x    | x      | unchanged → keep, no working write           |
//! | x    | y    | x      | only ours modified → take ours               |
//! | x    | x    | y      | only theirs modified → take theirs, plan write |
//! | x    | y    | z      | both modified: y==z take; else 3-way text merge |
//!
//! Textual leaf conflicts are surfaced as [`ObjConflict::Textual`]. Modify/
//! delete conflicts are surfaced as [`ObjConflict::Structural`] with kind
//! `TreeDivergent`. The merge engine itself never writes to the working
//! tree — it returns a plan in [`TreeMergeOutcome::working_writes`] so the
//! CLI orchestrator can apply it after dominance gating.

use crate::objects::{Blob, MorphObject};
use crate::objmerge::{ObjConflict, StructuralKind};
use crate::store::{MorphError, Store};
use crate::text3way::{merge_text, TextMergeLabels, TextMergeResult};
use crate::tree::{build_tree, flatten_tree};
use crate::Hash;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Working-tree write planned by the tree merger. The CLI orchestrator
/// applies these operations after dominance gating in PR 4.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkdirOp {
    /// Write `bytes` to `path` (creating parent directories as needed).
    Write { path: PathBuf, bytes: Vec<u8> },
    /// Delete `path` from the working tree.
    Delete { path: PathBuf },
}

/// Outcome of a 3-way tree merge.
///
/// `merged_tree` is set when the merge produced a clean result. With
/// conflicts present, callers should not auto-write the merged tree —
/// they must guide the user through resolution first.
#[derive(Clone, Debug)]
pub struct TreeMergeOutcome {
    pub merged_tree: Option<Hash>,
    pub conflicts: Vec<ObjConflict>,
    pub working_writes: Vec<WorkdirOp>,
}

/// 3-way merge two tree hashes against an optional common base.
pub fn merge_trees(
    store: &dyn Store,
    base: Option<&Hash>,
    ours: &Hash,
    theirs: &Hash,
) -> Result<TreeMergeOutcome, MorphError> {
    let base_map: BTreeMap<String, String> = match base {
        Some(b) => flatten_tree(store, b)?,
        None => BTreeMap::new(),
    };
    let ours_map = flatten_tree(store, ours)?;
    let theirs_map = flatten_tree(store, theirs)?;

    let all_paths: BTreeSet<String> = base_map
        .keys()
        .chain(ours_map.keys())
        .chain(theirs_map.keys())
        .cloned()
        .collect();

    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    let mut conflicts: Vec<ObjConflict> = Vec::new();
    let mut writes: Vec<WorkdirOp> = Vec::new();

    for path in &all_paths {
        let b = base_map.get(path);
        let o = ours_map.get(path);
        let t = theirs_map.get(path);
        match (b, o, t) {
            (None, None, None) => unreachable!(),

            (Some(_), None, None) => {
                writes.push(WorkdirOp::Delete { path: PathBuf::from(path) });
            }
            (None, Some(h), None) | (None, None, Some(h)) => {
                let bytes = read_blob_bytes(store, h)?;
                merged.insert(path.clone(), h.clone());
                writes.push(WorkdirOp::Write { path: PathBuf::from(path), bytes });
            }

            (None, Some(o_h), Some(t_h)) => {
                if o_h == t_h {
                    let bytes = read_blob_bytes(store, o_h)?;
                    merged.insert(path.clone(), o_h.clone());
                    writes.push(WorkdirOp::Write { path: PathBuf::from(path), bytes });
                } else {
                    let o_bytes = read_blob_bytes(store, o_h)?;
                    let t_bytes = read_blob_bytes(store, t_h)?;
                    match merge_text(None, &o_bytes, &t_bytes, labels_for(path))? {
                        TextMergeResult::Clean(bytes) => {
                            let new_h = put_blob_bytes(store, &bytes)?;
                            merged.insert(path.clone(), new_h.to_string());
                            writes.push(WorkdirOp::Write { path: PathBuf::from(path), bytes });
                        }
                        TextMergeResult::Conflict { content_with_markers } => {
                            conflicts.push(ObjConflict::Textual {
                                path: PathBuf::from(path),
                                base: None,
                                ours: Some(Hash::from_hex(o_h)?),
                                theirs: Some(Hash::from_hex(t_h)?),
                            });
                            writes.push(WorkdirOp::Write {
                                path: PathBuf::from(path),
                                bytes: content_with_markers,
                            });
                        }
                    }
                }
            }

            (Some(b_h), Some(o_h), None) => {
                if b_h == o_h {
                    writes.push(WorkdirOp::Delete { path: PathBuf::from(path) });
                } else {
                    conflicts.push(ObjConflict::Structural {
                        kind: StructuralKind::TreeDivergent,
                        message: format!("modify/delete: {}", path),
                    });
                    let bytes = read_blob_bytes(store, o_h)?;
                    merged.insert(path.clone(), o_h.clone());
                    writes.push(WorkdirOp::Write { path: PathBuf::from(path), bytes });
                }
            }
            (Some(b_h), None, Some(t_h)) => {
                if b_h == t_h {
                    writes.push(WorkdirOp::Delete { path: PathBuf::from(path) });
                } else {
                    conflicts.push(ObjConflict::Structural {
                        kind: StructuralKind::TreeDivergent,
                        message: format!("modify/delete: {}", path),
                    });
                    let bytes = read_blob_bytes(store, t_h)?;
                    merged.insert(path.clone(), t_h.clone());
                    writes.push(WorkdirOp::Write { path: PathBuf::from(path), bytes });
                }
            }

            (Some(b_h), Some(o_h), Some(t_h)) => {
                if o_h == t_h {
                    merged.insert(path.clone(), o_h.clone());
                } else if b_h == o_h {
                    let bytes = read_blob_bytes(store, t_h)?;
                    merged.insert(path.clone(), t_h.clone());
                    writes.push(WorkdirOp::Write { path: PathBuf::from(path), bytes });
                } else if b_h == t_h {
                    merged.insert(path.clone(), o_h.clone());
                } else {
                    let b_bytes = read_blob_bytes(store, b_h)?;
                    let o_bytes = read_blob_bytes(store, o_h)?;
                    let t_bytes = read_blob_bytes(store, t_h)?;
                    match merge_text(Some(&b_bytes), &o_bytes, &t_bytes, labels_for(path))? {
                        TextMergeResult::Clean(bytes) => {
                            let new_h = put_blob_bytes(store, &bytes)?;
                            merged.insert(path.clone(), new_h.to_string());
                            writes.push(WorkdirOp::Write { path: PathBuf::from(path), bytes });
                        }
                        TextMergeResult::Conflict { content_with_markers } => {
                            conflicts.push(ObjConflict::Textual {
                                path: PathBuf::from(path),
                                base: Some(Hash::from_hex(b_h)?),
                                ours: Some(Hash::from_hex(o_h)?),
                                theirs: Some(Hash::from_hex(t_h)?),
                            });
                            writes.push(WorkdirOp::Write {
                                path: PathBuf::from(path),
                                bytes: content_with_markers,
                            });
                        }
                    }
                }
            }
        }
    }

    // Always build the merged tree — even when conflicts exist callers
    // benefit from a "current best preview" hash. None only when we
    // cannot construct a meaningful tree (e.g. all sides empty + nothing
    // produced). build_tree handles the empty case.
    let merged_tree = Some(build_tree(store, &merged)?);

    Ok(TreeMergeOutcome { merged_tree, conflicts, working_writes: writes })
}

fn read_blob_bytes(store: &dyn Store, hash_hex: &str) -> Result<Vec<u8>, MorphError> {
    let h = Hash::from_hex(hash_hex).map_err(|_| MorphError::InvalidHash(hash_hex.to_string()))?;
    match store.get(&h)? {
        MorphObject::Blob(b) => extract_blob_bytes(&b),
        _ => Err(MorphError::Serialization(format!(
            "expected Blob at {}",
            hash_hex
        ))),
    }
}

fn extract_blob_bytes(blob: &Blob) -> Result<Vec<u8>, MorphError> {
    let body = blob
        .content
        .get("body")
        .and_then(|v| v.as_str())
        .ok_or_else(|| MorphError::Serialization("blob missing body".to_string()))?;
    if blob.content.get("encoding").and_then(|v| v.as_str()) == Some("base64") {
        BASE64
            .decode(body.as_bytes())
            .map_err(|e| MorphError::Serialization(format!("invalid base64: {}", e)))
    } else {
        Ok(body.as_bytes().to_vec())
    }
}

fn put_blob_bytes(store: &dyn Store, bytes: &[u8]) -> Result<Hash, MorphError> {
    let content = match std::str::from_utf8(bytes) {
        Ok(s) => serde_json::json!({ "body": s }),
        Err(_) => serde_json::json!({ "body": BASE64.encode(bytes), "encoding": "base64" }),
    };
    store.put(&MorphObject::Blob(Blob {
        kind: "blob".into(),
        content,
    }))
}

fn labels_for(path: &str) -> TextMergeLabels {
    TextMergeLabels {
        base: format!("base:{}", path),
        ours: format!("HEAD:{}", path),
        theirs: format!("MERGE_HEAD:{}", path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{MorphObject, Tree, TreeEntry};
    use crate::repo::init_repo;
    use crate::store::FsStore;

    fn setup_repo() -> (tempfile::TempDir, FsStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = init_repo(dir.path()).unwrap();
        (dir, store)
    }

    fn put_text_blob(store: &dyn Store, content: &str) -> String {
        put_blob_bytes(store, content.as_bytes()).unwrap().to_string()
    }

    /// Build a tree from a flat path→content map, storing each blob and
    /// returning the root hash.
    fn put_tree(store: &dyn Store, files: &[(&str, &str)]) -> Hash {
        let mut entries: BTreeMap<String, String> = BTreeMap::new();
        for (path, content) in files {
            let h = put_text_blob(store, content);
            entries.insert((*path).to_string(), h);
        }
        build_tree(store, &entries).unwrap()
    }

    fn empty_tree(store: &dyn Store) -> Hash {
        store
            .put(&MorphObject::Tree(Tree { entries: vec![] }))
            .unwrap()
    }

    fn write_op_path(op: &WorkdirOp) -> &PathBuf {
        match op {
            WorkdirOp::Write { path, .. } => path,
            WorkdirOp::Delete { path } => path,
        }
    }

    fn find_write<'a>(ops: &'a [WorkdirOp], path: &str) -> Option<&'a [u8]> {
        ops.iter().find_map(|op| match op {
            WorkdirOp::Write { path: p, bytes } if p == &PathBuf::from(path) => Some(bytes.as_slice()),
            _ => None,
        })
    }

    fn has_delete(ops: &[WorkdirOp], path: &str) -> bool {
        ops.iter()
            .any(|op| matches!(op, WorkdirOp::Delete { path: p } if p == &PathBuf::from(path)))
    }

    // ── trivial cases ─────────────────────────────────────────────────

    #[test]
    fn merge_trees_no_changes_returns_same_hash() {
        let (_dir, store) = setup_repo();
        let h = put_tree(&store, &[("a.txt", "hello\n")]);
        let outcome = merge_trees(&store, Some(&h), &h, &h).unwrap();
        assert!(outcome.conflicts.is_empty());
        assert_eq!(outcome.merged_tree, Some(h));
        assert!(
            outcome.working_writes.is_empty(),
            "no changes should produce no working writes; got {:?}",
            outcome.working_writes.iter().map(write_op_path).collect::<Vec<_>>()
        );
    }

    // ── add ───────────────────────────────────────────────────────────

    #[test]
    fn merge_trees_one_side_added() {
        let (_dir, store) = setup_repo();
        let base = empty_tree(&store);
        let ours = put_tree(&store, &[("a.txt", "hello\n")]);
        let theirs = base;
        let outcome = merge_trees(&store, Some(&base), &ours, &theirs).unwrap();
        assert!(outcome.conflicts.is_empty(), "{:?}", outcome.conflicts);
        assert_eq!(find_write(&outcome.working_writes, "a.txt"), Some(b"hello\n".as_slice()));
        let flat = flatten_tree(&store, outcome.merged_tree.as_ref().unwrap()).unwrap();
        assert!(flat.contains_key("a.txt"));
    }

    #[test]
    fn merge_trees_both_sides_added_same_content() {
        let (_dir, store) = setup_repo();
        let base = empty_tree(&store);
        let t = put_tree(&store, &[("a.txt", "hello\n")]);
        let outcome = merge_trees(&store, Some(&base), &t, &t).unwrap();
        assert!(outcome.conflicts.is_empty(), "{:?}", outcome.conflicts);
        assert_eq!(find_write(&outcome.working_writes, "a.txt"), Some(b"hello\n".as_slice()));
    }

    #[test]
    fn merge_trees_both_sides_added_diff_content_text_merges() {
        let (_dir, store) = setup_repo();
        let base = empty_tree(&store);
        let ours = put_tree(&store, &[("a.txt", "line1\nOURS\nline3\n")]);
        let theirs = put_tree(&store, &[("a.txt", "line1\nline2\nTHEIRS\n")]);
        let outcome = merge_trees(&store, Some(&base), &ours, &theirs).unwrap();
        // git merge-file with no base will see disjoint inserts; whether
        // it merges clean or conflicts depends on the algorithm. We don't
        // hard-assert clean; we only assert that any textual conflict is
        // reported via Textual + an explicit working write so the user
        // can resolve.
        let bytes = find_write(&outcome.working_writes, "a.txt").expect("must plan a write");
        if outcome.conflicts.is_empty() {
            assert!(
                String::from_utf8_lossy(bytes).contains("line1"),
                "clean merge must contain shared content"
            );
        } else {
            assert!(
                outcome
                    .conflicts
                    .iter()
                    .any(|c| matches!(c, ObjConflict::Textual { .. })),
                "expected Textual conflict, got {:?}",
                outcome.conflicts
            );
        }
    }

    #[test]
    fn merge_trees_both_sides_added_diff_content_text_conflicts() {
        let (_dir, store) = setup_repo();
        // Both add a.txt with content that has nothing in common -> conflict.
        let base = empty_tree(&store);
        let ours = put_tree(&store, &[("a.txt", "OURS_ONLY\n")]);
        let theirs = put_tree(&store, &[("a.txt", "THEIRS_ONLY\n")]);
        let outcome = merge_trees(&store, Some(&base), &ours, &theirs).unwrap();
        assert!(
            outcome.conflicts.iter().any(|c| matches!(c, ObjConflict::Textual { .. })),
            "expected Textual conflict, got {:?}",
            outcome.conflicts
        );
        let bytes = find_write(&outcome.working_writes, "a.txt").expect("conflict should plan a write");
        let s = String::from_utf8_lossy(bytes);
        assert!(s.contains("<<<<<<<"), "expected conflict markers in working write, got:\n{}", s);
    }

    // ── modify ────────────────────────────────────────────────────────

    #[test]
    fn merge_trees_one_side_modified_other_unchanged() {
        let (_dir, store) = setup_repo();
        let base = put_tree(&store, &[("a.txt", "old\n")]);
        let ours = put_tree(&store, &[("a.txt", "NEW\n")]);
        let theirs = base;
        let outcome = merge_trees(&store, Some(&base), &ours, &theirs).unwrap();
        assert!(outcome.conflicts.is_empty(), "{:?}", outcome.conflicts);
        let flat = flatten_tree(&store, outcome.merged_tree.as_ref().unwrap()).unwrap();
        let blob_hex = &flat["a.txt"];
        let bytes = read_blob_bytes(&store, blob_hex).unwrap();
        assert_eq!(bytes, b"NEW\n");
    }

    #[test]
    fn merge_trees_both_sides_modified_same_way() {
        let (_dir, store) = setup_repo();
        let base = put_tree(&store, &[("a.txt", "old\n")]);
        let modified = put_tree(&store, &[("a.txt", "NEW\n")]);
        let outcome = merge_trees(&store, Some(&base), &modified, &modified).unwrap();
        assert!(outcome.conflicts.is_empty(), "{:?}", outcome.conflicts);
        let flat = flatten_tree(&store, outcome.merged_tree.as_ref().unwrap()).unwrap();
        let bytes = read_blob_bytes(&store, &flat["a.txt"]).unwrap();
        assert_eq!(bytes, b"NEW\n");
    }

    #[test]
    fn merge_trees_both_sides_modified_clean_text_merge() {
        let (_dir, store) = setup_repo();
        let base = put_tree(&store, &[("a.txt", "line1\nline2\nline3\n")]);
        let ours = put_tree(&store, &[("a.txt", "OURS1\nline2\nline3\n")]);
        let theirs = put_tree(&store, &[("a.txt", "line1\nline2\nTHEIRS3\n")]);
        let outcome = merge_trees(&store, Some(&base), &ours, &theirs).unwrap();
        assert!(outcome.conflicts.is_empty(), "got: {:?}", outcome.conflicts);
        let flat = flatten_tree(&store, outcome.merged_tree.as_ref().unwrap()).unwrap();
        let bytes = read_blob_bytes(&store, &flat["a.txt"]).unwrap();
        assert_eq!(bytes, b"OURS1\nline2\nTHEIRS3\n");
    }

    #[test]
    fn merge_trees_both_sides_modified_text_conflict() {
        let (_dir, store) = setup_repo();
        let base = put_tree(&store, &[("a.txt", "line1\nline2\nline3\n")]);
        let ours = put_tree(&store, &[("a.txt", "line1\nOURS_LINE2\nline3\n")]);
        let theirs = put_tree(&store, &[("a.txt", "line1\nTHEIRS_LINE2\nline3\n")]);
        let outcome = merge_trees(&store, Some(&base), &ours, &theirs).unwrap();
        assert!(
            outcome.conflicts.iter().any(|c| matches!(c, ObjConflict::Textual { path, .. } if path == &PathBuf::from("a.txt"))),
            "expected Textual conflict at a.txt, got {:?}",
            outcome.conflicts
        );
        let bytes = find_write(&outcome.working_writes, "a.txt").unwrap();
        let s = String::from_utf8_lossy(bytes);
        assert!(s.contains("<<<<<<<"), "expected markers, got:\n{}", s);
        assert!(s.contains("OURS_LINE2"), "ours content missing");
        assert!(s.contains("THEIRS_LINE2"), "theirs content missing");
    }

    // ── delete ────────────────────────────────────────────────────────

    #[test]
    fn merge_trees_one_side_deleted_other_unchanged() {
        let (_dir, store) = setup_repo();
        let base = put_tree(&store, &[("a.txt", "x\n"), ("b.txt", "y\n")]);
        let ours = put_tree(&store, &[("b.txt", "y\n")]); // a.txt removed
        let theirs = base;
        let outcome = merge_trees(&store, Some(&base), &ours, &theirs).unwrap();
        assert!(outcome.conflicts.is_empty(), "{:?}", outcome.conflicts);
        let flat = flatten_tree(&store, outcome.merged_tree.as_ref().unwrap()).unwrap();
        assert!(!flat.contains_key("a.txt"), "a.txt must not survive merge");
        assert!(flat.contains_key("b.txt"));
        assert!(has_delete(&outcome.working_writes, "a.txt"));
    }

    #[test]
    fn merge_trees_modify_delete_conflicts() {
        let (_dir, store) = setup_repo();
        let base = put_tree(&store, &[("a.txt", "x\n")]);
        let ours = put_tree(&store, &[("a.txt", "MODIFIED\n")]);
        let theirs = empty_tree(&store);
        let outcome = merge_trees(&store, Some(&base), &ours, &theirs).unwrap();
        let structural_count = outcome
            .conflicts
            .iter()
            .filter(|c| matches!(c, ObjConflict::Structural { kind: StructuralKind::TreeDivergent, message } if message.contains("modify/delete: a.txt")))
            .count();
        assert_eq!(
            structural_count, 1,
            "expected one TreeDivergent modify/delete on a.txt, got: {:?}",
            outcome.conflicts
        );
        // Preview keeps the modified side so the user can see it on disk.
        let bytes = find_write(&outcome.working_writes, "a.txt").unwrap();
        assert_eq!(bytes, b"MODIFIED\n");
    }

    // Suppress dead_code for an unused TreeEntry import that may surface
    // if the test layout is expanded.
    #[allow(dead_code)]
    fn _silence(_: TreeEntry) {}
}
