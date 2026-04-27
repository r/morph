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

/// Internal: create the on-disk layout at `morph_dir`. Used by both
/// the working-repo `init_repo` (which calls us with
/// `<root>/.morph`) and the bare-repo `init_bare` (which calls us
/// with `<root>` directly). The only difference between the two
/// shapes:
///   - `bare = true` → no `.gitignore`, `bare: true` in config.
///   - `bare = false` → `.gitignore` ignoring `objects/`, no
///     `bare` flag (treated as false on read).
fn init_morph_dir_at(morph_dir: &Path, bare: bool) -> Result<FsStore, MorphError> {
    if morph_dir.exists() {
        let meta = std::fs::metadata(morph_dir).map_err(MorphError::Io)?;
        if !meta.is_dir() {
            return Err(MorphError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "target path exists and is not a directory",
            )));
        }
        // Refuse if it already looks like a Morph repo. We use the
        // presence of `objects/` + `config.json` as the signature
        // since `<root>/.morph` (working) and `<root>` (bare) are
        // both legal but neither should already be a repo when we
        // run `init`.
        if morph_dir.join(OBJECTS_DIR).exists() || morph_dir.join(CONFIG_FILE).exists() {
            return Err(MorphError::AlreadyExists(
                "already a morph repository".into(),
            ));
        }
    }

    std::fs::create_dir_all(morph_dir.join(OBJECTS_DIR))?;
    std::fs::create_dir_all(morph_dir.join(REFS_HEADS_DIR))?;
    std::fs::create_dir_all(morph_dir.join(RUNS_DIR))?;
    std::fs::create_dir_all(morph_dir.join(TRACES_DIR))?;
    std::fs::create_dir_all(morph_dir.join(PROMPTS_DIR))?;
    std::fs::create_dir_all(morph_dir.join(EVALS_DIR))?;

    let mut config = serde_json::json!({ REPO_VERSION_KEY: STORE_VERSION_INIT });
    if bare {
        config["bare"] = serde_json::Value::Bool(true);
    }
    // Phase 2a: opinionated default policy on every fresh repo
    // (working or bare). New repos enforce the simplest possible
    // behavioral evidence — `tests_total` and `tests_passed` —
    // so commits without test results fail loudly. Existing repos
    // are unaffected; they have no `policy` key and the default
    // remains empty when no `policy` block is found.
    let mut policy = crate::policy::RepoPolicy::default();
    policy.required_metrics = vec!["tests_total".into(), "tests_passed".into()];
    config["policy"] = serde_json::to_value(&policy).expect("RepoPolicy serializes");
    std::fs::write(morph_dir.join(CONFIG_FILE), serde_json::to_string_pretty(&config).unwrap())?;

    // Every fresh repo (bare or working) gets a stable
    // `agent.instance_id` (PR 6 stage B). Bare repos get one too —
    // even though they don't author commits today, future PRs may
    // (eg. CI bots running on the server) and we want a single
    // place that owns ID generation.
    crate::agent::ensure_instance_id(morph_dir)?;

    std::fs::write(morph_dir.join("refs").join("HEAD"), "ref: heads/main\n")?;

    if !bare {
        std::fs::write(morph_dir.join(".gitignore"), "/objects/\n")?;
    }

    Ok(FsStore::new(morph_dir))
}

/// Initialize a Morph repository at `root`. Creates only `.morph/` — the
/// working directory itself is the user's project and is not modified.
pub fn init_repo(root: impl AsRef<Path>) -> Result<FsStore, MorphError> {
    let morph_dir = root.as_ref().join(".morph");
    match init_morph_dir_at(&morph_dir, false) {
        Ok(store) => Ok(store),
        // Preserve the legacy phrasing for working-repo errors so
        // existing UI tests that grep for ".morph exists" still pass.
        Err(MorphError::AlreadyExists(_)) => Err(MorphError::AlreadyExists(
            "already a morph repository (directory .morph exists)".into(),
        )),
        Err(e) => Err(e),
    }
}

/// PR 6 stage D: initialize a *bare* Morph repository at `root`.
/// The repo lives directly at `root` (not under `<root>/.morph`)
/// and carries `bare = true` in `config.json`. Bare repos are
/// intended for hosting on a server and have no working tree, no
/// staging index, and no `.gitignore` shim.
pub fn init_bare(root: impl AsRef<Path>) -> Result<FsStore, MorphError> {
    let root = root.as_ref();
    init_morph_dir_at(root, true)
}

/// PR 6 stage D cycle 17: given a path that's either a working
/// project root (`<path>/.morph` is the repo) or a bare repo
/// (`<path>` itself is the repo), return the path that
/// `open_store` should be called with. Errors when neither layout
/// is present so callers get a clear "not a morph repository"
/// message.
pub fn resolve_morph_dir(path: &Path) -> Result<std::path::PathBuf, MorphError> {
    let working = path.join(".morph");
    let working_repo = working.join(OBJECTS_DIR).is_dir() && working.join(CONFIG_FILE).is_file();
    let bare_repo = path.join(OBJECTS_DIR).is_dir() && path.join(CONFIG_FILE).is_file();
    match (working_repo, bare_repo) {
        (true, _) => Ok(working),
        (false, true) => Ok(path.to_path_buf()),
        (false, false) => Err(MorphError::NotFound(format!(
            "not a morph repository: {}",
            path.display()
        ))),
    }
}

/// Read the `bare` flag from `<morph_dir>/config.json`. Defaults to
/// `false` (working repo) when the key or file is missing.
pub fn is_bare(morph_dir: &Path) -> Result<bool, MorphError> {
    let config_path = morph_dir.join(CONFIG_FILE);
    if !config_path.exists() {
        return Ok(false);
    }
    let data = std::fs::read_to_string(&config_path)?;
    let config: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| MorphError::Serialization(e.to_string()))?;
    Ok(config.get("bare").and_then(|v| v.as_bool()).unwrap_or(false))
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

    /// PR 6 stage D cycle 14: bare repos live at `root` directly,
    /// have `bare: true` in config, and no `.gitignore` shim.
    #[test]
    fn init_bare_layout_at_root_no_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project.morph");
        let _ = init_bare(&root).unwrap();
        assert!(root.join(OBJECTS_DIR).is_dir(), "objects/ must exist at root");
        assert!(root.join(REFS_HEADS_DIR).is_dir(), "refs/heads/ must exist");
        assert!(root.join(PROMPTS_DIR).is_dir());
        assert!(root.join(EVALS_DIR).is_dir());
        assert!(root.join(CONFIG_FILE).exists());
        // No working-repo shim: no enclosing .morph and no
        // `.gitignore` (a bare repo isn't checked in to git).
        assert!(!root.join(".morph").exists());
        assert!(!root.join(".gitignore").exists());
        // Seeded HEAD so the first push has a default branch to
        // fast-forward.
        assert!(root.join("refs/HEAD").exists());

        // Config marks the repo as bare and carries the version.
        let raw = std::fs::read_to_string(root.join(CONFIG_FILE)).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(cfg["bare"], true);
        assert_eq!(cfg[REPO_VERSION_KEY], STORE_VERSION_INIT);
    }

    #[test]
    fn init_bare_seeds_agent_instance_id() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("server.morph");
        let _ = init_bare(&root).unwrap();
        let id = crate::agent::read_instance_id(&root).unwrap();
        assert!(id.is_some(), "bare repo should still get an instance_id");
    }

    #[test]
    fn init_bare_refuses_existing_repo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("server.morph");
        let _ = init_bare(&root).unwrap();
        match init_bare(&root) {
            Err(MorphError::AlreadyExists(_)) => {}
            Ok(_) => panic!("expected AlreadyExists"),
            Err(e) => panic!("expected AlreadyExists, got: {:?}", e),
        }
    }

    /// PR 6 stage D cycle 15: `is_bare` reads the config flag.
    #[test]
    fn is_bare_true_for_bare_repos() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("server.morph");
        let _ = init_bare(&root).unwrap();
        assert!(is_bare(&root).unwrap());
    }

    #[test]
    fn is_bare_false_for_working_repos() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        assert!(!is_bare(&morph_dir).unwrap());
    }

    #[test]
    fn is_bare_defaults_to_false_when_config_missing() {
        let dir = tempfile::tempdir().unwrap();
        // No config.json at all → not bare.
        assert!(!is_bare(dir.path()).unwrap());
    }

    /// PR 6 stage D cycle 17: resolve_morph_dir auto-detects both
    /// layouts so callers don't need to know whether a server-side
    /// path is bare or working.
    #[test]
    fn resolve_morph_dir_finds_working_layout() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let resolved = resolve_morph_dir(dir.path()).unwrap();
        assert_eq!(resolved, dir.path().join(".morph"));
    }

    #[test]
    fn resolve_morph_dir_finds_bare_layout() {
        let dir = tempfile::tempdir().unwrap();
        let bare_root = dir.path().join("server.morph");
        let _ = init_bare(&bare_root).unwrap();
        let resolved = resolve_morph_dir(&bare_root).unwrap();
        assert_eq!(resolved, bare_root);
    }

    #[test]
    fn resolve_morph_dir_prefers_working_when_both_exist() {
        // Edge case: a directory with a `.morph/` *and* a top-level
        // `objects/`. The working layout wins because that's
        // unambiguously the user's checkout.
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        // Manually plant a fake bare layout at the same level.
        std::fs::create_dir_all(dir.path().join(OBJECTS_DIR)).unwrap();
        std::fs::write(
            dir.path().join(CONFIG_FILE),
            r#"{"repo_version":"0.5","bare":true}"#,
        )
        .unwrap();
        let resolved = resolve_morph_dir(dir.path()).unwrap();
        assert_eq!(resolved, dir.path().join(".morph"));
    }

    #[test]
    fn resolve_morph_dir_errors_on_unrelated_directory() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_morph_dir(dir.path()).unwrap_err();
        match err {
            MorphError::NotFound(msg) => assert!(msg.contains("not a morph repository")),
            e => panic!("expected NotFound, got {:?}", e),
        }
    }

    #[test]
    fn init_seeds_agent_instance_id() {
        // PR 6 stage B cycle 6: every fresh repo gets a stable
        // agent.instance_id auto-generated at init time. Two
        // separate inits get distinct IDs so cross-machine merges
        // can tell them apart in commit metadata.
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let _ = init_repo(a.path()).unwrap();
        let _ = init_repo(b.path()).unwrap();
        let id_a = crate::agent::read_instance_id(&a.path().join(".morph"))
            .unwrap()
            .expect("repo a should have an instance_id");
        let id_b = crate::agent::read_instance_id(&b.path().join(".morph"))
            .unwrap()
            .expect("repo b should have an instance_id");
        assert!(id_a.starts_with("morph-"));
        assert!(id_b.starts_with("morph-"));
        // We can't *guarantee* uniqueness from a 6-hex-char space
        // in a tight loop on the same machine, but the time-mixed
        // generator should still differ. If this ever flakes we'll
        // widen the suffix in `generate_instance_id`.
        assert_ne!(id_a, id_b, "two fresh inits should not collide");
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

    /// Phase 2a: every fresh `morph init` ships an opinionated
    /// behavioral policy that demands `tests_total` and
    /// `tests_passed`. This is the stick that turns
    /// "behavioral version control" from aspiration into the
    /// happy-path default.
    #[test]
    fn init_writes_default_policy_with_required_metrics() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let policy = crate::read_policy(&dir.path().join(".morph")).unwrap();
        assert!(
            policy.required_metrics.iter().any(|m| m == "tests_total"),
            "required_metrics should include tests_total: {:?}",
            policy.required_metrics
        );
        assert!(
            policy.required_metrics.iter().any(|m| m == "tests_passed"),
            "required_metrics should include tests_passed: {:?}",
            policy.required_metrics
        );
        assert_eq!(policy.merge_policy, "dominance");
        assert!(policy.push_gated_branches.is_empty());
    }

    #[test]
    fn init_bare_writes_default_policy_with_required_metrics() {
        let dir = tempfile::tempdir().unwrap();
        let _ = crate::init_bare(dir.path()).unwrap();
        // Bare repo writes config directly under `path`, not under
        // `path/.morph`. Verify the policy still made it.
        let policy = crate::read_policy(dir.path()).unwrap();
        assert!(policy.required_metrics.contains(&"tests_total".to_string()));
        assert!(policy.required_metrics.contains(&"tests_passed".to_string()));
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
            morph_instance: None,
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
