//! Repository operations: init, directory layout.

use crate::store::{FsStore, MorphError};
use std::path::Path;

/// Directory names under .morph/
const OBJECTS_DIR: &str = "objects";
const REFS_HEADS_DIR: &str = "refs/heads";
const RUNS_DIR: &str = "runs";
const TRACES_DIR: &str = "traces";
const CONFIG_FILE: &str = "config.json";
const PROMPTS_DIR: &str = "prompts";
const EVALS_DIR: &str = "evals";

/// Initialize a Morph repository at `root`. Creates only `.morph/` — the
/// working directory itself is the user's project and is not modified.
pub fn init_repo(root: impl AsRef<Path>) -> Result<FsStore, MorphError> {
    let root = root.as_ref();
    let morph_dir = root.join(".morph");

    if morph_dir.exists() {
        let meta = std::fs::metadata(&morph_dir).map_err(MorphError::Io)?;
        if !meta.is_dir() {
            return Err(MorphError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                ".morph exists and is not a directory",
            )));
        }
        return Ok(FsStore::new(&morph_dir));
    }

    std::fs::create_dir_all(morph_dir.join(OBJECTS_DIR))?;
    std::fs::create_dir_all(morph_dir.join(REFS_HEADS_DIR))?;
    std::fs::create_dir_all(morph_dir.join(RUNS_DIR))?;
    std::fs::create_dir_all(morph_dir.join(TRACES_DIR))?;
    std::fs::create_dir_all(morph_dir.join(PROMPTS_DIR))?;
    std::fs::create_dir_all(morph_dir.join(EVALS_DIR))?;

    let config = serde_json::json!({});
    std::fs::write(morph_dir.join(CONFIG_FILE), serde_json::to_string_pretty(&config).unwrap())?;

    std::fs::write(morph_dir.join("refs").join("HEAD"), "ref: heads/main\n")?;

    Ok(FsStore::new(morph_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_morph_internals() {
        let dir = tempfile::tempdir().unwrap();
        let store = init_repo(dir.path()).unwrap();
        assert!(store.objects_dir().exists());
        assert!(store.refs_dir().exists());
        assert!(dir.path().join(".morph/config.json").exists());
        assert!(dir.path().join(".morph/refs/HEAD").exists());
    }

    #[test]
    fn init_creates_prompts_and_evals_under_morph() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        assert!(dir.path().join(".morph/prompts").is_dir());
        assert!(dir.path().join(".morph/evals").is_dir());
    }

    #[test]
    fn init_does_not_create_top_level_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        assert!(!dir.path().join("prompts").exists(), "top-level prompts/ should not exist");
        assert!(!dir.path().join("programs").exists(), "top-level programs/ should not exist");
        assert!(!dir.path().join("evals").exists(), "top-level evals/ should not exist");
    }

    #[test]
    fn init_does_not_create_programs_dir() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        assert!(!dir.path().join("programs").exists());
        assert!(!dir.path().join(".morph/programs").exists());
    }

    #[test]
    fn init_idempotent_if_exists() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let store2 = init_repo(dir.path()).unwrap();
        assert!(store2.objects_dir().exists());
    }
}
