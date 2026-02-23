//! Staging index: persistent mapping of working-directory paths to blob hashes.
//!
//! Stored as `.morph/index.json`. Updated by `morph add`, read by `morph commit`
//! to build the tree, then cleared after commit.

use crate::store::MorphError;
use std::collections::BTreeMap;
use std::path::Path;

const INDEX_FILE: &str = "index.json";

/// The staging index: relative paths mapped to their blob hashes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StagingIndex {
    pub entries: BTreeMap<String, String>,
}

impl StagingIndex {
    pub fn new() -> Self {
        StagingIndex {
            entries: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for StagingIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// Read the staging index from `.morph/index.json`. Returns empty index if file is missing.
pub fn read_index(morph_dir: &Path) -> Result<StagingIndex, MorphError> {
    let path = morph_dir.join(INDEX_FILE);
    if !path.exists() {
        return Ok(StagingIndex::new());
    }
    let data = std::fs::read_to_string(&path)?;
    let index: StagingIndex =
        serde_json::from_str(&data).map_err(|e| MorphError::Serialization(e.to_string()))?;
    Ok(index)
}

/// Write the staging index to `.morph/index.json`.
pub fn write_index(morph_dir: &Path, index: &StagingIndex) -> Result<(), MorphError> {
    let path = morph_dir.join(INDEX_FILE);
    let json =
        serde_json::to_string_pretty(index).map_err(|e| MorphError::Serialization(e.to_string()))?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Remove the staging index file (called after commit).
pub fn clear_index(morph_dir: &Path) -> Result<(), MorphError> {
    let path = morph_dir.join(INDEX_FILE);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Add or update a single entry in the staging index.
pub fn update_index(
    morph_dir: &Path,
    relative_path: &str,
    blob_hash: &str,
) -> Result<(), MorphError> {
    let mut index = read_index(morph_dir)?;
    index
        .entries
        .insert(relative_path.to_string(), blob_hash.to_string());
    write_index(morph_dir, &index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_missing_index_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let index = read_index(dir.path()).unwrap();
        assert!(index.is_empty());
        assert_eq!(index.entries.len(), 0);
    }

    #[test]
    fn write_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = StagingIndex::new();
        index.entries.insert("src/main.rs".into(), "a".repeat(64));
        index.entries.insert("README.md".into(), "b".repeat(64));
        write_index(dir.path(), &index).unwrap();

        let loaded = read_index(dir.path()).unwrap();
        assert_eq!(loaded, index);
        assert_eq!(loaded.entries.len(), 2);
    }

    #[test]
    fn clear_index_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = StagingIndex::new();
        index.entries.insert("f.txt".into(), "c".repeat(64));
        write_index(dir.path(), &index).unwrap();
        assert!(dir.path().join("index.json").exists());

        clear_index(dir.path()).unwrap();
        assert!(!dir.path().join("index.json").exists());

        let after = read_index(dir.path()).unwrap();
        assert!(after.is_empty());
    }

    #[test]
    fn clear_missing_index_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        clear_index(dir.path()).unwrap();
    }

    #[test]
    fn update_index_adds_entry() {
        let dir = tempfile::tempdir().unwrap();
        update_index(dir.path(), "a.txt", &"d".repeat(64)).unwrap();
        update_index(dir.path(), "b.txt", &"e".repeat(64)).unwrap();

        let index = read_index(dir.path()).unwrap();
        assert_eq!(index.entries.len(), 2);
        assert_eq!(index.entries["a.txt"], "d".repeat(64));
        assert_eq!(index.entries["b.txt"], "e".repeat(64));
    }

    #[test]
    fn update_index_overwrites_existing_entry() {
        let dir = tempfile::tempdir().unwrap();
        update_index(dir.path(), "f.txt", &"a".repeat(64)).unwrap();
        update_index(dir.path(), "f.txt", &"b".repeat(64)).unwrap();

        let index = read_index(dir.path()).unwrap();
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries["f.txt"], "b".repeat(64));
    }
}
