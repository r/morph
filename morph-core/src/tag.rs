//! Lightweight tags: named references to commits (like git tags).

use crate::store::{MorphError, Store};
use crate::Hash;

const TAGS_PREFIX: &str = "tags/";

/// Create a tag pointing to a commit hash.
pub fn create_tag(store: &dyn Store, name: &str, target: &Hash) -> Result<(), MorphError> {
    let ref_path = format!("{}{}", TAGS_PREFIX, name);
    if store.ref_read(&ref_path)?.is_some() {
        return Err(MorphError::AlreadyExists(format!(
            "tag '{}'",
            name
        )));
    }
    store.ref_write(&ref_path, target)
}

/// List all tags as (name, hash) pairs sorted by name.
pub fn list_tags(store: &dyn Store) -> Result<Vec<(String, Hash)>, MorphError> {
    let refs_dir = store.refs_dir().join("tags");
    if !refs_dir.is_dir() {
        return Ok(vec![]);
    }
    let mut tags = Vec::new();
    for entry in std::fs::read_dir(&refs_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(hash) = store.ref_read(&format!("{}{}", TAGS_PREFIX, name))? {
            tags.push((name, hash));
        }
    }
    tags.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(tags)
}

/// Delete a tag by name.
pub fn delete_tag(store: &dyn Store, name: &str) -> Result<(), MorphError> {
    let ref_path = format!("{}{}", TAGS_PREFIX, name);
    store.ref_delete(&ref_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{Blob, MorphObject};
    use crate::store::FsStore;

    fn setup() -> (tempfile::TempDir, FsStore, Hash) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        std::fs::create_dir_all(path.join("objects")).unwrap();
        std::fs::create_dir_all(path.join("refs/tags")).unwrap();
        let store = FsStore::new(path);
        let blob = MorphObject::Blob(Blob {
            kind: "test".into(),
            content: serde_json::json!({"x": 1}),
        });
        let hash = store.put(&blob).unwrap();
        (dir, store, hash)
    }

    #[test]
    fn create_and_list_tags() {
        let (_dir, store, hash) = setup();
        create_tag(&store, "v1.0", &hash).unwrap();
        create_tag(&store, "v2.0", &hash).unwrap();

        let tags = list_tags(&store).unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].0, "v1.0");
        assert_eq!(tags[0].1, hash);
        assert_eq!(tags[1].0, "v2.0");
    }

    #[test]
    fn create_duplicate_tag_fails() {
        let (_dir, store, hash) = setup();
        create_tag(&store, "v1", &hash).unwrap();
        let result = create_tag(&store, "v1", &hash);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn delete_tag_removes_it() {
        let (_dir, store, hash) = setup();
        create_tag(&store, "v1", &hash).unwrap();
        assert_eq!(list_tags(&store).unwrap().len(), 1);

        delete_tag(&store, "v1").unwrap();
        assert_eq!(list_tags(&store).unwrap().len(), 0);
    }

    #[test]
    fn delete_nonexistent_tag_fails() {
        let (_dir, store, _hash) = setup();
        let result = delete_tag(&store, "nope");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn list_tags_empty_repo() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        let tags = list_tags(&store).unwrap();
        assert!(tags.is_empty());
    }
}
