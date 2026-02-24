//! `morph setup cursor` — install hooks, MCP config, and Cursor rules into a project.

use std::path::Path;

/// Embedded asset contents (compiled into the binary).
mod assets {
    pub const HOOK_PROMPT: &str = include_str!("../assets/cursor/hooks/morph-record-prompt.sh");
    pub const HOOK_RESPONSE: &str =
        include_str!("../assets/cursor/hooks/morph-record-response.sh");
    pub const HOOK_STOP: &str = include_str!("../assets/cursor/hooks/morph-record-stop.sh");

    pub const RULE_MORPH_RECORD: &str =
        include_str!("../assets/cursor/rules/morph-record.mdc");
    pub const RULE_EVAL_DRIVEN: &str =
        include_str!("../assets/cursor/rules/eval-driven-development.mdc");
    pub const RULE_BEHAVIORAL_COMMITS: &str =
        include_str!("../assets/cursor/rules/behavioral-commits.mdc");
    pub const RULE_BRANCH_MERGE: &str =
        include_str!("../assets/cursor/rules/branch-merge-eval.mdc");

    pub const HOOK_SCRIPTS: &[(&str, &str)] = &[
        ("morph-record-prompt.sh", HOOK_PROMPT),
        ("morph-record-response.sh", HOOK_RESPONSE),
        ("morph-record-stop.sh", HOOK_STOP),
    ];

    pub const RULES: &[(&str, &str)] = &[
        ("morph-record.mdc", RULE_MORPH_RECORD),
        ("eval-driven-development.mdc", RULE_EVAL_DRIVEN),
        ("behavioral-commits.mdc", RULE_BEHAVIORAL_COMMITS),
        ("branch-merge-eval.mdc", RULE_BRANCH_MERGE),
    ];
}

#[derive(Debug)]
pub struct SetupReport {
    pub hooks_written: Vec<String>,
    pub rules_written: Vec<String>,
    pub hooks_json_updated: bool,
    pub mcp_json_updated: bool,
}

/// Install Cursor hooks, MCP config, and rules into `project_root`.
/// Requires `.morph/` to exist (run `morph init` first).
/// Idempotent: safe to call multiple times.
pub fn setup_cursor(project_root: &Path) -> anyhow::Result<SetupReport> {
    if !project_root.join(".morph").is_dir() {
        anyhow::bail!(
            ".morph directory not found in {}. Run `morph init` first.",
            project_root.display()
        );
    }

    let cursor_dir = project_root.join(".cursor");
    std::fs::create_dir_all(&cursor_dir)?;

    let hooks_written = write_hook_scripts(project_root)?;
    let hooks_json_updated = merge_hooks_json(&cursor_dir)?;
    let mcp_json_updated = merge_mcp_json(&cursor_dir, project_root)?;
    let rules_written = write_rules(&cursor_dir)?;

    Ok(SetupReport {
        hooks_written,
        rules_written,
        hooks_json_updated,
        mcp_json_updated,
    })
}

fn write_hook_scripts(project_root: &Path) -> anyhow::Result<Vec<String>> {
    let hooks_dir = project_root.join("cursor");
    std::fs::create_dir_all(&hooks_dir)?;

    let mut written = Vec::new();
    for (name, content) in assets::HOOK_SCRIPTS {
        let path = hooks_dir.join(name);
        std::fs::write(&path, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path)?.permissions();
            perms.set_mode(perms.mode() | 0o755);
            std::fs::set_permissions(&path, perms)?;
        }
        written.push(name.to_string());
    }
    Ok(written)
}

fn merge_hooks_json(cursor_dir: &Path) -> anyhow::Result<bool> {
    let path = cursor_dir.join("hooks.json");
    let mut doc: serde_json::Value = if path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&path)?)?
    } else {
        serde_json::json!({"version": 1, "hooks": {}})
    };

    let hooks = doc
        .as_object_mut()
        .and_then(|o| o.entry("hooks").or_insert(serde_json::json!({})).as_object_mut().cloned())
        .unwrap_or_default();

    let morph_hooks: &[(&str, &str)] = &[
        ("beforeSubmitPrompt", "cursor/morph-record-prompt.sh"),
        ("afterAgentResponse", "cursor/morph-record-response.sh"),
        ("stop", "cursor/morph-record-stop.sh"),
    ];

    let mut hooks_map = hooks;
    for (event, command) in morph_hooks {
        let arr = hooks_map
            .entry(event.to_string())
            .or_insert(serde_json::json!([]))
            .as_array_mut()
            .cloned()
            .unwrap_or_default();

        let already = arr.iter().any(|entry| {
            entry
                .get("command")
                .and_then(|c| c.as_str())
                .map_or(false, |c| c == *command)
        });
        if !already {
            let mut new_arr = arr;
            new_arr.push(serde_json::json!({"command": command}));
            hooks_map.insert(
                event.to_string(),
                serde_json::Value::Array(new_arr),
            );
        }
    }

    doc["hooks"] = serde_json::Value::Object(hooks_map.into_iter().collect());
    doc["version"] = serde_json::json!(1);
    std::fs::write(&path, serde_json::to_string_pretty(&doc)?)?;
    Ok(true)
}

fn merge_mcp_json(cursor_dir: &Path, project_root: &Path) -> anyhow::Result<bool> {
    let path = cursor_dir.join("mcp.json");
    let mut doc: serde_json::Value = if path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&path)?)?
    } else {
        serde_json::json!({"mcpServers": {}})
    };

    let servers = doc
        .as_object_mut()
        .and_then(|o| {
            o.entry("mcpServers")
                .or_insert(serde_json::json!({}))
                .as_object_mut()
        });

    if let Some(servers) = servers {
        let project_path = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf())
            .to_string_lossy()
            .to_string();

        servers.insert(
            "morph".to_string(),
            serde_json::json!({
                "command": "morph-mcp",
                "args": [],
                "env": {
                    "MORPH_WORKSPACE": project_path
                }
            }),
        );
    }

    std::fs::write(&path, serde_json::to_string_pretty(&doc)?)?;
    Ok(true)
}

fn write_rules(cursor_dir: &Path) -> anyhow::Result<Vec<String>> {
    let rules_dir = cursor_dir.join("rules");
    std::fs::create_dir_all(&rules_dir)?;

    let mut written = Vec::new();
    for (name, content) in assets::RULES {
        std::fs::write(rules_dir.join(name), content)?;
        written.push(name.to_string());
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn make_morph_repo(dir: &Path) {
        fs::create_dir_all(dir.join(".morph")).unwrap();
    }

    #[test]
    fn requires_morph_init() {
        let tmp = tempfile::tempdir().unwrap();
        let result = setup_cursor(tmp.path());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("morph") || msg.contains(".morph"),
            "error should mention morph init: {msg}"
        );
    }

    #[test]
    fn hooks_json_created() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        setup_cursor(tmp.path()).unwrap();

        let hooks_path = tmp.path().join(".cursor/hooks.json");
        assert!(hooks_path.exists(), ".cursor/hooks.json should exist");

        let val: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&hooks_path).unwrap()).unwrap();
        assert_eq!(val["version"], 1);
        let bsp = &val["hooks"]["beforeSubmitPrompt"];
        assert!(bsp.is_array() && !bsp.as_array().unwrap().is_empty());
        let aar = &val["hooks"]["afterAgentResponse"];
        assert!(aar.is_array() && !aar.as_array().unwrap().is_empty());
        let stop = &val["hooks"]["stop"];
        assert!(stop.is_array() && !stop.as_array().unwrap().is_empty());
    }

    #[test]
    fn hooks_json_merged_preserves_existing() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        let cursor_dir = tmp.path().join(".cursor");
        fs::create_dir_all(&cursor_dir).unwrap();
        fs::write(
            cursor_dir.join("hooks.json"),
            r#"{"version":1,"hooks":{"beforeSubmitPrompt":[{"command":"my-custom-hook.sh"}]}}"#,
        )
        .unwrap();

        setup_cursor(tmp.path()).unwrap();

        let val: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(cursor_dir.join("hooks.json")).unwrap(),
        )
        .unwrap();
        let bsp = val["hooks"]["beforeSubmitPrompt"].as_array().unwrap();
        let commands: Vec<&str> = bsp
            .iter()
            .filter_map(|e| e["command"].as_str())
            .collect();
        assert!(
            commands.contains(&"my-custom-hook.sh"),
            "original hook should be preserved: {commands:?}"
        );
        assert!(
            commands.iter().any(|c| c.contains("morph-record-prompt")),
            "morph hook should be added: {commands:?}"
        );
    }

    #[test]
    fn mcp_json_created() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        setup_cursor(tmp.path()).unwrap();

        let mcp_path = tmp.path().join(".cursor/mcp.json");
        assert!(mcp_path.exists(), ".cursor/mcp.json should exist");

        let val: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&mcp_path).unwrap()).unwrap();
        let morph = &val["mcpServers"]["morph"];
        assert_eq!(morph["command"].as_str().unwrap(), "morph-mcp");
        let ws = morph["env"]["MORPH_WORKSPACE"].as_str().unwrap();
        let expected = tmp.path().canonicalize().unwrap();
        assert_eq!(ws, expected.to_str().unwrap());
    }

    #[test]
    fn mcp_json_merged_preserves_existing() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        let cursor_dir = tmp.path().join(".cursor");
        fs::create_dir_all(&cursor_dir).unwrap();
        fs::write(
            cursor_dir.join("mcp.json"),
            r#"{"mcpServers":{"other-server":{"command":"other-mcp"}}}"#,
        )
        .unwrap();

        setup_cursor(tmp.path()).unwrap();

        let val: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(cursor_dir.join("mcp.json")).unwrap(),
        )
        .unwrap();
        assert!(
            val["mcpServers"]["other-server"].is_object(),
            "existing server should be preserved"
        );
        assert!(
            val["mcpServers"]["morph"].is_object(),
            "morph server should be added"
        );
    }

    #[test]
    fn hook_scripts_written() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        let report = setup_cursor(tmp.path()).unwrap();

        for (name, _) in assets::HOOK_SCRIPTS {
            let path = tmp.path().join("cursor").join(name);
            assert!(path.exists(), "hook script should exist: {name}");
            let content = fs::read_to_string(&path).unwrap();
            assert!(
                content.starts_with("#!/usr/bin/env bash"),
                "hook script should be a bash script: {name}"
            );
        }
        assert_eq!(report.hooks_written.len(), 3);
    }

    #[test]
    fn hook_scripts_executable() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        setup_cursor(tmp.path()).unwrap();

        for (name, _) in assets::HOOK_SCRIPTS {
            let path = tmp.path().join("cursor").join(name);
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert!(
                mode & 0o111 != 0,
                "hook script should be executable: {name} (mode: {mode:o})"
            );
        }
    }

    #[test]
    fn rules_installed() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        let report = setup_cursor(tmp.path()).unwrap();

        for (name, expected_content) in assets::RULES {
            let path = tmp.path().join(".cursor/rules").join(name);
            assert!(path.exists(), "rule should exist: {name}");
            let content = fs::read_to_string(&path).unwrap();
            assert_eq!(content, *expected_content, "rule content should match: {name}");
        }
        assert_eq!(report.rules_written.len(), 4);
    }

    #[test]
    fn idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());

        setup_cursor(tmp.path()).unwrap();
        let hooks_first = fs::read_to_string(tmp.path().join(".cursor/hooks.json")).unwrap();
        let mcp_first = fs::read_to_string(tmp.path().join(".cursor/mcp.json")).unwrap();

        setup_cursor(tmp.path()).unwrap();
        let hooks_second = fs::read_to_string(tmp.path().join(".cursor/hooks.json")).unwrap();
        let mcp_second = fs::read_to_string(tmp.path().join(".cursor/mcp.json")).unwrap();

        assert_eq!(hooks_first, hooks_second, "hooks.json should be stable across runs");
        assert_eq!(mcp_first, mcp_second, "mcp.json should be stable across runs");

        let bsp = serde_json::from_str::<serde_json::Value>(&hooks_second).unwrap()
            ["hooks"]["beforeSubmitPrompt"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(bsp, 1, "should not duplicate morph hooks on re-run");
    }
}
