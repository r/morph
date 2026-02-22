//! Storage backend: trait and filesystem implementation.
//!
//! - [FsStore]: 0.0 layout, hash = SHA-256(canonical_json).
//! - [GixStore]: 0.2 layout, hash = Git format SHA-256("blob "+len+"\0"+canonical_json). Same dir layout as FsStore.

use crate::hash::Hash;
use crate::objects::MorphObject;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum MorphError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization: {0}")]
    Serialization(String),
    #[error("Invalid hash: {0}")]
    InvalidHash(String),
    #[error("Object not found: {0}")]
    NotFound(String),
    #[error("Not a morph repository")]
    NotRepo,
    /// Repo store version is older than what this tool supports; user must run `morph upgrade` (CLI only).
    #[error("Upgrade required: {0}")]
    UpgradeRequired(String),
}

/// Object type filter for list operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectType {
    Blob,
    Tree,
    Program,
    EvalSuite,
    Commit,
    Run,
    Artifact,
    Trace,
    TraceRollup,
    Annotation,
}

impl MorphObject {
    pub fn object_type(&self) -> ObjectType {
        match self {
            MorphObject::Blob(_) => ObjectType::Blob,
            MorphObject::Tree(_) => ObjectType::Tree,
            MorphObject::Program(_) => ObjectType::Program,
            MorphObject::EvalSuite(_) => ObjectType::EvalSuite,
            MorphObject::Commit(_) => ObjectType::Commit,
            MorphObject::Run(_) => ObjectType::Run,
            MorphObject::Artifact(_) => ObjectType::Artifact,
            MorphObject::Trace(_) => ObjectType::Trace,
            MorphObject::TraceRollup(_) => ObjectType::TraceRollup,
            MorphObject::Annotation(_) => ObjectType::Annotation,
        }
    }
}

/// Abstract storage interface (v0-spec §3).
/// For refs: ref_read/ref_write work with resolved hashes; ref_read_raw/ref_write_raw
/// work with raw ref content (e.g. "ref: heads/main" for symbolic HEAD).
pub trait Store {
    fn put(&self, object: &MorphObject) -> Result<Hash, MorphError>;
    fn get(&self, hash: &Hash) -> Result<MorphObject, MorphError>;
    fn has(&self, hash: &Hash) -> Result<bool, MorphError>;
    fn list(&self, type_filter: ObjectType) -> Result<Vec<Hash>, MorphError>;
    fn ref_read(&self, name: &str) -> Result<Option<Hash>, MorphError>;
    fn ref_write(&self, name: &str, hash: &Hash) -> Result<(), MorphError>;
    /// Raw ref content (e.g. "ref: heads/main\n" or a hash string). Used for HEAD.
    fn ref_read_raw(&self, name: &str) -> Result<Option<String>, MorphError>;
    /// Write raw ref content (symbolic or hash).
    fn ref_write_raw(&self, name: &str, value: &str) -> Result<(), MorphError>;
    /// Path to refs directory (e.g. for listing branches).
    fn refs_dir(&self) -> std::path::PathBuf;
}

impl Store for Box<dyn Store + '_> {
    fn put(&self, object: &MorphObject) -> Result<Hash, MorphError> {
        self.as_ref().put(object)
    }
    fn get(&self, hash: &Hash) -> Result<MorphObject, MorphError> {
        self.as_ref().get(hash)
    }
    fn has(&self, hash: &Hash) -> Result<bool, MorphError> {
        self.as_ref().has(hash)
    }
    fn list(&self, type_filter: ObjectType) -> Result<Vec<Hash>, MorphError> {
        self.as_ref().list(type_filter)
    }
    fn ref_read(&self, name: &str) -> Result<Option<Hash>, MorphError> {
        self.as_ref().ref_read(name)
    }
    fn ref_write(&self, name: &str, hash: &Hash) -> Result<(), MorphError> {
        self.as_ref().ref_write(name, hash)
    }
    fn ref_read_raw(&self, name: &str) -> Result<Option<String>, MorphError> {
        self.as_ref().ref_read_raw(name)
    }
    fn ref_write_raw(&self, name: &str, value: &str) -> Result<(), MorphError> {
        self.as_ref().ref_write_raw(name, value)
    }
    fn refs_dir(&self) -> std::path::PathBuf {
        self.as_ref().refs_dir()
    }
}

/// Filesystem-backed store. Objects at `root/objects/<hash>.json`, refs at `root/refs/`.
pub struct FsStore {
    root: std::path::PathBuf,
}

impl FsStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        FsStore {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn objects_dir(&self) -> std::path::PathBuf {
        self.root.join("objects")
    }

    pub fn refs_dir(&self) -> std::path::PathBuf {
        self.root.join("refs")
    }

    fn object_path(&self, hash: &Hash) -> std::path::PathBuf {
        self.objects_dir().join(format!("{}.json", hash))
    }
}

fn type_index_dir(object: &MorphObject) -> Option<&'static str> {
    match object {
        MorphObject::Run(_) => Some("runs"),
        MorphObject::Trace(_) => Some("traces"),
        MorphObject::EvalSuite(_) => Some("evals"),
        MorphObject::Blob(b) if b.kind == "prompt" => Some("prompts"),
        _ => None,
    }
}

impl Store for FsStore {
    fn put(&self, object: &MorphObject) -> Result<Hash, MorphError> {
        let hash = crate::content_hash(object)?;
        let path = self.object_path(&hash);
        let json = if path.exists() {
            None
        } else {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let json = crate::canonical_json(object)?;
            std::fs::write(&path, &json)?;
            Some(json)
        };

        if let Some(dir_name) = type_index_dir(object) {
            let index_path = self.root.join(dir_name).join(format!("{}.json", hash));
            if !index_path.exists() {
                if let Some(parent) = index_path.parent() {
                    std::fs::create_dir_all(parent)?;
                    let content = match json {
                        Some(ref j) => j.clone(),
                        None => std::fs::read_to_string(&path)?,
                    };
                    std::fs::write(&index_path, content)?;
                }
            }
        }

        Ok(hash)
    }

    fn get(&self, hash: &Hash) -> Result<MorphObject, MorphError> {
        let path = self.object_path(hash);
        let bytes = std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                MorphError::NotFound(hash.to_string())
            } else {
                MorphError::Io(e)
            }
        })?;
        let obj: MorphObject = serde_json::from_slice(&bytes)
            .map_err(|e| MorphError::Serialization(e.to_string()))?;
        Ok(obj)
    }

    fn has(&self, hash: &Hash) -> Result<bool, MorphError> {
        Ok(self.object_path(hash).exists())
    }

    fn list(&self, type_filter: ObjectType) -> Result<Vec<Hash>, MorphError> {
        let dir = self.objects_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut hashes = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if name.len() != 64 {
                continue;
            }
            let hash = Hash::from_hex(name).map_err(|_| MorphError::InvalidHash(name.into()))?;
            let obj = self.get(&hash)?;
            if obj.object_type() == type_filter {
                hashes.push(hash);
            }
        }
        Ok(hashes)
    }

    fn ref_read(&self, name: &str) -> Result<Option<Hash>, MorphError> {
        let path = self.refs_dir().join(name);
        if !path.exists() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(&path)?.trim().to_string();
        if s.is_empty() {
            return Ok(None);
        }
        let hash = Hash::from_hex(&s)?;
        Ok(Some(hash))
    }

    fn ref_write(&self, name: &str, hash: &Hash) -> Result<(), MorphError> {
        let path = self.refs_dir().join(name);
        if let Some(parent) = path.parent() {
            if path != self.refs_dir() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(&path, hash.to_string())?;
        Ok(())
    }

    fn ref_read_raw(&self, name: &str) -> Result<Option<String>, MorphError> {
        let path = self.refs_dir().join(name);
        if !path.exists() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(&path)?.trim().to_string();
        Ok(if s.is_empty() { None } else { Some(s) })
    }

    fn ref_write_raw(&self, name: &str, value: &str) -> Result<(), MorphError> {
        let path = self.refs_dir().join(name);
        if let Some(parent) = path.parent() {
            if path != self.refs_dir() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let content = if value.ends_with('\n') { value.to_string() } else { format!("{}\n", value) };
        std::fs::write(&path, content)?;
        Ok(())
    }

    fn refs_dir(&self) -> std::path::PathBuf {
        self.root.join("refs")
    }
}

/// 0.2 store: same directory layout as FsStore but uses Git-format content hash
/// (SHA-256 of "blob "+len+"\0"+canonical_json). Used for repo_version "0.2".
pub struct GixStore {
    root: std::path::PathBuf,
}

impl GixStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        GixStore {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn objects_dir(&self) -> std::path::PathBuf {
        self.root.join("objects")
    }

    fn object_path(&self, hash: &Hash) -> std::path::PathBuf {
        self.objects_dir().join(format!("{}.json", hash))
    }
}

fn type_index_dir_gix(object: &MorphObject) -> Option<&'static str> {
    type_index_dir(object)
}

impl Store for GixStore {
    fn put(&self, object: &MorphObject) -> Result<Hash, MorphError> {
        let hash = crate::content_hash_git(object)?;
        let path = self.object_path(&hash);
        if path.exists() {
            return Ok(hash);
        }
        std::fs::create_dir_all(path.parent().unwrap())?;
        let json = crate::canonical_json(object)?;
        std::fs::write(&path, &json)?;

        if let Some(dir_name) = type_index_dir_gix(object) {
            let index_path = self.root.join(dir_name).join(format!("{}.json", hash));
            if !index_path.exists() {
                if let Some(parent) = index_path.parent() {
                    std::fs::create_dir_all(parent)?;
                    std::fs::write(&index_path, &json)?;
                }
            }
        }

        Ok(hash)
    }

    fn get(&self, hash: &Hash) -> Result<MorphObject, MorphError> {
        let path = self.object_path(hash);
        let bytes = std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                MorphError::NotFound(hash.to_string())
            } else {
                MorphError::Io(e)
            }
        })?;
        let obj: MorphObject =
            serde_json::from_slice(&bytes).map_err(|e| MorphError::Serialization(e.to_string()))?;
        Ok(obj)
    }

    fn has(&self, hash: &Hash) -> Result<bool, MorphError> {
        Ok(self.object_path(hash).exists())
    }

    fn list(&self, type_filter: ObjectType) -> Result<Vec<Hash>, MorphError> {
        let dir = self.objects_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut hashes = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if name.len() != 64 {
                continue;
            }
            let hash = Hash::from_hex(name).map_err(|_| MorphError::InvalidHash(name.into()))?;
            let obj = self.get(&hash)?;
            if obj.object_type() == type_filter {
                hashes.push(hash);
            }
        }
        Ok(hashes)
    }

    fn ref_read(&self, name: &str) -> Result<Option<Hash>, MorphError> {
        let path = self.root.join("refs").join(name);
        if !path.exists() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(&path)?.trim().to_string();
        if s.is_empty() {
            return Ok(None);
        }
        let hash = Hash::from_hex(&s)?;
        Ok(Some(hash))
    }

    fn ref_write(&self, name: &str, hash: &Hash) -> Result<(), MorphError> {
        let path = self.root.join("refs").join(name);
        if let Some(parent) = path.parent() {
            if path != self.root.join("refs") {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(&path, hash.to_string())?;
        Ok(())
    }

    fn ref_read_raw(&self, name: &str) -> Result<Option<String>, MorphError> {
        let path = self.root.join("refs").join(name);
        if !path.exists() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(&path)?.trim().to_string();
        Ok(if s.is_empty() { None } else { Some(s) })
    }

    fn ref_write_raw(&self, name: &str, value: &str) -> Result<(), MorphError> {
        let path = self.root.join("refs").join(name);
        if let Some(parent) = path.parent() {
            if path != self.root.join("refs") {
                std::fs::create_dir_all(parent)?;
            }
        }
        let content = if value.ends_with('\n') {
            value.to_string()
        } else {
            format!("{}\n", value)
        };
        std::fs::write(&path, content)?;
        Ok(())
    }

    fn refs_dir(&self) -> std::path::PathBuf {
        self.root.join("refs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::*;

    #[test]
    fn put_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"x": 1}),
        });
        let hash = store.put(&blob).unwrap();
        let got = store.get(&hash).unwrap();
        assert!(matches!(got, MorphObject::Blob(_)));
        assert!(store.has(&hash).unwrap());
    }

    #[test]
    fn put_prompt_blob_creates_type_index_even_without_prompts_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("objects")).unwrap();
        let store = FsStore::new(dir.path());
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"text": "hello"}),
        });
        let hash = store.put(&blob).unwrap();
        let prompts_dir = dir.path().join("prompts");
        assert!(prompts_dir.is_dir(), "put() should create prompts/ when missing");
        let index_file = prompts_dir.join(format!("{}.json", hash));
        assert!(index_file.is_file(), "prompts/<hash>.json should exist");
    }

    #[test]
    fn ref_write_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        store.refs_dir();
        std::fs::create_dir_all(store.refs_dir()).unwrap();
        let blob = MorphObject::Blob(Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let hash = store.put(&blob).unwrap();
        store.ref_write("heads/main", &hash).unwrap();
        let read = store.ref_read("heads/main").unwrap();
        assert_eq!(read, Some(hash));
    }

    #[test]
    fn get_missing_returns_not_found() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("objects")).unwrap();
        let store = FsStore::new(dir.path());
        let hash = Hash::from_hex(&"0".repeat(64)).unwrap();
        let err = store.get(&hash).unwrap_err();
        assert!(matches!(err, MorphError::NotFound(_)));
    }

    #[test]
    fn list_filters_by_object_type() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"body": "x"}),
        });
        let tree = MorphObject::Tree(Tree {
            entries: vec![TreeEntry {
                name: "f".into(),
                hash: "0".repeat(64),
            }],
        });
        let blob_hash = store.put(&blob).unwrap();
        let tree_hash = store.put(&tree).unwrap();

        let blobs = store.list(ObjectType::Blob).unwrap();
        assert!(blobs.contains(&blob_hash));
        assert!(!blobs.contains(&tree_hash));

        let trees = store.list(ObjectType::Tree).unwrap();
        assert!(trees.contains(&tree_hash));
        assert!(!trees.contains(&blob_hash));
    }

    #[test]
    fn ref_read_raw_ref_write_raw_symbolic_head() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        std::fs::create_dir_all(store.refs_dir()).unwrap();
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();
        let raw = store.ref_read_raw("HEAD").unwrap();
        assert!(raw.as_deref().map(|s| s.contains("ref:")).unwrap_or(false));
        assert!(raw.as_deref().map(|s| s.contains("heads/main")).unwrap_or(false));
    }

    #[test]
    fn ref_read_raw_after_ref_write_resolves_to_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        std::fs::create_dir_all(store.refs_dir()).unwrap();
        let blob = MorphObject::Blob(Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let hash = store.put(&blob).unwrap();
        store.ref_write("heads/main", &hash).unwrap();
        let raw = store.ref_read_raw("heads/main").unwrap();
        assert_eq!(raw.as_deref(), Some(hash.to_string().as_str()));
        assert_eq!(store.ref_read("heads/main").unwrap(), Some(hash));
    }

    // --- GixStore: same Store contract, Git-format hash ---

    #[test]
    fn gix_store_put_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = GixStore::new(dir.path());
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"x": 1}),
        });
        let hash = store.put(&blob).unwrap();
        let got = store.get(&hash).unwrap();
        assert!(matches!(got, MorphObject::Blob(_)));
        assert!(store.has(&hash).unwrap());
        // Git-format hash differs from legacy content_hash
        let legacy_hash = crate::content_hash(&blob).unwrap();
        assert_ne!(hash, legacy_hash);
    }

    #[test]
    fn gix_store_ref_write_read_and_symbolic_head() {
        let dir = tempfile::tempdir().unwrap();
        let store = GixStore::new(dir.path());
        std::fs::create_dir_all(store.refs_dir()).unwrap();
        let blob = MorphObject::Blob(Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let hash = store.put(&blob).unwrap();
        store.ref_write("heads/main", &hash).unwrap();
        assert_eq!(store.ref_read("heads/main").unwrap(), Some(hash));
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();
        let raw = store.ref_read_raw("HEAD").unwrap();
        assert!(raw.as_deref().map(|s| s.contains("ref:")).unwrap_or(false));
    }

    #[test]
    fn gix_store_list_filters_by_type() {
        let dir = tempfile::tempdir().unwrap();
        let store = GixStore::new(dir.path());
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"body": "x"}),
        });
        let tree = MorphObject::Tree(Tree {
            entries: vec![TreeEntry {
                name: "f".into(),
                hash: "0".repeat(64),
            }],
        });
        let blob_hash = store.put(&blob).unwrap();
        let tree_hash = store.put(&tree).unwrap();
        let blobs = store.list(ObjectType::Blob).unwrap();
        assert!(blobs.contains(&blob_hash));
        assert!(!blobs.contains(&tree_hash));
        let trees = store.list(ObjectType::Tree).unwrap();
        assert!(trees.contains(&tree_hash));
        assert!(!trees.contains(&blob_hash));
    }
}
