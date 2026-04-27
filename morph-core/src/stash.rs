//! Stash: save and restore working tree state without committing.
//!
//! Stores the current staging index as a JSON file in `.morph/stashes/`.
//! Each stash is timestamped and can be popped (applied + removed) or listed.

use crate::index::{read_index, write_index, clear_index, StagingIndex};
use crate::store::MorphError;
use std::path::Path;

const STASHES_DIR: &str = "stashes";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StashEntry {
    pub id: String,
    pub message: Option<String>,
    pub timestamp: String,
    pub index: StagingIndex,
}

fn next_stash_id(stashes_dir: &Path, now: chrono::DateTime<chrono::Utc>) -> String {
    // Use nanosecond precision and add a numeric suffix if a file still collides.
    // This keeps IDs stable-looking while guaranteeing uniqueness on fast runners.
    let base = now.format("%Y%m%dT%H%M%S_%9f").to_string();
    let mut candidate = base.clone();
    let mut suffix = 1usize;

    while stashes_dir.join(format!("{}.json", candidate)).exists() {
        candidate = format!("{}_{}", base, suffix);
        suffix += 1;
    }

    candidate
}

/// Save the current staging index as a stash and clear the index.
pub fn stash_save(
    morph_dir: &Path,
    message: Option<&str>,
) -> Result<StashEntry, MorphError> {
    let index = read_index(morph_dir)?;
    if index.is_empty() {
        return Err(MorphError::Serialization(
            "nothing to stash (staging index is empty)".into(),
        ));
    }

    let stashes_dir = morph_dir.join(STASHES_DIR);
    std::fs::create_dir_all(&stashes_dir)?;

    let now = chrono::Utc::now();
    let id = next_stash_id(&stashes_dir, now);
    let entry = StashEntry {
        id: id.clone(),
        message: message.map(|s| s.to_string()),
        timestamp: now.to_rfc3339(),
        index,
    };

    let json = serde_json::to_string_pretty(&entry)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    std::fs::write(stashes_dir.join(format!("{}.json", id)), json)?;

    clear_index(morph_dir)?;
    Ok(entry)
}

/// List all stashes, most recent first.
pub fn stash_list(morph_dir: &Path) -> Result<Vec<StashEntry>, MorphError> {
    let stashes_dir = morph_dir.join(STASHES_DIR);
    if !stashes_dir.is_dir() {
        return Ok(vec![]);
    }
    let mut entries = Vec::new();
    for file in std::fs::read_dir(&stashes_dir)? {
        let file = file?;
        let path = file.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let data = std::fs::read_to_string(&path)?;
        let entry: StashEntry = serde_json::from_str(&data)
            .map_err(|e| MorphError::Serialization(e.to_string()))?;
        entries.push(entry);
    }
    entries.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(entries)
}

/// Pop the most recent stash: restore its index and remove the stash file.
pub fn stash_pop(morph_dir: &Path) -> Result<StashEntry, MorphError> {
    let stashes = stash_list(morph_dir)?;
    let entry = stashes.into_iter().next().ok_or_else(|| {
        MorphError::Serialization("no stashes to pop".into())
    })?;

    write_index(morph_dir, &entry.index)?;

    let stash_file = morph_dir.join(STASHES_DIR).join(format!("{}.json", entry.id));
    std::fs::remove_file(&stash_file)?;

    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::update_index;

    fn setup() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        dir
    }

    #[test]
    fn stash_save_and_pop_roundtrip() {
        let dir = setup();
        let morph_dir = dir.path();
        update_index(morph_dir, "file.txt", &"a".repeat(64)).unwrap();

        let saved = stash_save(morph_dir, Some("wip")).unwrap();
        assert_eq!(saved.message.as_deref(), Some("wip"));
        assert_eq!(saved.index.entries.len(), 1);

        let index_after_save = read_index(morph_dir).unwrap();
        assert!(index_after_save.is_empty(), "index should be cleared after stash");

        let popped = stash_pop(morph_dir).unwrap();
        assert_eq!(popped.id, saved.id);
        assert_eq!(popped.index.entries.len(), 1);

        let index_after_pop = read_index(morph_dir).unwrap();
        assert_eq!(index_after_pop.entries["file.txt"], "a".repeat(64));
    }

    #[test]
    fn stash_save_empty_index_fails() {
        let dir = setup();
        let result = stash_save(dir.path(), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("nothing to stash"));
    }

    #[test]
    fn stash_pop_empty_fails() {
        let dir = setup();
        let result = stash_pop(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no stashes"));
    }

    #[test]
    fn stash_list_shows_entries_most_recent_first() {
        let dir = setup();
        let morph_dir = dir.path();

        update_index(morph_dir, "a.txt", &"a".repeat(64)).unwrap();
        let s1 = stash_save(morph_dir, Some("first")).unwrap();

        update_index(morph_dir, "b.txt", &"b".repeat(64)).unwrap();
        let s2 = stash_save(morph_dir, Some("second")).unwrap();

        let stashes = stash_list(morph_dir).unwrap();
        assert_eq!(stashes.len(), 2);
        assert_eq!(stashes[0].id, s2.id);
        assert_eq!(stashes[1].id, s1.id);
    }

    #[test]
    fn stash_list_empty_dir() {
        let dir = setup();
        let stashes = stash_list(dir.path()).unwrap();
        assert!(stashes.is_empty());
    }

    #[test]
    fn multiple_stash_pop_lifo() {
        let dir = setup();
        let morph_dir = dir.path();

        update_index(morph_dir, "a.txt", &"a".repeat(64)).unwrap();
        stash_save(morph_dir, Some("first")).unwrap();

        update_index(morph_dir, "b.txt", &"b".repeat(64)).unwrap();
        stash_save(morph_dir, Some("second")).unwrap();

        let popped = stash_pop(morph_dir).unwrap();
        assert_eq!(popped.message.as_deref(), Some("second"));

        let popped2 = stash_pop(morph_dir).unwrap();
        assert_eq!(popped2.message.as_deref(), Some("first"));

        assert!(stash_pop(morph_dir).is_err());
    }

    #[test]
    fn stash_save_without_message() {
        let dir = setup();
        update_index(dir.path(), "x.txt", &"x".repeat(64)).unwrap();
        let saved = stash_save(dir.path(), None).unwrap();
        assert!(saved.message.is_none());
    }

    #[test]
    fn next_stash_id_adds_suffix_on_collision() {
        let dir = setup();
        let stashes_dir = dir.path().join(STASHES_DIR);
        std::fs::create_dir_all(&stashes_dir).unwrap();

        let now = chrono::Utc::now();
        let id1 = next_stash_id(&stashes_dir, now);
        std::fs::write(stashes_dir.join(format!("{}.json", id1)), "{}").unwrap();

        let id2 = next_stash_id(&stashes_dir, now);
        assert_ne!(id1, id2);
        assert!(id2.ends_with("_1"));
    }
}
