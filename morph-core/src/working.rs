//! Working-space operations: create blobs from files, materialize, status, add.

use crate::objects::{Blob, EvalSuite, MorphObject, Program};
use crate::Hash;
use crate::store::{MorphError, Store};
use crate::content_hash;
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

/// Create a Blob from file with given kind. Content is {"body": "<utf8 contents>"}.
pub fn blob_from_file(path: &Path, kind: &str) -> Result<MorphObject, MorphError> {
    let body = std::fs::read_to_string(path)?;
    let content = serde_json::json!({ "body": body });
    Ok(MorphObject::Blob(Blob {
        kind: kind.to_string(),
        content,
    }))
}

/// Materialize a Blob from the store to a file path. Extracts "body" from content or whole content as JSON string.
pub fn materialize_blob(store: &dyn Store, hash: &Hash, dest: &Path) -> Result<(), MorphError> {
    let obj = store.get(hash)?;
    let blob = match &obj {
        MorphObject::Blob(b) => b,
        _ => return Err(MorphError::Serialization("object is not a blob".into())),
    };
    let body = blob
        .content
        .get("body")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| serde_json::to_string(&blob.content).unwrap_or_default());
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dest, body)?;
    Ok(())
}

/// Parse a Program from a JSON file.
pub fn program_from_file(path: &Path) -> Result<MorphObject, MorphError> {
    let s = std::fs::read_to_string(path)?;
    let program: Program = serde_json::from_str(&s).map_err(|e| MorphError::Serialization(e.to_string()))?;
    Ok(MorphObject::Program(program))
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

/// Compute status: scan the working directory and `.morph/prompts/`, `.morph/evals/`.
/// Files inside `.morph/` internals (objects, refs, etc.) are excluded.
pub fn status(store: &dyn Store, repo_root: &Path) -> Result<Vec<StatusEntry>, MorphError> {
    let mut entries = Vec::new();
    let (morph_dir, morph_prompts, morph_evals) = resolve_morph_paths(repo_root);

    for entry in walkdir::WalkDir::new(repo_root)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| {
            let p = e.path();
            let canonical = p.canonicalize().unwrap_or(p.to_path_buf());
            canonical != morph_dir
        })
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let kind = classify_file(path, &morph_prompts, &morph_evals);
        if let Some(obj) = object_from_file(path, kind).ok() {
            let hash = content_hash(&obj)?;
            let in_store = store.has(&hash)?;
            entries.push(StatusEntry {
                path: path.to_path_buf(),
                in_store,
                hash: Some(hash),
            });
        }
    }

    for dir_path in &[&morph_prompts, &morph_evals] {
        if !dir_path.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(dir_path)
            .min_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let kind = classify_file(path, &morph_prompts, &morph_evals);
            if let Some(obj) = object_from_file(path, kind).ok() {
                let hash = content_hash(&obj)?;
                let in_store = store.has(&hash)?;
                entries.push(StatusEntry {
                    path: path.to_path_buf(),
                    in_store,
                    hash: Some(hash),
                });
            }
        }
    }

    Ok(entries)
}

/// Add path(s) to the store. Works like `git add`:
/// - `"."` stages all working-directory files (excluding `.morph/` internals)
///   plus `.morph/prompts/*` and `.morph/evals/*`.
/// - A specific file is staged according to its location (prompt, eval, or blob).
/// - A directory is walked recursively, staging all files within.
pub fn add_paths(
    store: &dyn Store,
    repo_root: &Path,
    paths: &[std::path::PathBuf],
) -> Result<Vec<Hash>, MorphError> {
    let (morph_dir, morph_prompts, morph_evals) = resolve_morph_paths(repo_root);
    let canonical_root = repo_root.canonicalize().unwrap_or(repo_root.to_path_buf());
    let mut hashes = Vec::new();

    for p in paths {
        let full = if p.is_absolute() { p.clone() } else { repo_root.join(p) };
        let full = full.canonicalize().unwrap_or(full);

        if full.is_dir() {
            let is_repo_root = full == canonical_root || p.as_os_str() == ".";

            if is_repo_root {
                add_directory(&full, &morph_dir, &morph_prompts, &morph_evals, store, &mut hashes, true)?;
                for md in &[&morph_prompts, &morph_evals] {
                    if md.is_dir() {
                        add_directory(md, &morph_dir, &morph_prompts, &morph_evals, store, &mut hashes, false)?;
                    }
                }
            } else if is_morph_internal(&full, &morph_dir, &morph_prompts, &morph_evals) {
                continue;
            } else {
                add_directory(&full, &morph_dir, &morph_prompts, &morph_evals, store, &mut hashes, true)?;
            }
        } else if full.is_file() {
            if is_morph_internal(&full, &morph_dir, &morph_prompts, &morph_evals) {
                continue;
            }
            let kind = classify_file(&full, &morph_prompts, &morph_evals);
            let obj = object_from_file(&full, kind)?;
            hashes.push(store.put(&obj)?);
        }
    }
    Ok(hashes)
}

fn add_directory(
    dir: &Path,
    morph_dir: &Path,
    morph_prompts: &Path,
    morph_evals: &Path,
    store: &dyn Store,
    hashes: &mut Vec<Hash>,
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
            true
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
        let kind = classify_file(&canonical, morph_prompts, morph_evals);
        let obj = object_from_file(path, kind)?;
        hashes.push(store.put(&obj)?);
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
    fn status_shows_morph_prompts() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join(".morph/prompts/p.txt"), "a prompt").unwrap();

        let entries = status(&store, root).unwrap();
        assert!(entries.iter().any(|e| e.path.to_string_lossy().contains("prompts/p.txt")),
            "should see .morph/prompts/p.txt, got: {:?}", entries.iter().map(|e| &e.path).collect::<Vec<_>>());
    }

    #[test]
    fn status_shows_morph_evals() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        let eval_json = r#"{"cases":[],"metrics":[{"name":"acc","aggregation":"mean","threshold":0.0}]}"#;
        std::fs::write(root.join(".morph/evals/e.json"), eval_json).unwrap();

        let entries = status(&store, root).unwrap();
        assert!(entries.iter().any(|e| e.path.to_string_lossy().contains("evals/e.json")),
            "should see .morph/evals/e.json, got: {:?}", entries.iter().map(|e| &e.path).collect::<Vec<_>>());
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
    fn add_dot_stages_all_working_dir_and_morph_metadata() {
        let (dir, store) = setup_repo();
        let root = dir.path();
        std::fs::write(root.join("app.py"), "print('hi')").unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::write(root.join("lib/util.py"), "pass").unwrap();
        std::fs::write(root.join(".morph/prompts/p.txt"), "prompt text").unwrap();

        let hashes = add_paths(&store, root, &[std::path::PathBuf::from(".")]).unwrap();
        assert!(hashes.len() >= 3, "should stage at least 3 files, got {}", hashes.len());
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
}
