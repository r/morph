//! Repository operations: init, directory layout, store versioning.
//!
//! Interacting with an older store version requires an explicit upgrade via `morph upgrade` (CLI).
//! MCP and other tools must not perform upgrades; they call [require_store_version] and error if the
//! repo is older than supported.

use crate::store::{FsStore, MorphError, Store};
use std::path::Path;

/// Store version written by init and read for upgrade checks. "0.0" = FsStore layout.
pub const STORE_VERSION_INIT: &str = "0.0";

/// Store version after migration to Git-format hashes. "0.2" = FsStore with Git-format hashing.
pub const STORE_VERSION_0_2: &str = "0.2";

/// Store version with file tree storage in commits. "0.3" = Git-format hashing + tree commits.
pub const STORE_VERSION_0_3: &str = "0.3";

/// Store version with fan-out object layout. "0.4" = Git-format hashing + fan-out objects dir.
pub const STORE_VERSION_0_4: &str = "0.4";

/// Store version with merge state files and `unmerged_entries` index.
/// "0.5" = same FsStore layout as 0.4 + multi-machine merge primitives (PR 3).
pub const STORE_VERSION_0_5: &str = "0.5";

/// Directory names under .morph/
const OBJECTS_DIR: &str = "objects";
const REFS_HEADS_DIR: &str = "refs/heads";
const RUNS_DIR: &str = "runs";
const TRACES_DIR: &str = "traces";
const CONFIG_FILE: &str = "config.json";
const PROMPTS_DIR: &str = "prompts";
const EVALS_DIR: &str = "evals";
const REPO_VERSION_KEY: &str = "repo_version";

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
        return Err(MorphError::AlreadyExists(
            "already a morph repository (directory .morph exists)".into(),
        ));
    }

    std::fs::create_dir_all(morph_dir.join(OBJECTS_DIR))?;
    std::fs::create_dir_all(morph_dir.join(REFS_HEADS_DIR))?;
    std::fs::create_dir_all(morph_dir.join(RUNS_DIR))?;
    std::fs::create_dir_all(morph_dir.join(TRACES_DIR))?;
    std::fs::create_dir_all(morph_dir.join(PROMPTS_DIR))?;
    std::fs::create_dir_all(morph_dir.join(EVALS_DIR))?;

    let config = serde_json::json!({ REPO_VERSION_KEY: STORE_VERSION_INIT });
    std::fs::write(morph_dir.join(CONFIG_FILE), serde_json::to_string_pretty(&config).unwrap())?;

    std::fs::write(morph_dir.join("refs").join("HEAD"), "ref: heads/main\n")?;

    std::fs::write(morph_dir.join(".gitignore"), "/objects/\n")?;

    Ok(FsStore::new(morph_dir))
}

/// Read the store version from `.morph/config.json`. Returns `"0.0"` if the file or key is missing (legacy repos).
pub fn read_repo_version(morph_dir: &Path) -> Result<String, MorphError> {
    let config_path = morph_dir.join(CONFIG_FILE);
    if !config_path.exists() {
        return Ok(STORE_VERSION_INIT.to_string());
    }
    let data = std::fs::read_to_string(&config_path)?;
    let config: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| MorphError::Serialization(e.to_string()))?;
    let v = config
        .get(REPO_VERSION_KEY)
        .and_then(|v| v.as_str())
        .unwrap_or(STORE_VERSION_INIT);
    Ok(v.to_string())
}

/// Ensure the repo's store version is one of `allowed`. Returns
/// [`MorphError::RepoTooOld`] when the repo is at a known earlier
/// version (user should run `morph upgrade`) and [`MorphError::RepoTooNew`]
/// when the repo is at a version this binary doesn't recognize as a
/// prior format (user should update their `morph` binary).
pub fn require_store_version(morph_dir: &Path, allowed: &[&str]) -> Result<(), MorphError> {
    let current = read_repo_version(morph_dir)?;
    if allowed.contains(&current.as_str()) {
        return Ok(());
    }
    if is_known_prior_version(&current, allowed) {
        Err(MorphError::RepoTooOld(format!(
            "Repo store version is {}; this tool requires one of [{}]. Run `morph upgrade` in the project directory (morph-cli only), then retry.",
            current,
            allowed.join(", ")
        )))
    } else {
        Err(MorphError::RepoTooNew(format!(
            "Repo store version is {}; this tool only knows up to [{}]. Update your `morph` binary, then retry.",
            current,
            allowed.join(", ")
        )))
    }
}

/// True if `current` is one of the well-known prior versions, OR if it
/// numerically compares less than every version in `allowed`. Mostly the
/// former is sufficient since we maintain the full list of prior versions
/// here, but the numeric check guards against unknown intermediate
/// versions (e.g. a 0.4.1 hot-fix repo) being misclassified as too new.
fn is_known_prior_version(current: &str, allowed: &[&str]) -> bool {
    const KNOWN_PRIOR: &[&str] = &[
        STORE_VERSION_INIT,
        STORE_VERSION_0_2,
        STORE_VERSION_0_3,
        STORE_VERSION_0_4,
    ];
    if KNOWN_PRIOR.contains(&current) {
        return true;
    }
    let cur = parse_version(current);
    if let Some(cur) = cur {
        let max_allowed = allowed.iter().filter_map(|a| parse_version(a)).fold(None, |acc, v| {
            Some(match acc {
                None => v,
                Some(prev) if v > prev => v,
                Some(prev) => prev,
            })
        });
        if let Some(max) = max_allowed {
            return cur < max;
        }
    }
    false
}

fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.split('.').map(|p| p.parse::<u32>().ok());
    let major = parts.next().flatten()?;
    let minor = parts.next().flatten().unwrap_or(0);
    let patch = parts.next().flatten().unwrap_or(0);
    Some((major, minor, patch))
}

/// Open the store for an existing repo at `morph_dir`. Returns the backend appropriate for
/// the repo's `repo_version` (0.0 → legacy hashing, 0.2+ → Git-format hashing).
pub fn open_store(morph_dir: &Path) -> Result<Box<dyn Store>, MorphError> {
    let version = read_repo_version(morph_dir)?;
    Ok(match version.as_str() {
        STORE_VERSION_0_5 | STORE_VERSION_0_4 => Box::new(FsStore::new_git_fanout(morph_dir)),
        STORE_VERSION_0_2 | STORE_VERSION_0_3 => Box::new(FsStore::new_git(morph_dir)),
        _ => Box::new(FsStore::new(morph_dir)),
    })
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
    fn init_creates_gitignore_for_objects() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let gitignore = dir.path().join(".morph/.gitignore");
        assert!(gitignore.exists(), ".morph/.gitignore should exist");
        let content = std::fs::read_to_string(&gitignore).unwrap();
        assert!(content.contains("/objects/"), ".gitignore should ignore objects/");
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
    fn init_errors_when_already_initialized() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let second = init_repo(dir.path());
        match &second {
            Err(e) => {
                assert!(matches!(e, MorphError::AlreadyExists(_)));
                assert!(e.to_string().contains("already a morph repository"));
            }
            Ok(_) => panic!("second init should error when .morph already exists"),
        }
    }

    #[test]
    fn init_writes_repo_version_0_0() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let config_path = dir.path().join(".morph/config.json");
        let data = std::fs::read_to_string(&config_path).unwrap();
        let config: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(config.get("repo_version").and_then(|v| v.as_str()), Some("0.0"));
    }

    #[test]
    fn read_repo_version_returns_0_0_after_init() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let v = read_repo_version(&dir.path().join(".morph")).unwrap();
        assert_eq!(v, "0.0");
    }

    #[test]
    fn read_repo_version_defaults_to_0_0_when_config_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".morph")).unwrap();
        // No config.json
        let v = read_repo_version(&dir.path().join(".morph")).unwrap();
        assert_eq!(v, "0.0");
    }

    #[test]
    fn require_store_version_ok_when_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        assert!(require_store_version(&morph_dir, &["0.0"]).is_ok());
    }

    #[test]
    fn require_store_version_err_when_not_allowed() {
        // Repo is at 0.0 (legacy), allowed is 0.1. 0.0 is a known prior
        // version → RepoTooOld.
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        let err = require_store_version(&morph_dir, &["0.1"]).unwrap_err();
        assert!(
            matches!(err, MorphError::RepoTooOld(_)),
            "expected RepoTooOld for legacy repo, got: {:?}",
            err
        );
    }

    #[test]
    fn require_store_version_returns_repo_too_old_for_known_lower_version() {
        // Repo is at 0.3 (known prior), allowed is 0.5 → RepoTooOld with
        // message that points users at `morph upgrade`.
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        std::fs::write(
            morph_dir.join("config.json"),
            r#"{"repo_version":"0.3"}"#,
        )
        .unwrap();
        let err = require_store_version(&morph_dir, &[STORE_VERSION_0_5]).unwrap_err();
        match err {
            MorphError::RepoTooOld(msg) => {
                assert!(
                    msg.contains("morph upgrade"),
                    "RepoTooOld message must direct user to `morph upgrade`, got: {}",
                    msg
                );
            }
            other => panic!("expected RepoTooOld, got: {:?}", other),
        }
    }

    #[test]
    fn require_store_version_returns_repo_too_new_for_unknown_higher_version() {
        // Repo claims 0.99 (a future, unknown version) and allowed is 0.5
        // → RepoTooNew with message that directs user to update binary.
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        std::fs::write(
            morph_dir.join("config.json"),
            r#"{"repo_version":"0.99"}"#,
        )
        .unwrap();
        let err = require_store_version(&morph_dir, &[STORE_VERSION_0_5]).unwrap_err();
        match err {
            MorphError::RepoTooNew(msg) => {
                assert!(
                    msg.contains("Update your") || msg.contains("update your"),
                    "RepoTooNew message must direct user to update binary, got: {}",
                    msg
                );
            }
            other => panic!("expected RepoTooNew, got: {:?}", other),
        }
    }

    #[test]
    fn open_store_handles_0_5() {
        // 0.5 uses the same fan-out backend as 0.4. Round-trip a blob
        // through `open_store` to confirm it works.
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        std::fs::write(
            morph_dir.join("config.json"),
            r#"{"repo_version":"0.5"}"#,
        )
        .unwrap();
        // Create an objects/.gitignore safety file the way the migration
        // would have done. Not strictly needed to satisfy the test below.
        let store = open_store(&morph_dir).unwrap();
        let blob = crate::objects::MorphObject::Blob(crate::objects::Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let hash = store.put(&blob).unwrap();
        assert!(store.has(&hash).unwrap());
    }

    #[test]
    fn open_store_0_0_returns_fs_store_behavior() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        let store = open_store(&morph_dir).unwrap();
        let blob = crate::objects::MorphObject::Blob(crate::objects::Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let hash = store.put(&blob).unwrap();
        assert!(store.has(&hash).unwrap());
    }

    #[test]
    fn open_store_0_2_after_migrate_returns_gix_store_behavior() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        let fs = FsStore::new(&morph_dir);
        let blob = crate::objects::MorphObject::Blob(crate::objects::Blob {
            kind: "p".into(),
            content: serde_json::json!({}),
        });
        let blob_hash = fs.put(&blob).unwrap();
        let suite = crate::objects::MorphObject::EvalSuite(crate::objects::EvalSuite {
            cases: vec![],
            metrics: vec![],
        });
        let suite_hash = fs.put(&suite).unwrap();
        let commit = crate::objects::MorphObject::Commit(crate::objects::Commit {
            tree: None,
            pipeline: blob_hash.to_string(),
            parents: vec![],
            message: "m".into(),
            timestamp: "2020-01-01T00:00:00Z".into(),
            author: "a".into(),
            contributors: None,
            eval_contract: crate::objects::EvalContract {
                suite: suite_hash.to_string(),
                observed_metrics: std::collections::BTreeMap::new(),
            },
            env_constraints: None,
            evidence_refs: None,
            morph_version: None,
        });
        let commit_hash = fs.put(&commit).unwrap();
        fs.ref_write_raw("HEAD", "ref: heads/main").unwrap();
        fs.ref_write("heads/main", &commit_hash).unwrap();
        crate::migrate::migrate_0_0_to_0_2(&morph_dir).unwrap();

        let store = open_store(&morph_dir).unwrap();
        let head = crate::commit::resolve_head(store.as_ref()).unwrap();
        assert!(head.is_some());
        let obj = store.get(&head.unwrap()).unwrap();
        assert!(matches!(obj, crate::objects::MorphObject::Commit(_)));
    }
}
