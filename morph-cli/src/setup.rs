//! `morph setup cursor` / `morph setup opencode` — install IDE integration into a project.

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

    pub const OPENCODE_AGENTS_MD: &str = include_str!("../assets/opencode/AGENTS.md");
    pub const OPENCODE_PLUGIN: &str =
        include_str!("../assets/opencode/plugins/morph-record.ts");
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

#[derive(Debug)]
pub struct OpenCodeSetupReport {
    pub opencode_json_updated: bool,
    pub agents_md_written: bool,
    pub plugin_written: bool,
}

/// Install OpenCode MCP config, AGENTS.md, and plugin into `project_root`.
/// Requires `.morph/` to exist (run `morph init` first).
/// Idempotent: safe to call multiple times.
pub fn setup_opencode(project_root: &Path) -> anyhow::Result<OpenCodeSetupReport> {
    if !project_root.join(".morph").is_dir() {
        anyhow::bail!(
            ".morph directory not found in {}. Run `morph init` first.",
            project_root.display()
        );
    }

    let opencode_json_updated = merge_opencode_json(project_root)?;
    let agents_md_written = write_agents_md(project_root)?;
    let plugin_written = write_opencode_plugin(project_root)?;

    Ok(OpenCodeSetupReport {
        opencode_json_updated,
        agents_md_written,
        plugin_written,
    })
}

fn merge_opencode_json(project_root: &Path) -> anyhow::Result<bool> {
    let path = project_root.join("opencode.json");
    let mut doc: serde_json::Value = if path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&path)?)?
    } else {
        serde_json::json!({"$schema": "https://opencode.ai/config.json"})
    };

    let project_path = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
        .to_string_lossy()
        .to_string();

    let obj = doc.as_object_mut().ok_or_else(|| anyhow::anyhow!("opencode.json root is not an object"))?;

    let mcp = obj
        .entry("mcp")
        .or_insert(serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("opencode.json mcp is not an object"))?;

    mcp.insert(
        "morph".to_string(),
        serde_json::json!({
            "type": "local",
            "command": ["morph-mcp"],
            "environment": {
                "MORPH_WORKSPACE": project_path
            }
        }),
    );

    // Ensure instructions includes AGENTS.md
    let instructions = obj
        .entry("instructions")
        .or_insert(serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("opencode.json instructions is not an array"))?;

    let has_agents = instructions.iter().any(|v| v.as_str() == Some("AGENTS.md"));
    if !has_agents {
        instructions.push(serde_json::json!("AGENTS.md"));
    }

    std::fs::write(&path, serde_json::to_string_pretty(&doc)?)?;
    Ok(true)
}

fn write_agents_md(project_root: &Path) -> anyhow::Result<bool> {
    let path = project_root.join("AGENTS.md");
    if path.exists() {
        let existing = std::fs::read_to_string(&path)?;
        if !existing.contains("morph_record_session") {
            let appended = format!("{}\n\n{}", existing.trim_end(), assets::OPENCODE_AGENTS_MD);
            std::fs::write(&path, appended)?;
        }
    } else {
        std::fs::write(&path, assets::OPENCODE_AGENTS_MD)?;
    }
    Ok(true)
}

fn write_opencode_plugin(project_root: &Path) -> anyhow::Result<bool> {
    let plugins_dir = project_root.join(".opencode").join("plugins");
    std::fs::create_dir_all(&plugins_dir)?;
    std::fs::write(plugins_dir.join("morph-record.ts"), assets::OPENCODE_PLUGIN)?;
    Ok(true)
}

fn write_hook_scripts(project_root: &Path) -> anyhow::Result<Vec<String>> {
    let cursor_dir = project_root.join(".cursor");
    std::fs::create_dir_all(&cursor_dir)?;

    let mut written = Vec::new();
    for (name, content) in assets::HOOK_SCRIPTS {
        let path = cursor_dir.join(name);
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
        ("beforeSubmitPrompt", ".cursor/morph-record-prompt.sh"),
        ("afterAgentResponse", ".cursor/morph-record-response.sh"),
        ("stop", ".cursor/morph-record-stop.sh"),
    ];

    // Legacy paths we no longer use (scripts now live in .cursor/ for Git-style layout).
    let old_commands: &[&str] = &[
        "cursor/morph-record-prompt.sh",
        "cursor/morph-record-response.sh",
        "cursor/morph-record-stop.sh",
    ];

    let mut hooks_map = hooks;
    for (event, command) in morph_hooks {
        let arr = hooks_map
            .entry(event.to_string())
            .or_insert(serde_json::json!([]))
            .as_array_mut()
            .cloned()
            .unwrap_or_default();

        // Drop any legacy cursor/ entries so we don't duplicate when upgrading.
        let arr: Vec<_> = arr
            .into_iter()
            .filter(|entry| {
                entry
                    .get("command")
                    .and_then(|c| c.as_str())
                    .is_none_or(|c| !old_commands.contains(&c))
            })
            .collect();

        let already = arr.iter().any(|entry| {
            entry
                .get("command")
                .and_then(|c| c.as_str()) == Some(*command)
        });
        if !already {
            let mut new_arr = arr;
            new_arr.push(serde_json::json!({"command": command}));
            hooks_map.insert(
                event.to_string(),
                serde_json::Value::Array(new_arr),
            );
        } else {
            hooks_map.insert(event.to_string(), serde_json::Value::Array(arr));
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
    #[cfg(unix)]
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
            let path = tmp.path().join(".cursor").join(name);
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
            let path = tmp.path().join(".cursor").join(name);
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

    // --- OpenCode tests ---

    #[test]
    fn opencode_requires_morph_init() {
        let tmp = tempfile::tempdir().unwrap();
        let result = setup_opencode(tmp.path());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("morph") || msg.contains(".morph"),
            "error should mention morph init: {msg}"
        );
    }

    #[test]
    fn opencode_json_created() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        setup_opencode(tmp.path()).unwrap();

        let path = tmp.path().join("opencode.json");
        assert!(path.exists(), "opencode.json should exist");

        let val: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let morph = &val["mcp"]["morph"];
        assert_eq!(morph["type"].as_str().unwrap(), "local");
        let cmd = morph["command"].as_array().unwrap();
        assert_eq!(cmd[0].as_str().unwrap(), "morph-mcp");
        let ws = morph["environment"]["MORPH_WORKSPACE"].as_str().unwrap();
        let expected = tmp.path().canonicalize().unwrap();
        assert_eq!(ws, expected.to_str().unwrap());
    }

    #[test]
    fn opencode_json_merged_preserves_existing() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        fs::write(
            tmp.path().join("opencode.json"),
            r#"{"$schema":"https://opencode.ai/config.json","mcp":{"other":{"type":"local","command":["other-mcp"]}},"model":"anthropic/claude-sonnet-4-5"}"#,
        )
        .unwrap();

        setup_opencode(tmp.path()).unwrap();

        let val: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(tmp.path().join("opencode.json")).unwrap(),
        )
        .unwrap();
        assert!(
            val["mcp"]["other"].is_object(),
            "existing MCP server should be preserved"
        );
        assert!(
            val["mcp"]["morph"].is_object(),
            "morph MCP server should be added"
        );
        assert_eq!(
            val["model"].as_str().unwrap(),
            "anthropic/claude-sonnet-4-5",
            "existing model config should be preserved"
        );
    }

    #[test]
    fn opencode_agents_md_created() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        setup_opencode(tmp.path()).unwrap();

        let path = tmp.path().join("AGENTS.md");
        assert!(path.exists(), "AGENTS.md should exist");
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("morph_record_session"),
            "AGENTS.md should mention morph_record_session"
        );
    }

    #[test]
    fn opencode_agents_md_appended_to_existing() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        fs::write(tmp.path().join("AGENTS.md"), "# My Project\n\nExisting rules here.\n").unwrap();

        setup_opencode(tmp.path()).unwrap();

        let content = fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
        assert!(
            content.starts_with("# My Project"),
            "existing content should be preserved at the top"
        );
        assert!(
            content.contains("morph_record_session"),
            "morph instructions should be appended"
        );
    }

    #[test]
    fn opencode_agents_md_not_duplicated() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        setup_opencode(tmp.path()).unwrap();
        let first = fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();

        setup_opencode(tmp.path()).unwrap();
        let second = fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();

        assert_eq!(first, second, "AGENTS.md should not duplicate morph section on re-run");
    }

    #[test]
    fn opencode_plugin_written() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        setup_opencode(tmp.path()).unwrap();

        let path = tmp.path().join(".opencode/plugins/morph-record.ts");
        assert!(path.exists(), "plugin should exist");
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("MorphRecordPlugin"),
            "plugin should contain MorphRecordPlugin export"
        );
    }

    #[test]
    fn opencode_instructions_includes_agents_md() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        setup_opencode(tmp.path()).unwrap();

        let val: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(tmp.path().join("opencode.json")).unwrap(),
        )
        .unwrap();
        let instructions = val["instructions"].as_array().unwrap();
        assert!(
            instructions.iter().any(|v| v.as_str() == Some("AGENTS.md")),
            "instructions should include AGENTS.md"
        );
    }

    #[test]
    fn opencode_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());

        setup_opencode(tmp.path()).unwrap();
        let json_first = fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        let agents_first = fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();

        setup_opencode(tmp.path()).unwrap();
        let json_second = fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        let agents_second = fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();

        assert_eq!(json_first, json_second, "opencode.json should be stable across runs");
        assert_eq!(agents_first, agents_second, "AGENTS.md should be stable across runs");

        let instructions = serde_json::from_str::<serde_json::Value>(&json_second).unwrap()
            ["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v.as_str() == Some("AGENTS.md"))
            .count();
        assert_eq!(instructions, 1, "should not duplicate AGENTS.md in instructions on re-run");
    }
}
