//! Storage backend: trait and filesystem implementation.

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
pub trait Store {
    fn put(&self, object: &MorphObject) -> Result<Hash, MorphError>;
    fn get(&self, hash: &Hash) -> Result<MorphObject, MorphError>;
    fn has(&self, hash: &Hash) -> Result<bool, MorphError>;
    fn list(&self, type_filter: ObjectType) -> Result<Vec<Hash>, MorphError>;
    fn ref_read(&self, name: &str) -> Result<Option<Hash>, MorphError>;
    fn ref_write(&self, name: &str, hash: &Hash) -> Result<(), MorphError>;
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
                    if parent.is_dir() {
                        let content = match json {
                            Some(ref j) => j.clone(),
                            None => std::fs::read_to_string(&path)?,
                        };
                        std::fs::write(&index_path, content)?;
                    }
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
}
