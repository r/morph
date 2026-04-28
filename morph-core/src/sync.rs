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

// ── Branch upstream config ───────────────────────────────────────────

/// Per-branch upstream tracking. Mirrors git's `branch.<name>.remote`
/// + `branch.<name>.merge`. Drives `morph sync` and the
///   "Already up to date" / "Diverged" hint in status.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BranchUpstream {
    pub remote: String,
    pub branch: String,
}

/// Read all configured branch upstreams from config.json.
pub fn read_branch_upstreams(
    morph_dir: &Path,
) -> Result<BTreeMap<String, BranchUpstream>, MorphError> {
    let config_path = morph_dir.join("config.json");
    if !config_path.exists() {
        return Ok(BTreeMap::new());
    }
    let data = std::fs::read_to_string(&config_path)?;
    let config: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| MorphError::Serialization(e.to_string()))?;
    match config.get("branches") {
        Some(b) => serde_json::from_value(b.clone())
            .map_err(|e| MorphError::Serialization(e.to_string())),
        None => Ok(BTreeMap::new()),
    }
}

/// Persist the upstream for a single branch, preserving other keys
/// in config.json.
pub fn set_branch_upstream(
    morph_dir: &Path,
    branch: &str,
    upstream: BranchUpstream,
) -> Result<(), MorphError> {
    let config_path = morph_dir.join("config.json");
    let mut config: serde_json::Value = if config_path.exists() {
        let data = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&data).map_err(|e| MorphError::Serialization(e.to_string()))?
    } else {
        serde_json::json!({})
    };
    let mut branches = read_branch_upstreams(morph_dir)?;
    branches.insert(branch.to_string(), upstream);
    config["branches"] = serde_json::to_value(branches)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    let pretty = serde_json::to_string_pretty(&config)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    std::fs::write(&config_path, pretty)?;
    Ok(())
}

pub fn get_branch_upstream(
    morph_dir: &Path,
    branch: &str,
) -> Result<Option<BranchUpstream>, MorphError> {
    Ok(read_branch_upstreams(morph_dir)?.remove(branch))
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

/// PR 6 stage F: verify that the entire reachable closure of `tip`
/// is present in `store`. Returns `Err(MorphError::NotFound(_))` on
/// the first missing object.
///
/// The server side of `push` calls this on `RefWrite` so it never
/// records a ref that points at an object the client hasn't fully
/// uploaded — a bare repo with such a ref would silently corrupt
/// every subsequent fetch.
pub fn verify_closure(store: &dyn Store, tip: &Hash) -> Result<(), MorphError> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(*tip);
    while let Some(h) = queue.pop_front() {
        if !visited.insert(h) {
            continue;
        }
        let obj = store.get(&h)?;
        collect_refs(&obj, &mut queue);
    }
    Ok(())
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
    // Transport-neutral enumeration. Filesystem stores walk
    // `refs/heads`; SSH stores will issue a single `list-branches`
    // RPC. Either way `fetch_remote` no longer touches the
    // filesystem of the remote directly.
    let mut updated = Vec::new();
    for (branch, tip) in remote_store.list_branches()? {
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
    // SSH transport: anything that looks like a URL or scp-style
    // host:path. `SshUrl::parse` returns `None` for a plain
    // filesystem path so we fall through.
    if let Some(url) = crate::ssh_store::SshUrl::parse(remote_path) {
        let spawn = crate::ssh_store::RemoteSpawn::new(url);
        let store = crate::ssh_store::SshStore::connect(&spawn)?;
        return Ok(Box::new(store));
    }

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

// ── Clone (PR 8) ─────────────────────────────────────────────────────

/// Options for [`clone_repo`].
#[derive(Clone, Debug, Default)]
pub struct CloneOpts {
    /// Branch to check out. When `None`, `clone_repo` picks the
    /// remote's HEAD branch if it can read it (filesystem remotes
    /// always can; SSH remotes don't expose HEAD on the v0 wire so
    /// the fallback is `"main"`).
    pub branch: Option<String>,
    /// Create a bare destination (no `.morph/` wrapper, no working
    /// tree restored). Used when cloning to set up a server.
    pub bare: bool,
}

/// Result of a successful [`clone_repo`].
#[derive(Clone, Debug)]
pub struct CloneOutcome {
    /// Branch checked out locally.
    pub branch: String,
    /// Tip of the checked-out branch.
    pub tip: Hash,
    /// Every (branch, tip) fetched from the remote.
    pub fetched: Vec<(String, Hash)>,
}

/// PR 8: clone a remote Morph repository to a fresh local
/// destination. Composes `init_repo` / `init_bare`, `add_remote`,
/// `fetch_remote`, and the local checkout into a single onboarding
/// command — the `morph clone` users expect from Git.
///
/// Behavior:
/// - Refuses to clone into a non-empty directory (preserves any
///   user-authored files just like `git clone` does).
/// - Configures the new repo with `origin = <remote_url>`.
/// - Fetches every branch into `refs/remotes/origin/*`.
/// - Picks the default branch from `opts.branch` → remote HEAD →
///   `"main"` and writes it to `refs/heads/<branch>`.
/// - Sets the per-branch upstream to `origin/<branch>` so
///   `morph sync` works out of the box.
/// - For working clones (`opts.bare = false`), restores the tree
///   into the working directory; for bare clones, leaves the
///   working tree alone.
pub fn clone_repo(
    remote_url: &str,
    destination: &Path,
    opts: CloneOpts,
) -> Result<CloneOutcome, MorphError> {
    if destination.exists() {
        let mut iter = std::fs::read_dir(destination).map_err(MorphError::Io)?;
        if iter.next().is_some() {
            return Err(MorphError::AlreadyExists(format!(
                "destination '{}' is not empty; refusing to clone over existing files",
                destination.display()
            )));
        }
    } else {
        std::fs::create_dir_all(destination).map_err(MorphError::Io)?;
    }

    let local_store: Box<dyn Store> = if opts.bare {
        Box::new(crate::repo::init_bare(destination)?)
    } else {
        Box::new(crate::repo::init_repo(destination)?)
    };
    let morph_dir = if opts.bare {
        destination.to_path_buf()
    } else {
        destination.join(".morph")
    };

    add_remote(&morph_dir, "origin", remote_url)?;

    let remote_store = open_remote_store(remote_url)?;
    let fetched = fetch_remote(local_store.as_ref(), remote_store.as_ref(), "origin")?;

    let branch = match &opts.branch {
        Some(b) => b.clone(),
        None => detect_default_branch(remote_store.as_ref()),
    };

    let tracking = format!("remotes/{}/{}", "origin", branch);
    let tip = local_store.ref_read(&tracking)?.ok_or_else(|| {
        let available: Vec<String> = fetched.iter().map(|(b, _)| b.clone()).collect();
        MorphError::NotFound(format!(
            "branch '{}' not found on remote '{}' (available: {})",
            branch,
            remote_url,
            if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            }
        ))
    })?;

    let local_ref = format!("heads/{}", branch);
    local_store.ref_write(&local_ref, &tip)?;
    crate::commit::set_head_branch(local_store.as_ref(), &branch)?;

    set_branch_upstream(
        &morph_dir,
        &branch,
        BranchUpstream {
            remote: "origin".into(),
            branch: branch.clone(),
        },
    )?;

    if !opts.bare {
        if let MorphObject::Commit(commit) = local_store.get(&tip)? {
            if let Some(tree_hash_str) = &commit.tree {
                let tree_hash = Hash::from_hex(tree_hash_str)?;
                crate::tree::restore_tree(local_store.as_ref(), &tree_hash, destination)?;
            }
        }
    }

    Ok(CloneOutcome {
        branch,
        tip,
        fetched,
    })
}

/// Best-effort detection of a remote's default branch via `HEAD`.
/// Filesystem stores expose this directly; SSH stores currently
/// return an error from `ref_read_raw`, in which case we fall back
/// to the conventional `"main"`. Either way the user can always
/// override with `CloneOpts::branch`.
fn detect_default_branch(remote: &dyn Store) -> String {
    if let Ok(Some(raw)) = remote.ref_read_raw("HEAD") {
        let trimmed = raw.trim();
        if let Some(rest) = trimmed.strip_prefix("ref:") {
            let path = rest.trim();
            if let Some(branch) = path.strip_prefix("heads/") {
                return branch.to_string();
            }
        }
    }
    "main".to_string()
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

        let initial = crate::repo::read_repo_version(&morph_dir).unwrap();
        add_remote(&morph_dir, "origin", "/tmp/remote").unwrap();

        let version = crate::repo::read_repo_version(&morph_dir).unwrap();
        assert_eq!(
            version, initial,
            "add_remote must not change the repo's store version"
        );
    }

    #[test]
    fn branch_upstream_round_trip() {
        // PR5 cycle 30 RED→GREEN.
        let (dir, _) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        assert!(get_branch_upstream(&morph_dir, "main").unwrap().is_none());

        set_branch_upstream(
            &morph_dir,
            "main",
            BranchUpstream {
                remote: "origin".into(),
                branch: "main".into(),
            },
        )
        .unwrap();

        let got = get_branch_upstream(&morph_dir, "main").unwrap().unwrap();
        assert_eq!(got.remote, "origin");
        assert_eq!(got.branch, "main");
    }

    #[test]
    fn set_branch_upstream_preserves_remotes() {
        // PR5 cycle 30 RED→GREEN: config.json must keep
        // pre-existing keys (here `remotes`) when we add the
        // branches section.
        let (dir, _) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        add_remote(&morph_dir, "origin", "/some/path").unwrap();

        set_branch_upstream(
            &morph_dir,
            "main",
            BranchUpstream {
                remote: "origin".into(),
                branch: "main".into(),
            },
        )
        .unwrap();

        let remotes = read_remotes(&morph_dir).unwrap();
        assert_eq!(remotes["origin"].path, "/some/path");
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

    // ── Closure verification (PR 6 stage F, cycle 24) ────────────────

    #[test]
    fn verify_closure_passes_when_all_objects_present() {
        // PR 6 stage F cycle 24 RED→GREEN: the happy path. After a
        // successful sync, every reachable object is in the
        // destination store, and verify_closure returns Ok.
        let (dir, store) = setup_repo();
        let hash = make_commit(store.as_ref(), dir.path(), "first");
        verify_closure(store.as_ref(), &hash).expect("closure should be present");
    }

    #[test]
    fn verify_closure_fails_when_tip_missing() {
        // The simplest case: the tip itself isn't in the store.
        // Critical for the server side of `push`: we must refuse a
        // ref-write that points at an object we never received.
        let (_dir, store) = setup_repo();
        let bogus = Hash::from_hex(&"a".repeat(64)).unwrap();
        let err = verify_closure(store.as_ref(), &bogus)
            .expect_err("bogus tip must fail closure check");
        assert!(matches!(err, MorphError::NotFound(_)));
    }

    #[test]
    fn verify_closure_fails_when_dependency_missing() {
        // Source has the full closure, destination only has the
        // commit and tree but not the blob — verify_closure must
        // catch that. Mimics a partial upload that crashed
        // mid-flight.
        let (src_dir, src_store) = setup_repo();
        let tip = make_commit(src_store.as_ref(), src_dir.path(), "first");

        let dest_dir = tempfile::tempdir().unwrap();
        let _ = crate::repo::init_repo(dest_dir.path()).unwrap();
        let dest_store = crate::repo::open_store(&dest_dir.path().join(".morph")).unwrap();

        // Copy only the commit (and its tree+pipeline+suite) but
        // intentionally omit the blob the tree points at.
        let commit_obj = src_store.get(&tip).unwrap();
        let _ = dest_store.put(&commit_obj).unwrap();
        if let crate::objects::MorphObject::Commit(c) = &commit_obj {
            if let Some(t) = &c.tree {
                let tree_h = Hash::from_hex(t).unwrap();
                let tree_obj = src_store.get(&tree_h).unwrap();
                let _ = dest_store.put(&tree_obj).unwrap();
            }
            let pipe_h = Hash::from_hex(&c.pipeline).unwrap();
            let pipe_obj = src_store.get(&pipe_h).unwrap();
            let _ = dest_store.put(&pipe_obj).unwrap();
            let suite_h = Hash::from_hex(&c.eval_contract.suite).unwrap();
            let suite_obj = src_store.get(&suite_h).unwrap();
            let _ = dest_store.put(&suite_obj).unwrap();
        }

        let err = verify_closure(dest_store.as_ref(), &tip)
            .expect_err("missing blob should be detected");
        assert!(matches!(err, MorphError::NotFound(_)));
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

    /// Wraps an FsStore and hides `refs_dir()` (returns a dangling
    /// path) while delegating `list_branches` / `list_refs` to the
    /// inner store. Used to assert `fetch_remote` only reaches into
    /// the trait method, not into the filesystem — i.e. the same
    /// fetch logic will work for an SSH-backed Store in PR5 Stage D.
    struct OpaqueRefsStore {
        inner: Box<dyn Store>,
        fake_refs: PathBuf,
    }

    impl Store for OpaqueRefsStore {
        fn put(&self, o: &MorphObject) -> Result<Hash, MorphError> {
            self.inner.put(o)
        }
        fn get(&self, h: &Hash) -> Result<MorphObject, MorphError> {
            self.inner.get(h)
        }
        fn has(&self, h: &Hash) -> Result<bool, MorphError> {
            self.inner.has(h)
        }
        fn list(&self, t: crate::ObjectType) -> Result<Vec<Hash>, MorphError> {
            self.inner.list(t)
        }
        fn ref_read(&self, name: &str) -> Result<Option<Hash>, MorphError> {
            self.inner.ref_read(name)
        }
        fn ref_write(&self, name: &str, h: &Hash) -> Result<(), MorphError> {
            self.inner.ref_write(name, h)
        }
        fn ref_read_raw(&self, name: &str) -> Result<Option<String>, MorphError> {
            self.inner.ref_read_raw(name)
        }
        fn ref_write_raw(&self, name: &str, value: &str) -> Result<(), MorphError> {
            self.inner.ref_write_raw(name, value)
        }
        fn ref_delete(&self, name: &str) -> Result<(), MorphError> {
            self.inner.ref_delete(name)
        }
        fn refs_dir(&self) -> PathBuf {
            self.fake_refs.clone()
        }
        fn hash_object(&self, o: &MorphObject) -> Result<Hash, MorphError> {
            self.inner.hash_object(o)
        }
        fn list_refs(&self, prefix: &str) -> Result<Vec<(String, Hash)>, MorphError> {
            self.inner.list_refs(prefix)
        }
        fn list_branches(&self) -> Result<Vec<(String, Hash)>, MorphError> {
            self.inner.list_branches()
        }
    }

    #[test]
    fn fetch_remote_uses_list_branches_not_refs_dir() {
        // PR5 cycle 2: fetch_remote must work over a Store impl that
        // does not expose a real refs_dir on the local filesystem,
        // i.e. it must drive enumeration through `list_branches()`.
        let (_, local_store) = setup_repo();
        let (remote_dir, remote_store) = setup_repo();
        let commit =
            make_commit(remote_store.as_ref(), remote_dir.path(), "remote-only");

        let opaque = OpaqueRefsStore {
            inner: remote_store,
            fake_refs: PathBuf::from("/var/empty/no-such-refs-dir-xyz"),
        };

        let updated =
            fetch_remote(local_store.as_ref(), &opaque, "origin").unwrap();
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].0, "main");
        assert_eq!(updated[0].1, commit);
        assert_eq!(
            local_store.ref_read("remotes/origin/main").unwrap(),
            Some(commit)
        );
    }

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

    #[test]
    fn open_remote_store_dispatches_ssh_urls() {
        // PR5 cycle 25 RED→GREEN: an `ssh://...` URL must take the
        // SshStore branch, not the FS one. We stub MORPH_SSH to a
        // command that fails fast so the test is deterministic; the
        // important assertion is that the failure message blames
        // the spawn (i.e. we reached the SSH branch) rather than
        // "not a morph repository" (the filesystem branch).
        let key = "MORPH_SSH";
        let prev = std::env::var(key).ok();
        std::env::set_var(key, "/bin/false");
        let result = open_remote_store("ssh://nobody@unreachable.invalid/repo");
        if let Some(p) = prev {
            std::env::set_var(key, p);
        } else {
            std::env::remove_var(key);
        }
        let err = match result {
            Ok(_) => panic!("ssh dial should fail in test"),
            Err(e) => e,
        };
        let msg = err.to_string().to_lowercase();
        assert!(
            !msg.contains("not a morph repository"),
            "should not have hit the FS branch: {}",
            err
        );
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

    // ── PR 8: clone_repo ─────────────────────────────────────────────

    /// PR 8 cycle 1: cloning a working repo into an empty destination
    /// produces a `.morph/` layout, configures `origin`, fetches every
    /// branch, and checks out the default branch's tree.
    #[test]
    fn clone_repo_into_empty_destination_creates_working_repo() {
        let (remote_dir, remote_store) = setup_repo();
        let tip = make_commit(remote_store.as_ref(), remote_dir.path(), "from_remote");

        let dest = tempfile::tempdir().unwrap();
        let dest_path = dest.path().join("clone");

        let outcome = clone_repo(
            &remote_dir.path().to_string_lossy(),
            &dest_path,
            CloneOpts::default(),
        )
        .unwrap();

        assert_eq!(outcome.branch, "main");
        assert_eq!(outcome.tip, tip);
        assert!(dest_path.join(".morph").is_dir(), ".morph/ should exist");
        assert!(dest_path.join(".morph/objects").is_dir());

        // origin remote configured.
        let remotes = read_remotes(&dest_path.join(".morph")).unwrap();
        assert_eq!(remotes["origin"].path, remote_dir.path().to_string_lossy());

        // local heads/main matches remote tip.
        let local_store = crate::repo::open_store(&dest_path.join(".morph")).unwrap();
        let local_main = local_store.ref_read("heads/main").unwrap().unwrap();
        assert_eq!(local_main, tip);

        // remote-tracking ref also written.
        let tracking = local_store.ref_read("remotes/origin/main").unwrap().unwrap();
        assert_eq!(tracking, tip);

        // working tree restored.
        assert!(
            dest_path.join("from_remote.txt").exists(),
            "checked-out file should exist in working tree"
        );
    }

    /// PR 8 cycle 2: refuse to clone into a non-empty directory so we
    /// never silently scribble inside a half-populated workspace.
    #[test]
    fn clone_repo_refuses_existing_non_empty_destination() {
        let (remote_dir, remote_store) = setup_repo();
        make_commit(remote_store.as_ref(), remote_dir.path(), "remote");

        let dest = tempfile::tempdir().unwrap();
        let dest_path = dest.path().join("clone");
        std::fs::create_dir_all(&dest_path).unwrap();
        std::fs::write(dest_path.join("preexisting.txt"), "do not destroy").unwrap();

        let result = clone_repo(
            &remote_dir.path().to_string_lossy(),
            &dest_path,
            CloneOpts::default(),
        );
        assert!(matches!(result, Err(MorphError::AlreadyExists(_))));
        assert!(
            dest_path.join("preexisting.txt").exists(),
            "preexisting file must be untouched on refusal"
        );
    }

    /// PR 8 cycle 3: explicit `--branch` overrides the remote's HEAD
    /// for users who want to clone a topic branch directly.
    #[test]
    fn clone_repo_with_explicit_branch_checks_out_that_branch() {
        let (remote_dir, remote_store) = setup_repo();
        let _main_tip = make_commit(remote_store.as_ref(), remote_dir.path(), "main_commit");
        // Create a `feature` branch pointing at a different commit.
        let feature_tip = {
            let r = remote_store.as_ref();
            crate::commit::set_head_branch(r, "feature").unwrap();
            make_commit(r, remote_dir.path(), "feature_commit")
        };
        // Reset HEAD back to main on the remote so our default would
        // pick `main` if we didn't pass an explicit branch.
        crate::commit::set_head_branch(remote_store.as_ref(), "main").unwrap();

        let dest = tempfile::tempdir().unwrap();
        let dest_path = dest.path().join("clone");

        let outcome = clone_repo(
            &remote_dir.path().to_string_lossy(),
            &dest_path,
            CloneOpts {
                branch: Some("feature".into()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(outcome.branch, "feature");
        assert_eq!(outcome.tip, feature_tip);
        let local_store = crate::repo::open_store(&dest_path.join(".morph")).unwrap();
        assert_eq!(
            local_store.ref_read("heads/feature").unwrap().unwrap(),
            feature_tip
        );
        // working tree reflects the feature commit.
        assert!(dest_path.join("feature_commit.txt").exists());
    }

    /// PR 8 cycle 4: `--bare` produces a server-shaped layout with no
    /// working tree restored.
    #[test]
    fn clone_repo_into_bare_creates_bare_layout_without_working_tree() {
        let (remote_dir, remote_store) = setup_repo();
        let tip = make_commit(remote_store.as_ref(), remote_dir.path(), "remote");

        let dest = tempfile::tempdir().unwrap();
        let dest_path = dest.path().join("server.morph");

        let outcome = clone_repo(
            &remote_dir.path().to_string_lossy(),
            &dest_path,
            CloneOpts {
                bare: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(outcome.tip, tip);
        // Bare layout: objects/ and refs/ at the root, no .morph/ wrapper.
        assert!(dest_path.join("objects").is_dir());
        assert!(dest_path.join("refs/heads").is_dir());
        assert!(!dest_path.join(".morph").exists(), "bare repo has no .morph/ wrapper");
        // No working tree.
        assert!(
            !dest_path.join("remote.txt").exists(),
            "bare clone must not restore working tree"
        );
        // is_bare reads true.
        assert!(crate::repo::is_bare(&dest_path).unwrap());
    }

    /// PR 8 cycle 5: a working clone configures the default branch's
    /// upstream so `morph sync` works out of the box.
    #[test]
    fn clone_repo_sets_upstream_for_default_branch() {
        let (remote_dir, remote_store) = setup_repo();
        make_commit(remote_store.as_ref(), remote_dir.path(), "remote");

        let dest = tempfile::tempdir().unwrap();
        let dest_path = dest.path().join("clone");

        clone_repo(
            &remote_dir.path().to_string_lossy(),
            &dest_path,
            CloneOpts::default(),
        )
        .unwrap();

        let upstream = get_branch_upstream(&dest_path.join(".morph"), "main")
            .unwrap()
            .expect("upstream should be configured by clone");
        assert_eq!(upstream.remote, "origin");
        assert_eq!(upstream.branch, "main");
    }

    /// PR 8 cycle 6: cloning when the requested branch doesn't exist
    /// on the remote must fail with a clear NotFound.
    #[test]
    fn clone_repo_errors_when_requested_branch_missing() {
        let (remote_dir, remote_store) = setup_repo();
        make_commit(remote_store.as_ref(), remote_dir.path(), "remote");

        let dest = tempfile::tempdir().unwrap();
        let dest_path = dest.path().join("clone");

        let err = clone_repo(
            &remote_dir.path().to_string_lossy(),
            &dest_path,
            CloneOpts {
                branch: Some("does-not-exist".into()),
                ..Default::default()
            },
        )
        .unwrap_err();

        assert!(
            matches!(&err, MorphError::NotFound(msg) if msg.contains("does-not-exist")),
            "expected NotFound mentioning the missing branch, got: {:?}",
            err
        );
    }
}
