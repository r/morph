//! Working-space operations: create blobs from files, materialize, status, add.

use crate::diff::{diff_file_maps, DiffEntry};
use crate::morphignore::{is_ignored, is_rel_path_ignored, load_ignore_rules};
use crate::objects::{Blob, EvalSuite, MorphObject, Pipeline};
use crate::Hash;
use crate::store::{MorphError, Store};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::collections::BTreeMap;
use std::path::Path;

/// Find repository root by walking up from `from` until we find a directory containing `.morph`.
pub fn find_repo(from: &Path) -> Option<std::path::PathBuf> {
    let mut current = from.canonicalize().ok()?;
    loop {
        if current.join(".morph").is_dir() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Create a prompt Blob from a file path. Reads file as UTF-8; content is {"body": "<contents>"}.
pub fn blob_from_prompt_file(path: &Path) -> Result<MorphObject, MorphError> {
    let body = std::fs::read_to_string(path)?;
    let content = serde_json::json!({ "body": body });
    Ok(MorphObject::Blob(Blob {
        kind: "prompt".to_string(),
        content,
    }))
}

/// Create a Blob from file with given kind.
/// Content is {"body": "<utf8 contents>"} for text, or {"body": "<base64>", "encoding": "base64"} for binary.
pub fn blob_from_file(path: &Path, kind: &str) -> Result<MorphObject, MorphError> {
    let bytes = std::fs::read(path)?;
    let content = match std::str::from_utf8(&bytes) {
        Ok(s) => serde_json::json!({ "body": s }),
        Err(_) => serde_json::json!({ "body": BASE64.encode(&bytes), "encoding": "base64" }),
    };
    Ok(MorphObject::Blob(Blob {
        kind: kind.to_string(),
        content,
    }))
}

/// Materialize a Blob from the store to a file path. Extracts "body" from content or whole content as JSON string.
/// If content has "encoding": "base64", decodes body and writes raw bytes.
pub fn materialize_blob(store: &dyn Store, hash: &Hash, dest: &Path) -> Result<(), MorphError> {
    let obj = store.get(hash)?;
    let bytes: Vec<u8> = match &obj {
        MorphObject::Blob(blob) => {
            let body_str: std::borrow::Cow<str> = match blob.content.get("body").and_then(|v| v.as_str()) {
                Some(s) => std::borrow::Cow::Borrowed(s),
                None => std::borrow::Cow::Owned(serde_json::to_string(&blob.content).unwrap_or_default()),
            };
            if blob.content.get("encoding").and_then(|v| v.as_str()) == Some("base64") {
                BASE64.decode(body_str.as_ref().as_bytes()).map_err(|e| MorphError::Serialization(format!("invalid base64: {}", e)))?
            } else {
                body_str.as_bytes().to_vec()
            }
        }
        other => serde_json::to_string_pretty(other)
            .map_err(|e| MorphError::Serialization(e.to_string()))?
            .into_bytes(),
    };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dest, bytes)?;
    Ok(())
}

/// Parse a Pipeline from a JSON file.
pub fn pipeline_from_file(path: &Path) -> Result<MorphObject, MorphError> {
    let s = std::fs::read_to_string(path)?;
    let pipeline: Pipeline = serde_json::from_str(&s).map_err(|e| MorphError::Serialization(e.to_string()))?;
    Ok(MorphObject::Pipeline(pipeline))
}

/// Parse an EvalSuite from a JSON file.
pub fn eval_suite_from_file(path: &Path) -> Result<MorphObject, MorphError> {
    let s = std::fs::read_to_string(path)?;
    let suite: EvalSuite = serde_json::from_str(&s).map_err(|e| MorphError::Serialization(e.to_string()))?;
    Ok(MorphObject::EvalSuite(suite))
}

/// Status entry for one working-space file.
#[derive(Debug, Clone)]
pub struct StatusEntry {
    pub path: std::path::PathBuf,
    pub in_store: bool,
    pub hash: Option<Hash>,
}

/// Classify a file path into an object kind using canonical morph subdirectory paths.
fn classify_file(path: &Path, morph_prompts: &Path, morph_evals: &Path) -> &'static str {
    if path.starts_with(morph_prompts) {
        return "prompt";
    }
    if path.starts_with(morph_evals) {
        return "eval";
    }
    "blob"
}

fn object_from_file(path: &Path, kind: &str) -> Result<MorphObject, MorphError> {
    match kind {
        "eval" => eval_suite_from_file(path),
        "prompt" => blob_from_file(path, "prompt"),
        _ => blob_from_file(path, "blob"),
    }
}

/// Returns true if `path` is inside `morph_dir` but NOT inside prompts or evals.
fn is_morph_internal(path: &Path, morph_dir: &Path, morph_prompts: &Path, morph_evals: &Path) -> bool {
    if !path.starts_with(morph_dir) {
        return false;
    }
    !path.starts_with(morph_prompts) && !path.starts_with(morph_evals)
}

/// Resolve canonical paths for the morph directory and its metadata subdirs.
fn resolve_morph_paths(repo_root: &Path) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let morph_dir = repo_root.join(".morph");
    let morph_dir = morph_dir.canonicalize().unwrap_or(morph_dir);
    let prompts = morph_dir.join("prompts");
    let prompts = prompts.canonicalize().unwrap_or(prompts);
    let evals = morph_dir.join("evals");
    let evals = evals.canonicalize().unwrap_or(evals);
    (morph_dir, prompts, evals)
}

/// Compute status: scan the working directory only.
/// The entire `.morph/` tree (including prompts and evals) is excluded — those
/// files are internal bookkeeping already stored as objects when recorded.
pub fn status(store: &dyn Store, repo_root: &Path) -> Result<Vec<StatusEntry>, MorphError> {
    let mut entries = Vec::new();
    let canonical_root = repo_root.canonicalize().unwrap_or_else(|_| repo_root.to_path_buf());
    let (morph_dir, morph_prompts, morph_evals) = resolve_morph_paths(repo_root);
    let morphignore = load_ignore_rules(&canonical_root);

    for entry in walkdir::WalkDir::new(repo_root)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| {
            let p = e.path();
            let canonical = p.canonicalize().unwrap_or(p.to_path_buf());
            if canonical == morph_dir {
                return false;
            }
            !is_ignored(morphignore.as_ref(), &canonical_root, &canonical, e.file_type().is_dir())
        })
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let canonical = path.canonicalize().unwrap_or(path.to_path_buf());
        if is_ignored(morphignore.as_ref(), &canonical_root, &canonical, false) {
            continue;
        }
        let kind = classify_file(path, &morph_prompts, &morph_evals);
        if let Ok(obj) = object_from_file(path, kind) {
            let hash = store.hash_object(&obj)?;
            let in_store = store.has(&hash)?;
            entries.push(StatusEntry {
                path: path.to_path_buf(),
                in_store,
                hash: Some(hash),
            });
        }
    }

    Ok(entries)
}

/// Git-style status: diff the working directory against HEAD's committed tree.
/// Returns a list of changes (added, modified, deleted) relative to the last commit.
/// On a fresh repo with no commits, all working-dir files appear as Added.
pub fn working_status(store: &dyn Store, repo_root: &Path) -> Result<Vec<DiffEntry>, MorphError> {
    let canonical_root = repo_root.canonicalize().unwrap_or_else(|_| repo_root.to_path_buf());
    let (morph_dir, morph_prompts, morph_evals) = resolve_morph_paths(repo_root);
    let morphignore = load_ignore_rules(&canonical_root);

    // Build working-dir file map: relative path -> content hash
    let mut working_files: BTreeMap<String, String> = BTreeMap::new();
    for entry in walkdir::WalkDir::new(repo_root)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| {
            let p = e.path();
            let canonical = p.canonicalize().unwrap_or(p.to_path_buf());
            if canonical == morph_dir {
                return false;
            }
            !is_ignored(morphignore.as_ref(), &canonical_root, &canonical, e.file_type().is_dir())
        })
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let canonical = path.canonicalize().unwrap_or(path.to_path_buf());
        if is_ignored(morphignore.as_ref(), &canonical_root, &canonical, false) {
            continue;
        }
        let kind = classify_file(path, &morph_prompts, &morph_evals);
        if let Ok(obj) = object_from_file(path, kind) {
            let hash = store.hash_object(&obj)?;
            if let Some(rel) = relative_path(&canonical_root, &canonical) {
                working_files.insert(rel, hash.to_string());
            }
        }
    }

    // Get HEAD's committed tree (empty if no commits yet)
    let head_files: BTreeMap<String, String> = match crate::commit::resolve_head(store)? {
        Some(head_hash) => {
            let obj = store.get(&head_hash)?;
            match obj {
                MorphObject::Commit(c) => match c.tree {
                    Some(ref tree_hash_str) => {
                        let tree_hash = Hash::from_hex(tree_hash_str)?;
                        crate::tree::flatten_tree(store, &tree_hash)?
                    }
                    None => BTreeMap::new(),
                },
                _ => BTreeMap::new(),
            }
        }
        None => BTreeMap::new(),
    };

    Ok(diff_file_maps(&head_files, &working_files))
}

/// Summary of accumulated Morph activity (runs, traces, prompts) in the store.
#[derive(Debug, Clone, Default)]
pub struct ActivitySummary {
    pub runs: usize,
    pub traces: usize,
    pub prompts: usize,
}

/// Count accumulated Morph objects by type.
/// Counts from type-index directories under `.morph/` for speed.
pub fn activity_summary(_store: &dyn Store, repo_root: &Path) -> Result<ActivitySummary, MorphError> {
    let morph_dir = repo_root.join(".morph");
    Ok(ActivitySummary {
        runs: count_dir_entries(&morph_dir.join("runs")),
        traces: count_dir_entries(&morph_dir.join("traces")),
        prompts: count_dir_entries(&morph_dir.join("prompts")),
    })
}

fn count_dir_entries(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).filter(|e| e.path().is_file()).count())
        .unwrap_or(0)
}

fn short8(s: &str) -> String { s.chars().take(8).collect() }

/// Build the structured JSON envelope used by `morph status --json` and
/// `morph_status` (MCP). Single source of truth so humans and agents see
/// the same shape.
pub fn build_status_json(repo_root: &Path, store: &dyn Store) -> Result<serde_json::Value, MorphError> {
    let morph_dir = repo_root.join(".morph");
    let changes = working_status(store, repo_root)?;
    let summary = activity_summary(store, repo_root)?;
    let merge = crate::merge_progress_summary(store, &morph_dir).ok().flatten();

    let head_hash = crate::resolve_head(store)?;
    let branch = crate::current_branch(store)?;
    let head = match &head_hash {
        Some(h) => {
            let commit = match store.get(h)? {
                MorphObject::Commit(c) => Some(c),
                _ => None,
            };
            // PR 1: report `effective_metrics` so a late certification
            // shows up in `morph_status` immediately, matching `morph
            // log --json` and the eval-gaps signal.
            let metrics = match &commit {
                Some(c) => Some(crate::policy::effective_metrics_for_commit(store, h, c)?),
                None => None,
            };
            let h_str = h.to_string();
            serde_json::json!({
                "hash": h_str,
                "short": short8(&h_str),
                "message": commit.as_ref().map(|c| c.message.clone()),
                "author": commit.as_ref().map(|c| c.author.clone()),
                "timestamp": commit.as_ref().map(|c| c.timestamp.clone()),
                "metrics": metrics,
            })
        }
        None => serde_json::Value::Null,
    };

    let mut added: Vec<&str> = Vec::new();
    let mut modified: Vec<&str> = Vec::new();
    let mut deleted: Vec<&str> = Vec::new();
    for c in &changes {
        match c.status {
            crate::DiffStatus::Added => added.push(&c.path),
            crate::DiffStatus::Modified => modified.push(&c.path),
            crate::DiffStatus::Deleted => deleted.push(&c.path),
        }
    }

    let staging = crate::read_index(&morph_dir)?;
    let staged_paths: Vec<&String> = staging.entries.keys().collect();

    let policy = crate::read_policy(&morph_dir)?;
    let suite_summary = match policy.default_eval_suite.as_deref() {
        Some(s) => match Hash::from_hex(s).ok().and_then(|h| store.get(&h).ok()) {
            Some(MorphObject::EvalSuite(es)) => serde_json::json!({
                "hash": s,
                "short": short8(s),
                "case_count": es.cases.len(),
                "metric_count": es.metrics.len(),
            }),
            _ => serde_json::json!({ "hash": s, "short": short8(s) }),
        },
        None => serde_json::Value::Null,
    };

    let merge_obj = match merge {
        Some(p) => serde_json::json!({
            "in_progress": true,
            "branch": p.on_branch,
            "unmerged_paths": p.unmerged_paths,
            "pipeline_node_conflicts": p.pipeline_node_conflicts,
        }),
        None => serde_json::json!({ "in_progress": false }),
    };

    Ok(serde_json::json!({
        "repo": repo_root.display().to_string(),
        "branch": branch,
        "detached": branch.is_none() && head_hash.is_some(),
        "head": head,
        "working_tree": {
            "added": added,
            "modified": modified,
            "deleted": deleted,
            "clean": changes.is_empty(),
        },
        "staging": {
            "count": staged_paths.len(),
            "paths": staged_paths,
        },
        "activity": {
            "runs": summary.runs,
            "traces": summary.traces,
            "prompts": summary.prompts,
        },
        "eval_suite": suite_summary,
        "required_metrics": policy.required_metrics,
        "merge": merge_obj,
    }))
}

/// Add path(s) to the store and update the staging index. Works like `git add`:
/// - `"."` stages all working-directory files (excluding `.morph/` internals).
/// - A specific file is staged according to its location (prompt, eval, or blob).
/// - A directory is walked recursively, staging all files within.
pub fn add_paths(
    store: &dyn Store,
    repo_root: &Path,
    paths: &[std::path::PathBuf],
) -> Result<Vec<Hash>, MorphError> {
    let (morph_dir, morph_prompts, morph_evals) = resolve_morph_paths(repo_root);
    let canonical_root = repo_root.canonicalize().unwrap_or_else(|_| repo_root.to_path_buf());
    let morphignore = load_ignore_rules(&canonical_root);
    let mut hashes = Vec::new();
    let mut staged_entries: Vec<(String, Hash)> = Vec::new();

    for p in paths {
        let full = if p.is_absolute() { p.clone() } else { repo_root.join(p) };
        let full = full.canonicalize().unwrap_or(full);

        if full.is_dir() {
            let is_repo_root = full == canonical_root || p.as_os_str() == ".";

            if is_repo_root {
                add_directory(
                    &full,
                    &morph_dir,
                    &morph_prompts,
                    &morph_evals,
                    morphignore.as_ref(),
                    &canonical_root,
                    store,
                    &mut hashes,
                    &mut staged_entries,
                    true,
                )?;
            } else if is_morph_internal(&full, &morph_dir, &morph_prompts, &morph_evals) {
                continue;
            } else {
                add_directory(
                    &full,
                    &morph_dir,
                    &morph_prompts,
                    &morph_evals,
                    morphignore.as_ref(),
                    &canonical_root,
                    store,
                    &mut hashes,
                    &mut staged_entries,
                    true,
                )?;
            }
        } else if full.is_file() {
            if is_morph_internal(&full, &morph_dir, &morph_prompts, &morph_evals) {
                continue;
            }
            if is_ignored(morphignore.as_ref(), &canonical_root, &full, false) {
                continue;
            }
            let kind = classify_file(&full, &morph_prompts, &morph_evals);
            let obj = object_from_file(&full, kind)?;
            let hash = store.put(&obj)?;
            if let Some(rel) = relative_path(&canonical_root, &full) {
                staged_entries.push((rel, hash));
            }
            hashes.push(hash);
        }
    }

    if !staged_entries.is_empty() {
        let mut index = crate::index::read_index(&morph_dir)?;
        for (rel, hash) in &staged_entries {
            index.entries.insert(rel.clone(), hash.to_string());
            // Resolving a path during a merge: drop any unmerged-entry
            // record so `morph merge --continue` can finalize. Mirrors
            // git's behavior where `git add file` removes the conflict
            // markers from the index.
            index.unmerged_entries.remove(rel);
        }
        // Prune stale entries that now match ignore rules (self-healing for old repos).
        index.entries.retain(|rel, _| !is_rel_path_ignored(morphignore.as_ref(), rel, false));
        crate::index::write_index(&morph_dir, &index)?;
    }

    Ok(hashes)
}

fn relative_path(root: &Path, full: &Path) -> Option<String> {
    full.strip_prefix(root)
        .ok()
        .and_then(|p| p.to_str())
        .map(|s| s.replace('\\', "/"))
}

#[allow(clippy::too_many_arguments)] // recursive directory walker; threading
                                      // a context struct here would make
                                      // the recursion noisier than it is now
fn add_directory(
    dir: &Path,
    morph_dir: &Path,
    morph_prompts: &Path,
    morph_evals: &Path,
    morphignore: Option<&ignore::gitignore::Gitignore>,
    repo_root: &Path,
    store: &dyn Store,
    hashes: &mut Vec<Hash>,
    staged_entries: &mut Vec<(String, Hash)>,
    skip_morph: bool,
) -> Result<(), MorphError> {
    for entry in walkdir::WalkDir::new(dir)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| {
            if skip_morph {
                let p = e.path().canonicalize().unwrap_or(e.path().to_path_buf());
                if p == *morph_dir {
                    return false;
                }
            }
            let canonical = e.path().canonicalize().unwrap_or(e.path().to_path_buf());
            !is_ignored(morphignore, repo_root, &canonical, e.file_type().is_dir())
        })
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let canonical = path.canonicalize().unwrap_or(path.to_path_buf());
        if is_morph_internal(&canonical, morph_dir, morph_prompts, morph_evals) {
            continue;
        }
        if is_ignored(morphignore, repo_root, &canonical, false) {
            continue;
        }
        let kind = classify_file(&canonical, morph_prompts, morph_evals);
        let obj = object_from_file(path, kind)?;
        let hash = store.put(&obj)?;
        if let Some(rel) = relative_path(repo_root, &canonical) {
            staged_entries.push((rel, hash));
        }
        hashes.push(hash);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{Blob, MorphObject};
    use crate::repo::init_repo;
    use crate::store::FsStore;

    fn setup_repo() -> (tempfile::TempDir, FsStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = init_repo(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn blob_from_prompt_file_creates_blob() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("p.txt");
        std::fs::write(&f, "Hello world").unwrap();
        let obj = blob_from_prompt_file(&f).unwrap();
        let blob = match &obj {
            MorphObject::Blob(b) => b,
            _ => panic!("expected blob"),
        };
        assert_eq!(blob.kind, "prompt");
        assert_eq!(blob.content.get("body").and_then(|v| v.as_str()), Some("Hello world"));
    }

    #[test]
    fn materialize_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().join("objects"));
        std::fs::create_dir_all(store.objects_dir()).unwrap();
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({ "body": "content here" }),
        });
        let hash = store.put(&blob).unwrap();
        let dest = dir.path().join("out.txt");
        materialize_blob(&store, &hash, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "content here");
    }

    // ── status() tests ───────────────────────────────────────────────

    #[test]
    fn status_shows_working_dir_files() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("README.md"), "hello").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

        let entries = status(&store, root).unwrap();
        let paths: Vec<_> = entries.iter().map(|e| e.path.clone()).collect();
        assert!(paths.iter().any(|p| p.ends_with("README.md")), "should see README.md, got: {:?}", paths);
        assert!(paths.iter().any(|p| p.ends_with("src/main.rs")), "should see src/main.rs, got: {:?}", paths);
        assert!(entries.iter().all(|e| !e.in_store), "new files should not be in store");
    }

    #[test]
    fn status_excludes_morph_prompts() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join(".morph/prompts/p.txt"), "a prompt").unwrap();

        let entries = status(&store, root).unwrap();
        assert!(!entries.iter().any(|e| e.path.to_string_lossy().contains("prompts/p.txt")),
            "should NOT see .morph/prompts/ in status");
    }

    #[test]
    fn status_excludes_morph_evals() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        let eval_json = r#"{"cases":[],"metrics":[{"name":"acc","aggregation":"mean","threshold":0.0}]}"#;
        std::fs::write(root.join(".morph/evals/e.json"), eval_json).unwrap();

        let entries = status(&store, root).unwrap();
        assert!(!entries.iter().any(|e| e.path.to_string_lossy().contains("evals/e.json")),
            "should NOT see .morph/evals/ in status");
    }

    #[test]
    fn status_excludes_morph_internals() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("app.py"), "print('hi')").unwrap();

        let entries = status(&store, root).unwrap();
        for e in &entries {
            let p = e.path.to_string_lossy();
            assert!(!p.contains(".morph/objects"), "should not include objects: {}", p);
            assert!(!p.contains(".morph/refs"), "should not include refs: {}", p);
            assert!(!p.contains(".morph/config.json"), "should not include config: {}", p);
        }
    }

    #[test]
    fn status_excludes_morphignore_paths() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("included.txt"), "yes").unwrap();
        std::fs::write(root.join("skip.txt"), "no").unwrap();
        std::fs::create_dir_all(root.join("vendor")).unwrap();
        std::fs::write(root.join("vendor/lib.rs"), "ignored").unwrap();
        std::fs::write(root.join(".morphignore"), "skip.txt\nvendor/\n").unwrap();

        let entries = status(&store, root).unwrap();
        let paths: Vec<_> = entries.iter().map(|e| e.path.to_string_lossy().into_owned()).collect();
        assert!(paths.iter().any(|p| p.ends_with("included.txt")), "should see included.txt, got: {:?}", paths);
        assert!(!paths.iter().any(|p| p.ends_with("skip.txt")), "should not see skip.txt, got: {:?}", paths);
        assert!(!paths.iter().any(|p| p.contains("vendor")), "should not see vendor/, got: {:?}", paths);
    }

    #[test]
    fn status_after_add_shows_tracked() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("file.txt"), "content").unwrap();

        add_paths(&store, root, &[std::path::PathBuf::from("file.txt")]).unwrap();

        let entries = status(&store, root).unwrap();
        let entry = entries.iter().find(|e| e.path.to_string_lossy().contains("file.txt")).unwrap();
        assert!(entry.in_store, "file should be tracked after add");
    }

    // ── add_paths() tests ────────────────────────────────────────────

    #[test]
    fn add_stages_working_dir_file() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("hello.txt"), "world").unwrap();

        let hashes = add_paths(&store, root, &[std::path::PathBuf::from("hello.txt")]).unwrap();
        assert_eq!(hashes.len(), 1);
        let obj = store.get(&hashes[0]).unwrap();
        match &obj {
            MorphObject::Blob(b) => {
                assert_eq!(b.kind, "blob");
                assert_eq!(b.content.get("body").and_then(|v| v.as_str()), Some("world"));
            }
            _ => panic!("expected blob, got: {:?}", obj),
        }
    }

    #[test]
    fn add_and_materialize_binary_blob() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        let binary: Vec<u8> = (0u8..=255).collect();
        std::fs::write(root.join("data.bin"), &binary).unwrap();

        let hashes = add_paths(&store, root, &[std::path::PathBuf::from("data.bin")]).unwrap();
        assert_eq!(hashes.len(), 1);
        let obj = store.get(&hashes[0]).unwrap();
        let blob = match &obj {
            MorphObject::Blob(b) => b,
            _ => panic!("expected blob"),
        };
        assert_eq!(blob.content.get("encoding").and_then(|v| v.as_str()), Some("base64"));

        let dest = root.join("restored.bin");
        materialize_blob(&store, &hashes[0], &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), binary);
    }

    #[test]
    fn add_stages_prompt_from_morph_prompts() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join(".morph/prompts/p.txt"), "my prompt").unwrap();

        let hashes = add_paths(&store, root, &[std::path::PathBuf::from(".morph/prompts/p.txt")]).unwrap();
        assert_eq!(hashes.len(), 1);
        let obj = store.get(&hashes[0]).unwrap();
        match &obj {
            MorphObject::Blob(b) => assert_eq!(b.kind, "prompt"),
            _ => panic!("expected prompt blob"),
        }
    }

    #[test]
    fn add_stages_eval_from_morph_evals() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        let eval_json = r#"{"cases":[],"metrics":[{"name":"acc","aggregation":"mean","threshold":0.0}]}"#;
        std::fs::write(root.join(".morph/evals/e.json"), eval_json).unwrap();

        let hashes = add_paths(&store, root, &[std::path::PathBuf::from(".morph/evals/e.json")]).unwrap();
        assert_eq!(hashes.len(), 1);
        let obj = store.get(&hashes[0]).unwrap();
        match &obj {
            MorphObject::EvalSuite(_) => {}
            _ => panic!("expected EvalSuite, got: {:?}", obj),
        }
    }

    #[test]
    fn add_dot_stages_working_dir_only() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("app.py"), "print('hi')").unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::write(root.join("lib/util.py"), "pass").unwrap();
        std::fs::write(root.join(".morph/prompts/p.txt"), "prompt text").unwrap();

        let hashes = add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();
        assert_eq!(hashes.len(), 2, "should stage app.py and lib/util.py only, got {}", hashes.len());
    }

    #[test]
    fn add_dot_excludes_morph_internals() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("code.rs"), "fn main(){}").unwrap();

        let count_before = std::fs::read_dir(root.join(".morph/objects")).unwrap().count();
        let hashes = add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();
        assert_eq!(hashes.len(), 1, "should only stage code.rs");

        let count_after = std::fs::read_dir(root.join(".morph/objects")).unwrap().count();
        assert_eq!(count_after - count_before, 1, "only one object should be written");
    }

    #[test]
    fn add_subdirectory() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "a").unwrap();
        std::fs::write(root.join("src/b.rs"), "b").unwrap();
        std::fs::write(root.join("other.txt"), "ignored by this add").unwrap();

        let hashes = add_paths(&store, root, &[std::path::PathBuf::from("src")]).unwrap();
        assert_eq!(hashes.len(), 2, "should stage 2 files from src/");
    }

    #[test]
    fn add_respects_morphignore() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("staged.txt"), "staged").unwrap();
        std::fs::write(root.join("ignored.txt"), "ignored").unwrap();
        std::fs::write(root.join(".morphignore"), "ignored.txt\n").unwrap();

        let hashes = add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();
        // staged.txt and .morphignore are staged; ignored.txt is not
        assert!(!hashes.is_empty() && hashes.len() <= 2, "staged.txt (and optionally .morphignore), got {}", hashes.len());
        let staged: Vec<String> = hashes
            .iter()
            .filter_map(|h| store.get(h).ok())
            .filter_map(|o| match &o { MorphObject::Blob(b) => b.content.get("body").and_then(|v| v.as_str()).map(String::from), _ => None })
            .collect();
        assert!(staged.iter().any(|s| s == "staged"), "staged.txt should be in store, got: {:?}", staged);
        assert!(!staged.iter().any(|s| s == "ignored"), "ignored.txt should not be staged");
    }

    #[test]
    fn add_updates_staging_index() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("hello.txt"), "world").unwrap();

        let hashes = add_paths(&store, root, &[std::path::PathBuf::from("hello.txt")]).unwrap();
        assert_eq!(hashes.len(), 1);

        let morph_dir = root.join(".morph");
        let index = crate::index::read_index(&morph_dir).unwrap();
        assert_eq!(index.entries.len(), 1);
        assert!(index.entries.contains_key("hello.txt"), "index should contain hello.txt");
        assert_eq!(index.entries["hello.txt"], hashes[0].to_string());
    }

    #[test]
    fn add_dot_updates_staging_index_for_all_files() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("a.txt"), "aaa").unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/b.txt"), "bbb").unwrap();

        add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();

        let morph_dir = root.join(".morph");
        let index = crate::index::read_index(&morph_dir).unwrap();
        assert!(index.entries.contains_key("a.txt"), "index should contain a.txt, got: {:?}", index.entries.keys().collect::<Vec<_>>());
        assert!(index.entries.contains_key("sub/b.txt"), "index should contain sub/b.txt, got: {:?}", index.entries.keys().collect::<Vec<_>>());
    }

    // ── built-in ignore tests ────────────────────────────────────────

    #[test]
    fn status_excludes_git_dir_by_default() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("app.py"), "print('hi')").unwrap();
        std::fs::create_dir_all(root.join(".git/objects")).unwrap();
        std::fs::write(root.join(".git/config"), "[core]").unwrap();

        let entries = status(&store, root).unwrap();
        let paths: Vec<_> = entries.iter().map(|e| e.path.to_string_lossy().into_owned()).collect();
        assert!(paths.iter().any(|p| p.ends_with("app.py")), "should see app.py");
        assert!(!paths.iter().any(|p| p.contains(".git")), "should not see .git/, got: {:?}", paths);
    }

    #[test]
    fn status_excludes_node_modules_by_default() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("index.js"), "module.exports = {}").unwrap();
        std::fs::create_dir_all(root.join("node_modules/foo")).unwrap();
        std::fs::write(root.join("node_modules/foo/index.js"), "nope").unwrap();

        let entries = status(&store, root).unwrap();
        let paths: Vec<_> = entries.iter().map(|e| e.path.to_string_lossy().into_owned()).collect();
        assert!(paths.iter().any(|p| p.ends_with("index.js") && !p.contains("node_modules")));
        assert!(!paths.iter().any(|p| p.contains("node_modules")), "should not see node_modules/, got: {:?}", paths);
    }

    #[test]
    fn status_excludes_venv_by_default() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("app.py"), "pass").unwrap();
        std::fs::create_dir_all(root.join(".venv/bin")).unwrap();
        std::fs::write(root.join(".venv/bin/python"), "#!/bin/sh").unwrap();

        let entries = status(&store, root).unwrap();
        assert!(!entries.iter().any(|e| e.path.to_string_lossy().contains(".venv")),
            "should not see .venv/");
    }

    #[test]
    fn status_respects_gitignore() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join(".gitignore"), "secret.key\n").unwrap();
        std::fs::write(root.join("app.py"), "pass").unwrap();
        std::fs::write(root.join("secret.key"), "s3cr3t").unwrap();

        let entries = status(&store, root).unwrap();
        let paths: Vec<_> = entries.iter().map(|e| e.path.to_string_lossy().into_owned()).collect();
        assert!(paths.iter().any(|p| p.ends_with("app.py")));
        assert!(!paths.iter().any(|p| p.ends_with("secret.key")),
            "should not see secret.key (gitignore), got: {:?}", paths);
    }

    #[test]
    fn add_dot_excludes_git_dir() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("code.rs"), "fn main(){}").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), "[core]").unwrap();

        let hashes = add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();
        assert_eq!(hashes.len(), 1, "should only stage code.rs, not .git/config");
    }

    #[test]
    fn add_dot_prunes_stale_ignored_index_entries() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        let morph_dir = root.join(".morph");

        // Simulate stale index from an old binary that tracked .git/ and .venv/
        let mut stale_index = crate::index::StagingIndex::new();
        stale_index.entries.insert(".git/config".into(), "a".repeat(64));
        stale_index.entries.insert(".venv/bin/python".into(), "b".repeat(64));
        stale_index.entries.insert("app.py".into(), "c".repeat(64));
        crate::index::write_index(&morph_dir, &stale_index).unwrap();

        // Now stage with the new binary
        std::fs::write(root.join("app.py"), "print('hello')").unwrap();
        add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();

        let index = crate::index::read_index(&morph_dir).unwrap();
        assert!(index.entries.contains_key("app.py"), "app.py should remain");
        assert!(!index.entries.contains_key(".git/config"),
            "stale .git/config should be pruned, got: {:?}", index.entries.keys().collect::<Vec<_>>());
        assert!(!index.entries.contains_key(".venv/bin/python"),
            "stale .venv/bin/python should be pruned");
    }

    #[test]
    fn working_status_shows_new_files_before_commit() {
        let (dir, store) = setup_repo();
        std::fs::write(dir.path().join("hello.txt"), "world").unwrap();
        let changes = working_status(&store, dir.path()).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "hello.txt");
        assert_eq!(changes[0].status, crate::diff::DiffStatus::Added);
    }

    fn commit_helper(store: &dyn Store, repo_root: &Path, msg: &str) -> Hash {
        crate::commit::create_tree_commit(
            store, repo_root, None, None,
            std::collections::BTreeMap::new(), msg.to_string(), None, None,
        ).unwrap()
    }

    #[test]
    fn working_status_clean_after_commit() {
        let (dir, store) = setup_repo();
        std::fs::write(dir.path().join("hello.txt"), "world").unwrap();
        add_paths(&store, dir.path(), &[std::path::PathBuf::from(".")]).unwrap();
        commit_helper(&store, dir.path(), "initial");
        let changes = working_status(&store, dir.path()).unwrap();
        assert!(changes.is_empty(), "expected clean, got {:?}", changes);
    }

    #[test]
    fn working_status_shows_modified() {
        let (dir, store) = setup_repo();
        std::fs::write(dir.path().join("hello.txt"), "world").unwrap();
        add_paths(&store, dir.path(), &[std::path::PathBuf::from(".")]).unwrap();
        commit_helper(&store, dir.path(), "initial");

        std::fs::write(dir.path().join("hello.txt"), "changed").unwrap();
        let changes = working_status(&store, dir.path()).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].status, crate::diff::DiffStatus::Modified);
    }

    #[test]
    fn working_status_shows_deleted() {
        let (dir, store) = setup_repo();
        std::fs::write(dir.path().join("hello.txt"), "world").unwrap();
        add_paths(&store, dir.path(), &[std::path::PathBuf::from(".")]).unwrap();
        commit_helper(&store, dir.path(), "initial");

        std::fs::remove_file(dir.path().join("hello.txt")).unwrap();
        let changes = working_status(&store, dir.path()).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].status, crate::diff::DiffStatus::Deleted);
    }

    #[test]
    fn activity_summary_counts_dirs() {
        let (dir, store) = setup_repo();
        let summary = activity_summary(&store, dir.path()).unwrap();
        assert_eq!(summary.runs, 0);
        assert_eq!(summary.traces, 0);
        assert_eq!(summary.prompts, 0);

        // Write some files into the type-index directories
        let morph = dir.path().join(".morph");
        std::fs::create_dir_all(morph.join("runs")).unwrap();
        std::fs::write(morph.join("runs/abc.json"), "{}").unwrap();
        std::fs::write(morph.join("runs/def.json"), "{}").unwrap();
        std::fs::create_dir_all(morph.join("traces")).unwrap();
        std::fs::write(morph.join("traces/t1.json"), "{}").unwrap();
        std::fs::create_dir_all(morph.join("prompts")).unwrap();
        std::fs::write(morph.join("prompts/p1.json"), "{}").unwrap();
        std::fs::write(morph.join("prompts/p2.json"), "{}").unwrap();
        std::fs::write(morph.join("prompts/p3.json"), "{}").unwrap();

        let summary = activity_summary(&store, dir.path()).unwrap();
        assert_eq!(summary.runs, 2);
        assert_eq!(summary.traces, 1);
        assert_eq!(summary.prompts, 3);
    }
}
