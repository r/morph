//! Working-tree cleanliness check used by `morph merge` (PR 4) to refuse
//! starting a merge that would clobber uncommitted local changes.
//!
//! "Dirty" is defined narrowly: only **modified** and **deleted** entries
//! relative to HEAD's committed tree count as dirty. Untracked files
//! (paths not in HEAD) are tolerated, mirroring git's `merge` behavior —
//! they only become a problem if the merge plan would write to them, and
//! that's a conflict the tree merger surfaces separately as a Textual
//! conflict in PR 4.

use crate::diff::DiffStatus;
use crate::store::{MorphError, Store};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanResult {
    pub clean: bool,
    pub dirty_paths: Vec<String>,
}

/// Compare HEAD's committed tree with the current working directory.
/// Returns the list of tracked paths that have been modified or deleted.
pub fn working_tree_clean(store: &dyn Store, repo_root: &Path) -> Result<CleanResult, MorphError> {
    let changes = crate::working::working_status(store, repo_root)?;
    let dirty_paths: Vec<String> = changes
        .into_iter()
        .filter(|c| matches!(c.status, DiffStatus::Modified | DiffStatus::Deleted))
        .map(|c| c.path)
        .collect();
    Ok(CleanResult {
        clean: dirty_paths.is_empty(),
        dirty_paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::init_repo;
    use crate::store::FsStore;
    use crate::working::add_paths;
    use std::path::PathBuf;

    fn setup_repo() -> (tempfile::TempDir, FsStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = init_repo(dir.path()).unwrap();
        (dir, store)
    }

    fn commit_all(store: &dyn Store, repo_root: &Path, msg: &str) {
        crate::commit::create_tree_commit(
            store,
            repo_root,
            None,
            None,
            std::collections::BTreeMap::new(),
            msg.to_string(),
            None,
            None,
        )
        .unwrap();
    }

    #[test]
    fn working_tree_clean_when_no_changes() {
        let (dir, store) = setup_repo();
        std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
        add_paths(&store, dir.path(), &[PathBuf::from(".")]).unwrap();
        commit_all(&store, dir.path(), "init");

        let result = working_tree_clean(&store, dir.path()).unwrap();
        assert!(
            result.clean,
            "expected clean working tree, got dirty: {:?}",
            result.dirty_paths
        );
        assert!(result.dirty_paths.is_empty());
    }

    #[test]
    fn working_tree_dirty_when_file_modified() {
        let (dir, store) = setup_repo();
        std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
        add_paths(&store, dir.path(), &[PathBuf::from(".")]).unwrap();
        commit_all(&store, dir.path(), "init");

        std::fs::write(dir.path().join("a.txt"), "modified\n").unwrap();
        let result = working_tree_clean(&store, dir.path()).unwrap();
        assert!(!result.clean);
        assert!(
            result.dirty_paths.iter().any(|p| p == "a.txt"),
            "expected a.txt in dirty list: {:?}",
            result.dirty_paths
        );
    }

    #[test]
    fn working_tree_dirty_when_tracked_file_deleted() {
        let (dir, store) = setup_repo();
        std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
        add_paths(&store, dir.path(), &[PathBuf::from(".")]).unwrap();
        commit_all(&store, dir.path(), "init");

        std::fs::remove_file(dir.path().join("a.txt")).unwrap();
        let result = working_tree_clean(&store, dir.path()).unwrap();
        assert!(!result.clean);
        assert!(
            result.dirty_paths.iter().any(|p| p == "a.txt"),
            "deleted a.txt must be reported: {:?}",
            result.dirty_paths
        );
    }

    #[test]
    fn working_tree_clean_when_only_untracked_added() {
        // New file not in HEAD: tolerated by merge gating, mirroring git.
        let (dir, store) = setup_repo();
        std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
        add_paths(&store, dir.path(), &[PathBuf::from(".")]).unwrap();
        commit_all(&store, dir.path(), "init");

        std::fs::write(dir.path().join("new.txt"), "untracked\n").unwrap();
        let result = working_tree_clean(&store, dir.path()).unwrap();
        assert!(
            result.clean,
            "untracked file must not count as dirty for merge gating; got dirty: {:?}",
            result.dirty_paths
        );
    }
}
