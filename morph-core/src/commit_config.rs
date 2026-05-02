//! Read/write helpers for the `commit.*` namespace in
//! `.morph/config.json`.
//!
//! Today the only key is `commit.test_command`, the optional shell
//! command Morph runs from `morph commit` to gather test metrics
//! before recording the commit. Stored as a nested object so the
//! shape matches the rest of the config tree:
//!
//! ```json
//! { "commit": { "test_command": "cargo test --workspace" } }
//! ```
//!
//! Mirrors the `user.*` helpers in `author.rs` so the surface stays
//! consistent across namespaces.
//!
//! On-disk reads tolerate a missing `commit` block or missing
//! `test_command` field — both come back as `Ok(None)`. Writes
//! preserve every other key in `config.json`.

use crate::store::MorphError;
use std::path::Path;

const CONFIG_FILE: &str = "config.json";
const COMMIT_KEY: &str = "commit";
const TEST_COMMAND_KEY: &str = "test_command";

/// Read `commit.test_command` from `<morph_dir>/config.json`. Returns
/// `Ok(None)` when `config.json` is missing, the `commit` block is
/// absent, or `test_command` is unset. Errors only on I/O failure or
/// malformed JSON.
pub fn read_commit_test_command(morph_dir: &Path) -> Result<Option<String>, MorphError> {
    let config_path = morph_dir.join(CONFIG_FILE);
    if !config_path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&config_path)?;
    let config: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| MorphError::Serialization(e.to_string()))?;
    let cmd = config
        .get(COMMIT_KEY)
        .and_then(|c| c.get(TEST_COMMAND_KEY))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Ok(cmd)
}

/// Persist `commit.test_command` into `config.json`, preserving every
/// other key. Pass `Some("")` to clear the configured command (the
/// command will round-trip as the empty string and `morph commit` will
/// treat it as unset).
pub fn write_commit_test_command(morph_dir: &Path, command: &str) -> Result<(), MorphError> {
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
    let commit = config
        .as_object_mut()
        .unwrap()
        .entry(COMMIT_KEY.to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !commit.is_object() {
        return Err(MorphError::Serialization(
            "config.json: `commit` is not a JSON object".to_string(),
        ));
    }
    commit[TEST_COMMAND_KEY] = serde_json::Value::String(command.to_string());
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
    fn read_returns_none_when_config_missing() {
        let dir = tempdir().unwrap();
        let result = read_commit_test_command(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_returns_none_when_commit_block_missing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "{\"repo_version\":\"0.0\"}").unwrap();
        let result = read_commit_test_command(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_returns_value_when_set() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            "{\"commit\":{\"test_command\":\"cargo test\"}}",
        )
        .unwrap();
        let result = read_commit_test_command(dir.path()).unwrap();
        assert_eq!(result.as_deref(), Some("cargo test"));
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempdir().unwrap();
        write_commit_test_command(dir.path(), "cargo test --workspace").unwrap();
        let result = read_commit_test_command(dir.path()).unwrap();
        assert_eq!(result.as_deref(), Some("cargo test --workspace"));
    }

    #[test]
    fn write_preserves_other_keys() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            "{\"repo_version\":\"0.0\",\"user\":{\"name\":\"Alice\"}}",
        )
        .unwrap();
        write_commit_test_command(dir.path(), "pytest").unwrap();
        let raw = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
        assert!(raw.contains("repo_version"));
        assert!(raw.contains("Alice"));
        assert!(raw.contains("pytest"));
    }
}
