//! Agent identity. Each `morph` working repo has a stable
//! `agent.instance_id` that's auto-generated on `morph init` and
//! used to tag commits with the machine/agent they originated on.
//!
//! This is **distinct** from the human `user.name` / `user.email`
//! that PR 6 stage A introduced: a single human ("Raffi") can drive
//! commits from multiple agents (laptop, server, CI), and
//! cross-machine forensics needs both. The instance_id is the
//! "machine fingerprint", not a security primitive — it's not
//! cryptographic, it's not authoritative, it's just a stable handle
//! that round-trips on `Commit.morph_instance` so `morph log` /
//! `morph show` can tell two agents apart.
//!
//! On any operation that creates a `Commit`, we read the
//! `agent.instance_id` from `.morph/config.json` and copy it into
//! the new commit. Old commits don't have the field; they're treated
//! as `None` and rendered without one.

use crate::store::MorphError;
use std::path::Path;

const CONFIG_FILE: &str = "config.json";

/// Generate a fresh instance ID. Format: `morph-<6-hex>`. Six hex
/// chars give us 16M possible IDs — collision-resistant enough for
/// per-developer machines, short enough to render inline in logs.
pub fn generate_instance_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Mix nanos with a bit of process-id entropy. Not security-grade,
    // but the goal is uniqueness across two laptops, not adversarial
    // resistance — those concerns belong in PR 7+ signing work.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mixed = nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(pid);
    let hex = (mixed & 0xFFFFFF) as u32;
    format!("morph-{:06x}", hex)
}

/// Read `agent.instance_id` from `<morph_dir>/config.json`. Returns
/// `None` when absent (older repos, or a freshly-cloned bare repo
/// that doesn't carry one).
pub fn read_instance_id(morph_dir: &Path) -> Result<Option<String>, MorphError> {
    let config_path = morph_dir.join(CONFIG_FILE);
    if !config_path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&config_path)?;
    let config: serde_json::Value = serde_json::from_str(&data)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    let agent = match config.get("agent") {
        Some(a) => a,
        None => return Ok(None),
    };
    Ok(agent
        .get("instance_id")
        .and_then(|v| v.as_str())
        .map(str::to_string))
}

/// Persist `agent.instance_id` into `config.json`, preserving every
/// other key. Does **not** overwrite an existing instance_id.
pub fn ensure_instance_id(morph_dir: &Path) -> Result<String, MorphError> {
    if let Some(existing) = read_instance_id(morph_dir)? {
        return Ok(existing);
    }
    let id = generate_instance_id();
    write_instance_id(morph_dir, &id)?;
    Ok(id)
}

/// Force-write a specific instance_id. Mainly for tests and
/// migrations; production code should prefer `ensure_instance_id`.
pub fn write_instance_id(morph_dir: &Path, id: &str) -> Result<(), MorphError> {
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
    let agent = config
        .as_object_mut()
        .unwrap()
        .entry("agent".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !agent.is_object() {
        return Err(MorphError::Serialization(
            "config.json: `agent` is not a JSON object".to_string(),
        ));
    }
    agent["instance_id"] = serde_json::Value::String(id.to_string());
    let pretty = serde_json::to_string_pretty(&config)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    std::fs::write(&config_path, pretty)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn generated_id_has_expected_shape() {
        let id = generate_instance_id();
        assert!(id.starts_with("morph-"), "got {}", id);
        let suffix = &id["morph-".len()..];
        assert_eq!(suffix.len(), 6, "got {}", id);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn read_instance_id_missing_file() {
        let tmp = tempdir().unwrap();
        assert_eq!(read_instance_id(tmp.path()).unwrap(), None);
    }

    #[test]
    fn read_instance_id_missing_agent_block() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.json"),
            serde_json::json!({"repo_version": "0.5"}).to_string(),
        )
        .unwrap();
        assert_eq!(read_instance_id(tmp.path()).unwrap(), None);
    }

    #[test]
    fn write_then_read_round_trips() {
        let tmp = tempdir().unwrap();
        write_instance_id(tmp.path(), "morph-abc123").unwrap();
        assert_eq!(
            read_instance_id(tmp.path()).unwrap().as_deref(),
            Some("morph-abc123"),
        );
    }

    #[test]
    fn write_preserves_other_keys() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.json"),
            serde_json::json!({"repo_version": "0.5", "user": {"name": "X"}}).to_string(),
        )
        .unwrap();
        write_instance_id(tmp.path(), "morph-abcdef").unwrap();
        let raw = std::fs::read_to_string(tmp.path().join("config.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["repo_version"], "0.5");
        assert_eq!(v["user"]["name"], "X");
        assert_eq!(v["agent"]["instance_id"], "morph-abcdef");
    }

    #[test]
    fn ensure_is_idempotent() {
        let tmp = tempdir().unwrap();
        let first = ensure_instance_id(tmp.path()).unwrap();
        let second = ensure_instance_id(tmp.path()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn ensure_creates_when_absent() {
        let tmp = tempdir().unwrap();
        assert_eq!(read_instance_id(tmp.path()).unwrap(), None);
        let id = ensure_instance_id(tmp.path()).unwrap();
        assert_eq!(read_instance_id(tmp.path()).unwrap().as_deref(), Some(id.as_str()));
    }
}
