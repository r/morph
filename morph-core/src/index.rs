//! Staging index: persistent mapping of working-directory paths to blob hashes.
//!
//! Stored as `.morph/index.json`. Updated by `morph add`, read by `morph commit`
//! to build the tree, then cleared after commit.

use crate::store::MorphError;
use std::collections::BTreeMap;
use std::path::Path;

const INDEX_FILE: &str = "index.json";

/// The staging index: relative paths mapped to their blob hashes.
///
/// Optionally carries `unmerged_entries` during an in-progress merge. The
/// field is `#[serde(default)]` and skip-serialized when empty so that an
/// old morph binary reading a clean (no-merge-in-progress) index sees JSON
/// byte-identical to the pre-PR-3 format.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StagingIndex {
    pub entries: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unmerged_entries: BTreeMap<String, UnmergedEntry>,
}

/// One unmerged path during a 3-way merge. Each side may be absent
/// (representing add/delete on that side).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnmergedEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_blob: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ours_blob: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theirs_blob: Option<String>,
}

impl StagingIndex {
    pub fn new() -> Self {
        StagingIndex {
            entries: BTreeMap::new(),
            unmerged_entries: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.unmerged_entries.is_empty()
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

// ── unmerged-entries API (PR 3) ───────────────────────────────────────

/// Record a path as unmerged, capturing the three sides of the conflict
/// so `morph status` and `morph merge --continue` (PR 4) can describe and
/// resolve them. Removes any normal entry at the same path.
pub fn mark_unmerged(
    morph_dir: &Path,
    relative_path: &str,
    entry: UnmergedEntry,
) -> Result<(), MorphError> {
    let mut index = read_index(morph_dir)?;
    index.entries.remove(relative_path);
    index
        .unmerged_entries
        .insert(relative_path.to_string(), entry);
    write_index(morph_dir, &index)
}

/// Mark a path as resolved: drops the unmerged entry and stages the
/// resolved blob as a normal entry.
pub fn resolve_unmerged(
    morph_dir: &Path,
    relative_path: &str,
    blob_hash: &str,
) -> Result<(), MorphError> {
    let mut index = read_index(morph_dir)?;
    index.unmerged_entries.remove(relative_path);
    index
        .entries
        .insert(relative_path.to_string(), blob_hash.to_string());
    write_index(morph_dir, &index)
}

/// Return all unmerged paths, sorted (BTreeMap keys are already sorted).
pub fn unmerged_paths(morph_dir: &Path) -> Result<Vec<String>, MorphError> {
    let index = read_index(morph_dir)?;
    Ok(index.unmerged_entries.keys().cloned().collect())
}

/// True when the staging index has at least one unmerged entry.
pub fn has_unmerged(morph_dir: &Path) -> Result<bool, MorphError> {
    let index = read_index(morph_dir)?;
    Ok(!index.unmerged_entries.is_empty())
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

    // ── unmerged_entries (PR 3) ──────────────────────────────────────

    #[test]
    fn index_reads_old_format_without_unmerged_field() {
        // Simulate a pre-PR-3 index file: only the `entries` key, no
        // `unmerged_entries`. New binary must load it cleanly.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(INDEX_FILE);
        let legacy = format!(
            "{{\"entries\":{{\"a.txt\":\"{}\"}}}}\n",
            "a".repeat(64)
        );
        std::fs::write(&path, legacy).unwrap();
        let index = read_index(dir.path()).unwrap();
        assert_eq!(index.entries.len(), 1);
        assert!(index.unmerged_entries.is_empty());
    }

    #[test]
    fn index_writes_omit_unmerged_when_empty() {
        // Fresh index with no unmerged_entries must serialize without
        // that key, so old binaries see byte-identical JSON.
        let dir = tempfile::tempdir().unwrap();
        let mut index = StagingIndex::new();
        index.entries.insert("a.txt".into(), "a".repeat(64));
        write_index(dir.path(), &index).unwrap();
        let raw = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        assert!(
            !raw.contains("unmerged_entries"),
            "empty unmerged map must be skip_serialize-d, got:\n{}",
            raw
        );
    }

    #[test]
    fn mark_unmerged_persists_entry_with_three_blobs() {
        let dir = tempfile::tempdir().unwrap();
        // Stage a normal entry first; it must be removed when marked unmerged.
        update_index(dir.path(), "a.txt", &"a".repeat(64)).unwrap();
        let entry = UnmergedEntry {
            base_blob: Some("b".repeat(64)),
            ours_blob: Some("c".repeat(64)),
            theirs_blob: Some("d".repeat(64)),
        };
        mark_unmerged(dir.path(), "a.txt", entry.clone()).unwrap();

        let index = read_index(dir.path()).unwrap();
        assert!(!index.entries.contains_key("a.txt"), "normal entry must be cleared");
        assert_eq!(index.unmerged_entries["a.txt"], entry);
        assert!(has_unmerged(dir.path()).unwrap());
        assert_eq!(unmerged_paths(dir.path()).unwrap(), vec!["a.txt".to_string()]);
    }

    #[test]
    fn resolve_unmerged_clears_entry_and_writes_normal_entry() {
        let dir = tempfile::tempdir().unwrap();
        mark_unmerged(
            dir.path(),
            "a.txt",
            UnmergedEntry {
                base_blob: None,
                ours_blob: Some("c".repeat(64)),
                theirs_blob: Some("d".repeat(64)),
            },
        )
        .unwrap();
        assert!(has_unmerged(dir.path()).unwrap());

        let resolved = "f".repeat(64);
        resolve_unmerged(dir.path(), "a.txt", &resolved).unwrap();
        let index = read_index(dir.path()).unwrap();
        assert!(index.unmerged_entries.is_empty());
        assert_eq!(index.entries["a.txt"], resolved);
        assert!(!has_unmerged(dir.path()).unwrap());
    }
}
