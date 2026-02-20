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

/// Compute status: list files under prompts/, programs/, evals/ relative to repo_root and check if each blob exists in store.
pub fn status(store: &dyn Store, repo_root: &Path) -> Result<Vec<StatusEntry>, MorphError> {
    let mut entries = Vec::new();
    for (dir, kind) in [
        ("prompts", "prompt"),
        ("programs", "program"),
        ("evals", "eval"),
    ] {
        let dir_path = repo_root.join(dir);
        if !dir_path.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&dir_path)
            .min_depth(1)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let obj = if kind == "program" {
                program_from_file(path).ok()
            } else if kind == "eval" {
                eval_suite_from_file(path).ok()
            } else {
                blob_from_file(path, kind).ok()
            };
            if let Some(obj) = obj {
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

/// Add path(s) to the store: create objects from files and put them. Paths are relative to repo_root.
/// Path "." means add all of prompts/, programs/, evals/.
pub fn add_paths(
    store: &dyn Store,
    repo_root: &Path,
    paths: &[std::path::PathBuf],
) -> Result<Vec<Hash>, MorphError> {
    let mut hashes = Vec::new();
    for p in paths {
        let full = if p.is_absolute() { p.clone() } else { repo_root.join(p) };
        let full = full.canonicalize().unwrap_or(full);
        if full.is_dir() {
            let to_add: Vec<_> = if full == repo_root || full.file_name().map(|s| s == ".").unwrap_or(false) {
                vec![
                    repo_root.join("prompts"),
                    repo_root.join("programs"),
                    repo_root.join("evals"),
                ]
            } else if full.file_name().map(|s| s == "prompts" || s == "programs" || s == "evals").unwrap_or(false) {
                vec![full.clone()]
            } else {
                continue;
            };
            for dir in to_add {
                if !dir.is_dir() {
                    continue;
                }
                let kind = dir.file_name().and_then(|s| s.to_str()).unwrap_or("prompt");
                let k = if kind == "evals" { "eval" } else { kind.trim_end_matches('s') };
                for entry in walkdir::WalkDir::new(&dir).min_depth(1).max_depth(1).into_iter().filter_map(|e| e.ok()) {
                    if entry.file_type().is_file() {
                        let obj = if k == "program" {
                            program_from_file(entry.path())?
                        } else if k == "eval" {
                            eval_suite_from_file(entry.path())?
                        } else {
                            blob_from_file(entry.path(), k)?
                        };
                        hashes.push(store.put(&obj)?);
                    }
                }
            }
        } else if full.is_file() {
            let parent = full.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str());
            let (obj, _) = match parent {
                Some("prompts") => (blob_from_file(&full, "prompt")?, ()),
                Some("programs") => (program_from_file(&full)?, ()),
                Some("evals") => (eval_suite_from_file(&full)?, ()),
                _ => (blob_from_file(&full, "blob")?, ()),
            };
            hashes.push(store.put(&obj)?);
        }
    }
    Ok(hashes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{Blob, MorphObject};
    use crate::store::FsStore;

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
}
