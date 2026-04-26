//! Author / identity resolution for commits.
//!
//! PR 6 introduces durable human identity on commits. The user-visible
//! priority order, matching git's `--author` / `GIT_AUTHOR_*` /
//! `user.name|email` chain, is:
//!
//!   1. explicit `--author "Name <email>"` from the CLI
//!   2. `MORPH_AUTHOR_NAME` + `MORPH_AUTHOR_EMAIL` env vars
//!   3. `user.name` / `user.email` in `.morph/config.json`
//!   4. legacy default `"morph"` (used by all pre-PR-6 commits)
//!
//! `resolve_author` is the canonical pure form (all inputs explicit) so
//! tests don't need to mock env / filesystem; `resolve_author_for_repo`
//! is the thin wrapper that reads `morph_dir` and the process env.
//!
//! The disambiguation between *human* identity (this module) and *agent*
//! identity (`agent.instance_id`, see PR 6 stage B) is intentional: a
//! single human can drive many agents, and a merge across two laptops
//! should record both human-and-instance pairs as contributors.

use crate::store::MorphError;
use std::path::Path;

const CONFIG_FILE: &str = "config.json";

/// Format a `name` and `email` into a single author string.
///
/// - Both → `"Name <email>"`
/// - Name only → `"Name"`
/// - Email only → `"<email>"`
/// - Neither → `None`
fn format_author(name: Option<&str>, email: Option<&str>) -> Option<String> {
    match (
        name.map(str::trim).filter(|s| !s.is_empty()),
        email.map(str::trim).filter(|s| !s.is_empty()),
    ) {
        (Some(n), Some(e)) => Some(format!("{} <{}>", n, e)),
        (Some(n), None) => Some(n.to_string()),
        (None, Some(e)) => Some(format!("<{}>", e)),
        (None, None) => None,
    }
}

/// Pure form of author resolution. Inputs are passed in explicitly so
/// this is trivially testable without env or filesystem.
///
/// Returns the formatted author string. Callers that already have an
/// `explicit` author (e.g. `--author "Bob"`) get it back verbatim and
/// the env/config chain is short-circuited; this matches git's
/// behaviour where `--author` overrides everything else.
pub fn resolve_author(
    explicit: Option<&str>,
    env_name: Option<&str>,
    env_email: Option<&str>,
    cfg_name: Option<&str>,
    cfg_email: Option<&str>,
) -> String {
    if let Some(a) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return a.to_string();
    }
    if let Some(s) = format_author(env_name, env_email) {
        return s;
    }
    if let Some(s) = format_author(cfg_name, cfg_email) {
        return s;
    }
    "morph".to_string()
}

/// Read `user.name` and `user.email` from `<morph_dir>/config.json`.
/// Both are optional; missing keys come back as `None`. The config
/// shape mirrors git's `user.*` namespace (we use a nested object
/// `{"user": {"name": "...", "email": "..."}}` rather than dotted
/// keys, since that's what the rest of `morph` already does).
pub fn read_identity_config(
    morph_dir: &Path,
) -> Result<(Option<String>, Option<String>), MorphError> {
    let config_path = morph_dir.join(CONFIG_FILE);
    if !config_path.exists() {
        return Ok((None, None));
    }
    let data = std::fs::read_to_string(&config_path)?;
    let config: serde_json::Value = serde_json::from_str(&data)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    let user = match config.get("user") {
        Some(u) => u,
        None => return Ok((None, None)),
    };
    let name = user.get("name").and_then(|v| v.as_str()).map(str::to_string);
    let email = user.get("email").and_then(|v| v.as_str()).map(str::to_string);
    Ok((name, email))
}

/// Persist `user.name` (and/or `user.email`) into `config.json`,
/// preserving every other key. `None` leaves the existing value
/// untouched, matching the behaviour of `git config user.name`
/// when no value is supplied (we still error in the CLI when
/// neither key was specified, but at the storage layer we treat
/// `None` as "don't change").
pub fn write_identity_config(
    morph_dir: &Path,
    name: Option<&str>,
    email: Option<&str>,
) -> Result<(), MorphError> {
    let config_path = morph_dir.join(CONFIG_FILE);
    let mut config: serde_json::Value = if config_path.exists() {
        let data = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&data).map_err(|e| MorphError::Serialization(e.to_string()))?
    } else {
        serde_json::json!({})
    };
    if !config.is_object() {
        return Err(MorphError::Serialization(
            "config.json is not a JSON object".to_string(),
        ));
    }
    let user = config
        .as_object_mut()
        .unwrap()
        .entry("user".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !user.is_object() {
        return Err(MorphError::Serialization(
            "config.json: `user` is not a JSON object".to_string(),
        ));
    }
    if let Some(n) = name {
        user["name"] = serde_json::Value::String(n.to_string());
    }
    if let Some(e) = email {
        user["email"] = serde_json::Value::String(e.to_string());
    }
    let pretty = serde_json::to_string_pretty(&config)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    std::fs::write(&config_path, pretty)?;
    Ok(())
}

/// Wrapper that reads env + config and applies the priority chain.
/// Used by `morph commit` and `morph merge --continue`.
pub fn resolve_author_for_repo(
    morph_dir: &Path,
    explicit: Option<&str>,
) -> Result<String, MorphError> {
    let env_name = std::env::var("MORPH_AUTHOR_NAME").ok();
    let env_email = std::env::var("MORPH_AUTHOR_EMAIL").ok();
    let (cfg_name, cfg_email) = read_identity_config(morph_dir)?;
    Ok(resolve_author(
        explicit,
        env_name.as_deref(),
        env_email.as_deref(),
        cfg_name.as_deref(),
        cfg_email.as_deref(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn explicit_overrides_everything() {
        let s = resolve_author(
            Some("Alice"),
            Some("EnvName"),
            Some("env@e.com"),
            Some("CfgName"),
            Some("cfg@c.com"),
        );
        assert_eq!(s, "Alice");
    }

    #[test]
    fn env_overrides_config_and_default() {
        let s = resolve_author(None, Some("Bob"), Some("b@e.com"), Some("Cfg"), Some("c@e.com"));
        assert_eq!(s, "Bob <b@e.com>");
    }

    #[test]
    fn config_used_when_no_env_or_explicit() {
        let s = resolve_author(None, None, None, Some("Carol"), Some("c@e.com"));
        assert_eq!(s, "Carol <c@e.com>");
    }

    #[test]
    fn name_only_skips_angle_brackets() {
        let s = resolve_author(None, None, None, Some("Dave"), None);
        assert_eq!(s, "Dave");
    }

    #[test]
    fn email_only_emits_brackets_alone() {
        let s = resolve_author(None, None, None, None, Some("e@e.com"));
        assert_eq!(s, "<e@e.com>");
    }

    #[test]
    fn no_inputs_falls_back_to_morph() {
        let s = resolve_author(None, None, None, None, None);
        assert_eq!(s, "morph");
    }

    #[test]
    fn empty_strings_treated_as_absent() {
        // Whitespace-only strings should also not count.
        let s = resolve_author(None, Some(""), Some("   "), Some("Carol"), Some("c@e.com"));
        assert_eq!(s, "Carol <c@e.com>");
    }

    #[test]
    fn explicit_whitespace_falls_through() {
        // An empty `--author ""` shouldn't pin us to ""; fall through.
        let s = resolve_author(Some("   "), None, None, Some("Cfg"), None);
        assert_eq!(s, "Cfg");
    }

    #[test]
    fn partial_env_takes_precedence_over_full_config() {
        // git also does this: GIT_AUTHOR_NAME alone wins over
        // user.name+user.email. We mirror that to avoid a confusing
        // mixed-source identity.
        let s = resolve_author(None, Some("EnvOnly"), None, Some("Cfg"), Some("c@e.com"));
        assert_eq!(s, "EnvOnly");
    }

    #[test]
    fn read_identity_config_missing_file_is_ok() {
        let tmp = tempdir().unwrap();
        let (n, e) = read_identity_config(tmp.path()).unwrap();
        assert_eq!(n, None);
        assert_eq!(e, None);
    }

    #[test]
    fn read_identity_config_missing_user_block_is_ok() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.json"),
            serde_json::json!({"repo_version": "0.5"}).to_string(),
        )
        .unwrap();
        let (n, e) = read_identity_config(tmp.path()).unwrap();
        assert_eq!(n, None);
        assert_eq!(e, None);
    }

    #[test]
    fn write_identity_config_creates_user_block_preserving_other_keys() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.json"),
            serde_json::json!({"repo_version": "0.5"}).to_string(),
        )
        .unwrap();

        write_identity_config(tmp.path(), Some("Raffi"), Some("r@e.com")).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("config.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["repo_version"], "0.5");
        assert_eq!(v["user"]["name"], "Raffi");
        assert_eq!(v["user"]["email"], "r@e.com");
    }

    #[test]
    fn write_identity_config_partial_update_keeps_other_field() {
        let tmp = tempdir().unwrap();
        write_identity_config(tmp.path(), Some("Raffi"), Some("r@e.com")).unwrap();
        write_identity_config(tmp.path(), None, Some("new@e.com")).unwrap();
        let raw = std::fs::read_to_string(tmp.path().join("config.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["user"]["name"], "Raffi");
        assert_eq!(v["user"]["email"], "new@e.com");
    }

    #[test]
    fn read_after_write_round_trips() {
        let tmp = tempdir().unwrap();
        write_identity_config(tmp.path(), Some("Raffi"), Some("r@e.com")).unwrap();
        let (n, e) = read_identity_config(tmp.path()).unwrap();
        assert_eq!(n.as_deref(), Some("Raffi"));
        assert_eq!(e.as_deref(), Some("r@e.com"));
    }

    #[test]
    fn resolve_author_for_repo_uses_config_when_no_env() {
        let tmp = tempdir().unwrap();
        write_identity_config(tmp.path(), Some("Raffi"), Some("r@e.com")).unwrap();
        // Clear MORPH_AUTHOR_* to make this deterministic against
        // any inherited test env.
        // SAFETY: scoped to this test; we restore at end.
        let prev_n = std::env::var_os("MORPH_AUTHOR_NAME");
        let prev_e = std::env::var_os("MORPH_AUTHOR_EMAIL");
        unsafe {
            std::env::remove_var("MORPH_AUTHOR_NAME");
            std::env::remove_var("MORPH_AUTHOR_EMAIL");
        }
        let s = resolve_author_for_repo(tmp.path(), None).unwrap();
        unsafe {
            if let Some(v) = prev_n {
                std::env::set_var("MORPH_AUTHOR_NAME", v);
            }
            if let Some(v) = prev_e {
                std::env::set_var("MORPH_AUTHOR_EMAIL", v);
            }
        }
        assert_eq!(s, "Raffi <r@e.com>");
    }
}
