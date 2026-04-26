//! Remote sync: push, fetch, pull, remote config.
//!
//! Phase 5: local-path transport. The architecture separates object transfer
//! logic from transport so network transport can be added later.
//!
//! ## Object closure rule
//!
//! The reachable closure from a commit includes:
//! - The commit itself
//! - commit.tree and all tree entries recursively (blobs, sub-trees)
//! - commit.pipeline → Pipeline, plus its prompts, eval_suite, provenance refs
//! - commit.eval_contract.suite → EvalSuite
//! - commit.evidence_refs → Runs, Traces
//!   - For Runs: run.trace → Trace, run.pipeline, run.output_artifacts
//! - commit.parents → recursively (stopping at objects the destination already has)
//!
//! ## Ref model
//!
//! - Local branches: refs/heads/<branch>
//! - Remote-tracking refs: refs/remotes/<remote>/<branch>
//! - Remote repos use standard refs/heads/<branch> layout

use crate::objects::MorphObject;
use crate::store::{MorphError, Store};
use crate::Hash;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::Path;

/// A named remote: just a filesystem path for Phase 5.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RemoteSpec {
    pub path: String,
}

// ── Remote config ────────────────────────────────────────────────────

/// Read configured remotes from config.json.
pub fn read_remotes(morph_dir: &Path) -> Result<BTreeMap<String, RemoteSpec>, MorphError> {
    let config_path = morph_dir.join("config.json");
    if !config_path.exists() {
        return Ok(BTreeMap::new());
    }
    let data = std::fs::read_to_string(&config_path)?;
    let config: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| MorphError::Serialization(e.to_string()))?;
    match config.get("remotes") {
        Some(remotes) => serde_json::from_value(remotes.clone())
            .map_err(|e| MorphError::Serialization(e.to_string())),
        None => Ok(BTreeMap::new()),
    }
}

/// Write remotes into config.json, preserving other keys.
pub fn write_remotes(
    morph_dir: &Path,
    remotes: &BTreeMap<String, RemoteSpec>,
) -> Result<(), MorphError> {
    let config_path = morph_dir.join("config.json");
    let mut config: serde_json::Value = if config_path.exists() {
        let data = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&data).map_err(|e| MorphError::Serialization(e.to_string()))?
    } else {
        serde_json::json!({})
    };
    config["remotes"] = serde_json::to_value(remotes)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    let pretty = serde_json::to_string_pretty(&config)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    std::fs::write(&config_path, pretty)?;
    Ok(())
}

/// Add a named remote to the repo config.
pub fn add_remote(morph_dir: &Path, name: &str, path: &str) -> Result<(), MorphError> {
    let mut remotes = read_remotes(morph_dir)?;
    remotes.insert(name.to_string(), RemoteSpec { path: path.to_string() });
    write_remotes(morph_dir, &remotes)
}

// ── Object graph traversal ───────────────────────────────────────────

/// Collect all object hashes reachable from `tip` that the destination lacks.
///
/// Walks commits, trees, pipelines, eval suites, runs, traces, and artifacts.
/// Stops graph traversal at objects the destination already has.
pub fn collect_reachable_objects(
    source: &dyn Store,
    tip: &Hash,
    dest_has: &dyn Fn(&Hash) -> Result<bool, MorphError>,
) -> Result<Vec<Hash>, MorphError> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(*tip);

    while let Some(hash) = queue.pop_front() {
        if !visited.insert(hash) {
            continue;
        }
        if dest_has(&hash)? {
            continue;
        }

        let obj = match source.get(&hash) {
            Ok(o) => o,
            Err(MorphError::NotFound(_)) => continue,
            Err(e) => return Err(e),
        };
        result.push(hash);

        collect_refs(&obj, &mut queue);
    }

    Ok(result)
}

/// Extract outgoing object references from any MorphObject.
fn collect_refs(obj: &MorphObject, queue: &mut VecDeque<Hash>) {
    match obj {
        MorphObject::Commit(c) => {
            for p in &c.parents {
                enqueue(queue, p);
            }
            if let Some(t) = &c.tree {
                enqueue(queue, t);
            }
            enqueue(queue, &c.pipeline);
            enqueue(queue, &c.eval_contract.suite);
            if let Some(refs) = &c.evidence_refs {
                for r in refs {
                    enqueue(queue, r);
                }
            }
        }
        MorphObject::Tree(t) => {
            for e in &t.entries {
                enqueue(queue, &e.hash);
            }
        }
        MorphObject::Pipeline(p) => {
            for h in &p.prompts {
                enqueue(queue, h);
            }
            if let Some(s) = &p.eval_suite {
                enqueue(queue, s);
            }
            if let Some(prov) = &p.provenance {
                if let Some(r) = &prov.derived_from_run {
                    enqueue(queue, r);
                }
                if let Some(t) = &prov.derived_from_trace {
                    enqueue(queue, t);
                }
            }
        }
        MorphObject::Run(r) => {
            enqueue(queue, &r.trace);
            enqueue(queue, &r.pipeline);
            for a in &r.output_artifacts {
                enqueue(queue, a);
            }
        }
        MorphObject::Blob(_)
        | MorphObject::EvalSuite(_)
        | MorphObject::Trace(_)
        | MorphObject::Artifact(_)
        | MorphObject::TraceRollup(_)
        | MorphObject::Annotation(_) => {}
    }
}

fn enqueue(queue: &mut VecDeque<Hash>, hex: &str) {
    if let Ok(h) = Hash::from_hex(hex) {
        queue.push_back(h);
    }
}

// ── Ancestry check ───────────────────────────────────────────────────

/// Check whether `ancestor` is reachable from `descendant` via parent links.
pub fn is_ancestor(
    store: &dyn Store,
    ancestor: &Hash,
    descendant: &Hash,
) -> Result<bool, MorphError> {
    if ancestor == descendant {
        return Ok(true);
    }
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(*descendant);

    while let Some(hash) = queue.pop_front() {
        if hash == *ancestor {
            return Ok(true);
        }
        if !visited.insert(hash) {
            continue;
        }
        if let Ok(MorphObject::Commit(c)) = store.get(&hash) {
            for p in &c.parents {
                if let Ok(h) = Hash::from_hex(p) {
                    queue.push_back(h);
                }
            }
        }
    }

    Ok(false)
}

// ── Push ─────────────────────────────────────────────────────────────

/// Push a local branch to a remote. Transfers missing objects, then updates
/// the remote branch ref. Rejects non-fast-forward pushes.
pub fn push_branch(
    local_store: &dyn Store,
    remote_store: &dyn Store,
    branch: &str,
) -> Result<Hash, MorphError> {
    let local_ref = format!("heads/{}", branch);
    let local_tip = local_store
        .ref_read(&local_ref)?
        .ok_or_else(|| MorphError::NotFound(format!("local branch '{}'", branch)))?;

    if let Some(remote_tip) = remote_store.ref_read(&local_ref)? {
        if remote_tip == local_tip {
            return Ok(local_tip);
        }
        if !is_ancestor(local_store, &remote_tip, &local_tip)? {
            return Err(MorphError::Serialization(format!(
                "non-fast-forward: remote branch '{}' at {} is not an ancestor of local tip {}. \
                 Pull and merge before pushing.",
                branch, remote_tip, local_tip
            )));
        }
    }

    transfer_objects(local_store, remote_store, &local_tip)?;
    remote_store.ref_write(&local_ref, &local_tip)?;

    Ok(local_tip)
}

// ── Fetch ────────────────────────────────────────────────────────────

/// Fetch all branches from a remote into local remote-tracking refs.
/// Returns the list of (branch_name, tip_hash) pairs that were updated.
pub fn fetch_remote(
    local_store: &dyn Store,
    remote_store: &dyn Store,
    remote_name: &str,
) -> Result<Vec<(String, Hash)>, MorphError> {
    let remote_heads = remote_store.refs_dir().join("heads");
    let mut updated = Vec::new();

    if !remote_heads.exists() {
        return Ok(updated);
    }

    for entry in std::fs::read_dir(&remote_heads)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let branch = entry.file_name().to_string_lossy().into_owned();
        let ref_path = format!("heads/{}", branch);

        let tip = match remote_store.ref_read(&ref_path)? {
            Some(h) => h,
            None => continue,
        };

        transfer_objects(remote_store, local_store, &tip)?;

        let tracking = format!("remotes/{}/{}", remote_name, branch);
        local_store.ref_write(&tracking, &tip)?;
        updated.push((branch, tip));
    }

    Ok(updated)
}

// ── Pull ─────────────────────────────────────────────────────────────

/// Pull: fetch from remote + fast-forward local branch.
/// Fails if the local branch has diverged (not a fast-forward).
pub fn pull_branch(
    local_store: &dyn Store,
    remote_store: &dyn Store,
    remote_name: &str,
    branch: &str,
) -> Result<Hash, MorphError> {
    fetch_remote(local_store, remote_store, remote_name)?;

    let tracking = format!("remotes/{}/{}", remote_name, branch);
    let remote_tip = local_store.ref_read(&tracking)?.ok_or_else(|| {
        MorphError::NotFound(format!(
            "remote-tracking ref '{}/{}' not found after fetch",
            remote_name, branch
        ))
    })?;

    let local_ref = format!("heads/{}", branch);
    match local_store.ref_read(&local_ref)? {
        Some(local_tip) if local_tip == remote_tip => Ok(local_tip),
        Some(local_tip) => {
            if !is_ancestor(local_store, &local_tip, &remote_tip)? {
                return Err(MorphError::Diverged {
                    branch: branch.to_string(),
                    local_tip: local_tip.to_string(),
                    remote_tip: remote_tip.to_string(),
                });
            }
            local_store.ref_write(&local_ref, &remote_tip)?;
            Ok(remote_tip)
        }
        None => {
            local_store.ref_write(&local_ref, &remote_tip)?;
            Ok(remote_tip)
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Transfer all objects reachable from `tip` that the destination lacks.
fn transfer_objects(
    source: &dyn Store,
    dest: &dyn Store,
    tip: &Hash,
) -> Result<(), MorphError> {
    let missing = collect_reachable_objects(source, tip, &|h| dest.has(h))?;
    for hash in &missing {
        let obj = source.get(hash)?;
        let stored = dest.put(&obj)?;
        if stored != *hash {
            return Err(MorphError::Serialization(format!(
                "hash mismatch during transfer: expected {}, got {}. \
                 Ensure both repos use the same store version.",
                hash, stored
            )));
        }
    }
    Ok(())
}

/// Open a Store for a remote Morph repository at the given path.
/// The path should point to the repo root (the directory containing `.morph/`).
pub fn open_remote_store(remote_path: &str) -> Result<Box<dyn Store>, MorphError> {
    let p = Path::new(remote_path);
    let morph_dir = if p.join(".morph").exists() {
        p.join(".morph")
    } else if p.join("objects").exists() && p.join("refs").exists() {
        p.to_path_buf()
    } else {
        return Err(MorphError::Serialization(format!(
            "not a morph repository: {} (no .morph/ directory found)",
            remote_path
        )));
    };
    crate::repo::open_store(&morph_dir)
}

/// List all refs in the store (heads and remote-tracking).
/// Returns (ref_name, hash) pairs sorted by name.
pub fn list_refs(store: &dyn Store) -> Result<Vec<(String, Hash)>, MorphError> {
    let refs_dir = store.refs_dir();
    let mut result = Vec::new();
    collect_refs_recursive(&refs_dir, "", &mut result)?;
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

fn collect_refs_recursive(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<(String, Hash)>,
) -> Result<(), MorphError> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "HEAD" && prefix.is_empty() {
            continue;
        }
        let full = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", prefix, name)
        };
        if entry.file_type()?.is_dir() {
            collect_refs_recursive(&entry.path(), &full, out)?;
        } else {
            let content = std::fs::read_to_string(entry.path())?;
            let content = content.trim();
            if content.starts_with("ref:") {
                continue;
            }
            if let Ok(h) = Hash::from_hex(content) {
                out.push((full, h));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn setup_repo() -> (tempfile::TempDir, Box<dyn Store>) {
        let dir = tempfile::tempdir().unwrap();
        let _ = crate::repo::init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        let store = crate::repo::open_store(&morph_dir).unwrap();
        (dir, store)
    }

    fn make_commit(store: &dyn Store, root: &Path, msg: &str) -> Hash {
        std::fs::write(root.join(format!("{}.txt", msg.replace(' ', "_"))), msg).unwrap();
        crate::add_paths(store, root, &[PathBuf::from(".")]).unwrap();
        crate::create_tree_commit(
            store,
            root,
            None,
            None,
            BTreeMap::new(),
            msg.to_string(),
            None,
            Some("0.3"),
        )
        .unwrap()
    }

    // ── Remote config ────────────────────────────────────────────────

    #[test]
    fn remote_config_roundtrip() {
        let (dir, _) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        add_remote(&morph_dir, "origin", "/tmp/remote").unwrap();
        add_remote(&morph_dir, "upstream", "/tmp/upstream").unwrap();

        let remotes = read_remotes(&morph_dir).unwrap();
        assert_eq!(remotes.len(), 2);
        assert_eq!(remotes["origin"].path, "/tmp/remote");
        assert_eq!(remotes["upstream"].path, "/tmp/upstream");
    }

    #[test]
    fn remote_config_preserves_repo_version() {
        let (dir, _) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        add_remote(&morph_dir, "origin", "/tmp/remote").unwrap();

        let version = crate::repo::read_repo_version(&morph_dir).unwrap();
        assert_eq!(version, "0.0");
    }

    #[test]
    fn read_remotes_empty_when_none_configured() {
        let (dir, _) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        let remotes = read_remotes(&morph_dir).unwrap();
        assert!(remotes.is_empty());
    }

    // ── Reachability ─────────────────────────────────────────────────

    #[test]
    fn collect_reachable_from_commit() {
        let (dir, store) = setup_repo();
        let hash = make_commit(store.as_ref(), dir.path(), "first");

        let reachable =
            collect_reachable_objects(store.as_ref(), &hash, &|_| Ok(false)).unwrap();

        assert!(reachable.len() >= 3, "should include commit, tree, pipeline, suite, blob");
        assert!(reachable.contains(&hash));
    }

    #[test]
    fn collect_reachable_stops_at_existing() {
        let (dir, store) = setup_repo();
        let c1 = make_commit(store.as_ref(), dir.path(), "first");
        let c2 = make_commit(store.as_ref(), dir.path(), "second");

        let c1_closure: HashSet<Hash> =
            collect_reachable_objects(store.as_ref(), &c1, &|_| Ok(false))
                .unwrap()
                .into_iter()
                .collect();

        let missing =
            collect_reachable_objects(store.as_ref(), &c2, &|h| Ok(c1_closure.contains(h)))
                .unwrap();

        assert!(!missing.contains(&c1));
        assert!(missing.contains(&c2));
    }

    // ── Ancestry ─────────────────────────────────────────────────────

    #[test]
    fn is_ancestor_linear() {
        let (dir, store) = setup_repo();
        let c1 = make_commit(store.as_ref(), dir.path(), "first");
        let c2 = make_commit(store.as_ref(), dir.path(), "second");

        assert!(is_ancestor(store.as_ref(), &c1, &c2).unwrap());
        assert!(!is_ancestor(store.as_ref(), &c2, &c1).unwrap());
        assert!(is_ancestor(store.as_ref(), &c1, &c1).unwrap());
    }

    // ── Push ─────────────────────────────────────────────────────────

    #[test]
    fn push_to_empty_remote() {
        let (local_dir, local_store) = setup_repo();
        let (_, remote_store) = setup_repo();

        let commit = make_commit(local_store.as_ref(), local_dir.path(), "first");

        let tip =
            push_branch(local_store.as_ref(), remote_store.as_ref(), "main").unwrap();
        assert_eq!(tip, commit);

        let remote_tip = remote_store.ref_read("heads/main").unwrap();
        assert_eq!(remote_tip, Some(commit));
        assert!(remote_store.has(&commit).unwrap());
    }

    #[test]
    fn push_non_fast_forward_fails() {
        let (local_dir, local_store) = setup_repo();
        let (remote_dir, remote_store) = setup_repo();

        make_commit(local_store.as_ref(), local_dir.path(), "local");
        make_commit(remote_store.as_ref(), remote_dir.path(), "remote");

        let result =
            push_branch(local_store.as_ref(), remote_store.as_ref(), "main");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("non-fast-forward"), "error: {}", err);
    }

    #[test]
    fn push_already_up_to_date() {
        let (local_dir, local_store) = setup_repo();
        let (_, remote_store) = setup_repo();

        let commit = make_commit(local_store.as_ref(), local_dir.path(), "first");
        push_branch(local_store.as_ref(), remote_store.as_ref(), "main").unwrap();

        let tip =
            push_branch(local_store.as_ref(), remote_store.as_ref(), "main").unwrap();
        assert_eq!(tip, commit);
    }

    #[test]
    fn push_preserves_object_hashes() {
        let (local_dir, local_store) = setup_repo();
        let (_, remote_store) = setup_repo();

        let commit = make_commit(local_store.as_ref(), local_dir.path(), "test");
        push_branch(local_store.as_ref(), remote_store.as_ref(), "main").unwrap();

        let local_obj = local_store.get(&commit).unwrap();
        let remote_obj = remote_store.get(&commit).unwrap();

        let local_json = serde_json::to_string(&local_obj).unwrap();
        let remote_json = serde_json::to_string(&remote_obj).unwrap();
        assert_eq!(local_json, remote_json);
    }

    // ── Fetch ────────────────────────────────────────────────────────

    #[test]
    fn fetch_creates_remote_tracking_refs() {
        let (_, local_store) = setup_repo();
        let (remote_dir, remote_store) = setup_repo();

        let commit = make_commit(remote_store.as_ref(), remote_dir.path(), "remote-commit");

        let updated =
            fetch_remote(local_store.as_ref(), remote_store.as_ref(), "origin").unwrap();
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].0, "main");
        assert_eq!(updated[0].1, commit);

        let tracking = local_store.ref_read("remotes/origin/main").unwrap();
        assert_eq!(tracking, Some(commit));
    }

    #[test]
    fn fetch_does_not_overwrite_local_branch() {
        let (local_dir, local_store) = setup_repo();
        let (remote_dir, remote_store) = setup_repo();

        let local_commit =
            make_commit(local_store.as_ref(), local_dir.path(), "local");
        let _remote_commit =
            make_commit(remote_store.as_ref(), remote_dir.path(), "remote");

        fetch_remote(local_store.as_ref(), remote_store.as_ref(), "origin").unwrap();

        let local_main = local_store.ref_read("heads/main").unwrap();
        assert_eq!(local_main, Some(local_commit), "fetch must not overwrite local branch");
    }

    #[test]
    fn fetch_copies_only_missing_objects() {
        let (_, local_store) = setup_repo();
        let (remote_dir, remote_store) = setup_repo();

        let c1 = make_commit(remote_store.as_ref(), remote_dir.path(), "first");
        fetch_remote(local_store.as_ref(), remote_store.as_ref(), "origin").unwrap();
        assert!(local_store.has(&c1).unwrap());

        let c2 = make_commit(remote_store.as_ref(), remote_dir.path(), "second");

        let missing = collect_reachable_objects(
            remote_store.as_ref(),
            &c2,
            &|h| local_store.has(h),
        )
        .unwrap();

        assert!(!missing.contains(&c1));
        assert!(missing.contains(&c2));
    }

    // ── Pull ─────────────────────────────────────────────────────────

    #[test]
    fn pull_fast_forwards() {
        let (_, local_store) = setup_repo();
        let (remote_dir, remote_store) = setup_repo();

        let commit =
            make_commit(remote_store.as_ref(), remote_dir.path(), "remote-commit");

        let tip = pull_branch(
            local_store.as_ref(),
            remote_store.as_ref(),
            "origin",
            "main",
        )
        .unwrap();
        assert_eq!(tip, commit);

        let local_main = local_store.ref_read("heads/main").unwrap();
        assert_eq!(local_main, Some(commit));
    }

    #[test]
    fn pull_already_up_to_date() {
        let (_, local_store) = setup_repo();
        let (remote_dir, remote_store) = setup_repo();

        let commit =
            make_commit(remote_store.as_ref(), remote_dir.path(), "commit");
        pull_branch(local_store.as_ref(), remote_store.as_ref(), "origin", "main")
            .unwrap();

        let tip = pull_branch(
            local_store.as_ref(),
            remote_store.as_ref(),
            "origin",
            "main",
        )
        .unwrap();
        assert_eq!(tip, commit);
    }

    #[test]
    fn pull_diverged_fails() {
        let (local_dir, local_store) = setup_repo();
        let (remote_dir, remote_store) = setup_repo();

        make_commit(local_store.as_ref(), local_dir.path(), "local");
        make_commit(remote_store.as_ref(), remote_dir.path(), "remote");

        let result = pull_branch(
            local_store.as_ref(),
            remote_store.as_ref(),
            "origin",
            "main",
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("fast-forward") || err.contains("diverged") || err.contains("Diverged"),
            "error: {}",
            err
        );
    }

    #[test]
    fn pull_branch_returns_diverged_for_diverged_branches() {
        // PR 4 cycle 1: divergence must produce a typed `MorphError::Diverged`
        // with branch / local_tip / remote_tip populated, not an opaque
        // string-based error. The CLI uses the typed variant to suggest
        // `morph pull --merge`.
        let (local_dir, local_store) = setup_repo();
        let (remote_dir, remote_store) = setup_repo();

        let local_tip =
            make_commit(local_store.as_ref(), local_dir.path(), "local");
        let remote_tip =
            make_commit(remote_store.as_ref(), remote_dir.path(), "remote");

        let err = pull_branch(
            local_store.as_ref(),
            remote_store.as_ref(),
            "origin",
            "main",
        )
        .unwrap_err();

        match err {
            MorphError::Diverged {
                branch,
                local_tip: lt,
                remote_tip: rt,
            } => {
                assert_eq!(branch, "main");
                assert_eq!(lt, local_tip.to_string());
                assert_eq!(rt, remote_tip.to_string());
            }
            other => panic!("expected MorphError::Diverged, got: {:?}", other),
        }
    }

    #[test]
    fn pull_branch_still_fast_forwards_when_local_is_ancestor() {
        // Cycle 2: regression — clean fast-forward path still works after
        // we change the divergence error type.
        let (_, local_store) = setup_repo();
        let (remote_dir, remote_store) = setup_repo();

        let commit =
            make_commit(remote_store.as_ref(), remote_dir.path(), "remote-commit");

        let tip = pull_branch(
            local_store.as_ref(),
            remote_store.as_ref(),
            "origin",
            "main",
        )
        .unwrap();
        assert_eq!(tip, commit);
        assert_eq!(
            local_store.ref_read("heads/main").unwrap(),
            Some(commit)
        );
    }

    #[test]
    fn pull_branch_already_up_to_date_returns_local_tip() {
        // Cycle 3: regression — pull is idempotent.
        let (_, local_store) = setup_repo();
        let (remote_dir, remote_store) = setup_repo();

        let commit =
            make_commit(remote_store.as_ref(), remote_dir.path(), "commit");
        pull_branch(local_store.as_ref(), remote_store.as_ref(), "origin", "main")
            .unwrap();

        let tip = pull_branch(
            local_store.as_ref(),
            remote_store.as_ref(),
            "origin",
            "main",
        )
        .unwrap();
        assert_eq!(tip, commit);
    }

    // ── Evidence-backed sync ─────────────────────────────────────────

    #[test]
    fn sync_includes_evidence_backed_commit() {
        let (local_dir, local_store) = setup_repo();
        let (_, remote_store) = setup_repo();

        let trace = MorphObject::Trace(Trace {
            events: vec![TraceEvent {
                id: "e1".into(),
                seq: 0,
                ts: "2025-01-01T00:00:00Z".into(),
                kind: "prompt".into(),
                payload: BTreeMap::new(),
            }],
        });
        let trace_hash = local_store.put(&trace).unwrap();

        let identity = crate::identity_pipeline();
        let pipeline_hash = local_store.put(&identity).unwrap();

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
            trace: trace_hash.to_string(),
            agent: AgentInfo {
                id: "a".into(),
                version: "1".into(),
                policy: None,
                instance_id: None,
            },
            contributors: None,
            morph_version: None,
        });
        let run_hash = local_store.put(&run).unwrap();

        let provenance =
            crate::resolve_provenance_from_run(local_store.as_ref(), &run_hash).unwrap();
        std::fs::write(local_dir.path().join("code.txt"), "hello").unwrap();
        crate::add_paths(
            local_store.as_ref(),
            local_dir.path(),
            &[PathBuf::from(".")],
        )
        .unwrap();
        let commit_hash = crate::create_tree_commit_with_provenance(
            local_store.as_ref(),
            local_dir.path(),
            None,
            None,
            BTreeMap::new(),
            "evidence".to_string(),
            None,
            Some("0.3"),
            Some(&provenance),
        )
        .unwrap();

        push_branch(local_store.as_ref(), remote_store.as_ref(), "main").unwrap();

        assert!(remote_store.has(&commit_hash).unwrap());
        assert!(remote_store.has(&run_hash).unwrap());
        assert!(remote_store.has(&trace_hash).unwrap());

        let remote_commit = match remote_store.get(&commit_hash).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        let refs = remote_commit.evidence_refs.as_ref().unwrap();
        assert!(refs.contains(&run_hash.to_string()));
        assert!(refs.contains(&trace_hash.to_string()));
    }

    // ── Remote store ─────────────────────────────────────────────────

    #[test]
    fn open_remote_store_fails_on_invalid_path() {
        let result = open_remote_store("/nonexistent/path");
        assert!(result.is_err());
    }

    #[test]
    fn open_remote_store_works_on_valid_repo() {
        let (dir, _) = setup_repo();
        let path = dir.path().to_string_lossy().to_string();
        let store = open_remote_store(&path).unwrap();
        assert!(store.refs_dir().exists());
    }

    // ── list_refs ────────────────────────────────────────────────────

    #[test]
    fn list_refs_shows_branches_and_tracking() {
        let (local_dir, local_store) = setup_repo();
        let (remote_dir, remote_store) = setup_repo();

        make_commit(local_store.as_ref(), local_dir.path(), "local");
        make_commit(remote_store.as_ref(), remote_dir.path(), "remote");

        fetch_remote(local_store.as_ref(), remote_store.as_ref(), "origin").unwrap();

        let refs = list_refs(local_store.as_ref()).unwrap();
        let names: Vec<&str> = refs.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"heads/main"), "should list local branch");
        assert!(
            names.contains(&"remotes/origin/main"),
            "should list remote-tracking ref"
        );
    }
}
