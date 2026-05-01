//! `morph setup cursor` / `morph setup opencode` — install IDE integration into a project.

use std::path::Path;

/// Embedded asset contents (compiled into the binary).
mod assets {
    pub const HOOK_PROMPT: &str = include_str!("../assets/cursor/hooks/morph-record-prompt.sh");
    pub const HOOK_RESPONSE: &str =
        include_str!("../assets/cursor/hooks/morph-record-response.sh");
    pub const HOOK_STOP: &str = include_str!("../assets/cursor/hooks/morph-record-stop.sh");
    /// Phase 5b: optional stop hook that surfaces eval gaps via
    /// `morph eval gaps --json`. Co-installed alongside the
    /// recording hooks so users get the nudge without extra setup.
    pub const HOOK_CHECKS: &str =
        include_str!("../assets/cursor/hooks/morph-record-checks.sh");

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
        ("morph-record-checks.sh", HOOK_CHECKS),
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

    // Claude Code hook scripts. Mirror the shell scripts in
    // `claude-code/hooks/` so `morph setup claude-code` can install them
    // without depending on the source checkout being present at runtime.
    pub const CLAUDE_HOOK_PROMPT: &str =
        include_str!("../assets/claude-code/hooks/morph-record-prompt.sh");
    pub const CLAUDE_HOOK_STOP: &str =
        include_str!("../assets/claude-code/hooks/morph-record-stop.sh");

    pub const CLAUDE_HOOK_SCRIPTS: &[(&str, &str)] = &[
        ("morph-record-prompt.sh", CLAUDE_HOOK_PROMPT),
        ("morph-record-stop.sh", CLAUDE_HOOK_STOP),
    ];

    // Agent of Empires sandbox image template. Co-shipped with
    // `morph setup aoe` so users can opt out of host bind-mounts of
    // /usr/local/bin/morph{,-mcp} by baking the binaries into their
    // sandbox image instead.
    pub const AOE_DOCKERFILE: &str =
        include_str!("../assets/aoe/Dockerfile.morph-aoe");
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

#[derive(Debug)]
pub struct ClaudeCodeSetupReport {
    pub settings_json_updated: bool,
    pub hooks_written: Vec<String>,
}

/// Install Claude Code MCP config + hooks into `project_root`.
///
/// Writes hook scripts to `.claude/hooks/` (executable on Unix) and
/// merges `mcpServers.morph` plus `UserPromptSubmit` / `Stop` hook
/// entries into `.claude/settings.json`. Existing user state is
/// preserved; the morph entries are keyed by command path so a re-run
/// is idempotent rather than additive.
///
/// Requires `.morph/` to exist (run `morph init` first).
pub fn setup_claude_code(project_root: &Path) -> anyhow::Result<ClaudeCodeSetupReport> {
    if !project_root.join(".morph").is_dir() {
        anyhow::bail!(
            ".morph directory not found in {}. Run `morph init` first.",
            project_root.display()
        );
    }

    let claude_dir = project_root.join(".claude");
    std::fs::create_dir_all(&claude_dir)?;

    let hooks_written = write_claude_hook_scripts(&claude_dir)?;
    let settings_json_updated = merge_claude_settings_json(&claude_dir, project_root)?;

    Ok(ClaudeCodeSetupReport {
        settings_json_updated,
        hooks_written,
    })
}

fn write_claude_hook_scripts(claude_dir: &Path) -> anyhow::Result<Vec<String>> {
    let hooks_dir = claude_dir.join("hooks");
    std::fs::create_dir_all(&hooks_dir)?;

    let mut written = Vec::new();
    for (name, content) in assets::CLAUDE_HOOK_SCRIPTS {
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

fn merge_claude_settings_json(
    claude_dir: &Path,
    project_root: &Path,
) -> anyhow::Result<bool> {
    let path = claude_dir.join("settings.json");
    let mut doc: serde_json::Value = if path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&path)?)?
    } else {
        serde_json::json!({})
    };

    let obj = doc
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!(".claude/settings.json root is not an object"))?;

    // 1. mcpServers.morph
    let project_path = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
        .to_string_lossy()
        .to_string();

    let mcp_servers = obj
        .entry("mcpServers")
        .or_insert(serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!(".claude/settings.json mcpServers is not an object"))?;

    mcp_servers.insert(
        "morph".to_string(),
        serde_json::json!({
            "command": "morph-mcp",
            "args": [],
            "env": {
                "MORPH_WORKSPACE": project_path
            }
        }),
    );

    // 2. hooks.UserPromptSubmit + hooks.Stop. Claude Code's hook schema
    //    is `{event: [{matcher?, hooks: [{type, command}]}]}`. Morph
    //    owns the entries whose command points at our two hook scripts;
    //    other entries are left untouched.
    let claude_hooks: &[(&str, &str)] = &[
        ("UserPromptSubmit", ".claude/hooks/morph-record-prompt.sh"),
        ("Stop", ".claude/hooks/morph-record-stop.sh"),
    ];

    let hooks_section = obj
        .entry("hooks")
        .or_insert(serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!(".claude/settings.json hooks is not an object"))?;

    for (event, command) in claude_hooks {
        let matchers = hooks_section
            .entry(event.to_string())
            .or_insert(serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| {
                anyhow::anyhow!(".claude/settings.json hooks.{} is not an array", event)
            })?;

        // Drop any matcher group whose only hook is the morph entry, so
        // re-runs don't accumulate duplicates. Groups that mix our hook
        // with user hooks are kept and their morph-pointing entry is
        // pruned in-place, preserving the user's hook(s).
        matchers.retain_mut(|matcher| {
            let Some(hooks_arr) = matcher
                .get_mut("hooks")
                .and_then(|h| h.as_array_mut())
            else {
                return true;
            };
            hooks_arr.retain(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .is_none_or(|c| c != *command)
            });
            !hooks_arr.is_empty()
        });

        matchers.push(serde_json::json!({
            "hooks": [
                {"type": "command", "command": command}
            ]
        }));
    }

    std::fs::write(&path, serde_json::to_string_pretty(&doc)?)?;
    Ok(true)
}

// === Agent of Empires (`morph setup aoe`) ====================================

/// Caller-controlled options for `setup_aoe`.
#[derive(Debug, Clone)]
pub struct AoeSetupOpts {
    /// Per-agent integrations to install. Each entry must be one of
    /// "cursor", "opencode", or "claude-code". Empty means *all three*
    /// (the typical case — AoE may launch any of them, so we want morph
    /// recording to work regardless of which agent the user picks via
    /// `aoe add`).
    pub agents: Vec<String>,
    /// If true, skip per-agent delegation entirely. Only the AoE-glue
    /// layer (config.toml + Dockerfile + AGENTS.md) is written.
    pub skip_agents: bool,
    /// If true, write `[sandbox].extra_volumes` entries that bind-mount
    /// the host's `morph` and `morph-mcp` binaries into AoE's Docker
    /// sandbox. Set to false when the user prefers a baked sandbox image
    /// (built from the shipped `Dockerfile.morph-aoe`).
    pub bind_mount: bool,
    /// If true, emit `.agent-of-empires/Dockerfile.morph-aoe` as a
    /// reference image template.
    pub write_dockerfile: bool,
}

impl Default for AoeSetupOpts {
    fn default() -> Self {
        Self {
            agents: Vec::new(),
            skip_agents: false,
            bind_mount: true,
            write_dockerfile: true,
        }
    }
}

#[derive(Debug)]
pub struct AoeSetupReport {
    pub config_toml_updated: bool,
    pub dockerfile_written: bool,
    pub agents_md_written: bool,
    /// Names of per-agent integrations that were successfully delegated
    /// to (e.g. `["cursor", "opencode", "claude-code"]`).
    pub delegated: Vec<String>,
}

/// Install Agent of Empires integration into `project_root`.
///
/// Writes `.agent-of-empires/config.toml` with morph lifecycle hooks and
/// (optionally) sandbox bind-mounts; emits `Dockerfile.morph-aoe` as a
/// reference image template; ensures `AGENTS.md` is present so any agent
/// AoE launches sees morph guidance; and (unless `skip_agents`) delegates
/// to `setup_cursor` / `setup_opencode` / `setup_claude_code` so morph
/// recording works regardless of which agent AoE runs.
///
/// Requires `.morph/` to exist (run `morph init` first).
/// Idempotent: safe to call multiple times. Existing user state in
/// `config.toml` is preserved; only morph-owned entries are rewritten.
pub fn setup_aoe(
    project_root: &Path,
    opts: &AoeSetupOpts,
) -> anyhow::Result<AoeSetupReport> {
    if !project_root.join(".morph").is_dir() {
        anyhow::bail!(
            ".morph directory not found in {}. Run `morph init` first.",
            project_root.display()
        );
    }

    let aoe_dir = project_root.join(".agent-of-empires");
    std::fs::create_dir_all(&aoe_dir)?;

    let config_toml_updated = merge_aoe_config_toml(&aoe_dir, opts.bind_mount)?;
    let dockerfile_written = if opts.write_dockerfile {
        write_aoe_dockerfile(&aoe_dir)?
    } else {
        false
    };
    // Always seed AGENTS.md — even with --skip-agents, AoE-launched
    // agents should pick up morph instructions.
    let agents_md_written = write_agents_md(project_root)?;

    let mut delegated = Vec::new();
    if !opts.skip_agents {
        let agents_to_set_up: Vec<String> = if opts.agents.is_empty() {
            vec![
                "cursor".to_string(),
                "opencode".to_string(),
                "claude-code".to_string(),
            ]
        } else {
            opts.agents.clone()
        };

        for agent in &agents_to_set_up {
            match agent.as_str() {
                "cursor" => {
                    setup_cursor(project_root)?;
                    delegated.push("cursor".to_string());
                }
                "opencode" => {
                    setup_opencode(project_root)?;
                    delegated.push("opencode".to_string());
                }
                "claude-code" => {
                    setup_claude_code(project_root)?;
                    delegated.push("claude-code".to_string());
                }
                other => anyhow::bail!(
                    "unknown --agent value `{other}` for `morph setup aoe` \
                     (supported: cursor, opencode, claude-code)"
                ),
            }
        }
    }

    Ok(AoeSetupReport {
        config_toml_updated,
        dockerfile_written,
        agents_md_written,
        delegated,
    })
}

/// Substring patterns identifying a hook command morph owns. Anything in
/// the user's existing `config.toml` matching one of these is removed
/// before we re-emit the canonical morph block, so re-runs don't
/// accumulate duplicate hook lines.
const MORPH_HOOK_PREFIXES: &[&str] = &[
    "morph init --quiet",
    "morph add . && morph commit -m \"aoe-",
    "morph run record-session --prompt \"aoe-",
];

const MORPH_VOLUME_SUFFIXES: &[&str] = &[
    "/usr/local/bin/morph:ro",
    "/usr/local/bin/morph-mcp:ro",
];

const MORPH_ENV_KEYS: &[&str] = &["MORPH_WORKSPACE", "AOE_INSTANCE_ID"];

fn is_morph_hook_command(s: &str) -> bool {
    let trimmed = s.trim_start();
    MORPH_HOOK_PREFIXES.iter().any(|p| trimmed.starts_with(p))
}

fn merge_aoe_config_toml(aoe_dir: &Path, bind_mount: bool) -> anyhow::Result<bool> {
    use toml_edit::{value, Array, DocumentMut, Item, Table, Value};

    let path = aoe_dir.join("config.toml");
    let mut doc: DocumentMut = if path.exists() {
        std::fs::read_to_string(&path)?
            .parse()
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {}", path.display(), e))?
    } else {
        DocumentMut::new()
    };

    fn ensure_table_mut<'a>(doc: &'a mut DocumentMut, key: &str) -> &'a mut Table {
        let needs_init = match doc.get(key) {
            Some(item) => !item.is_table(),
            None => true,
        };
        if needs_init {
            let mut t = Table::new();
            t.set_implicit(false);
            doc.insert(key, Item::Table(t));
        }
        doc.get_mut(key)
            .and_then(|i| i.as_table_mut())
            .expect("table inserted above")
    }

    fn ensure_array<'a>(table: &'a mut Table, key: &str) -> &'a mut Array {
        // AoE accepts either `key = "single"` or `key = ["a", "b"]`.
        // Normalize to an array so we can append predictably.
        let promote_string: Option<String> = match table.get(key) {
            Some(Item::Value(Value::String(s))) => Some(s.value().clone()),
            _ => None,
        };
        let needs_init = !matches!(table.get(key), Some(Item::Value(Value::Array(_))));
        if needs_init {
            let mut arr = Array::new();
            if let Some(s) = promote_string {
                arr.push(s);
            }
            table.insert(key, value(arr));
        }
        table
            .get_mut(key)
            .and_then(|i| i.as_array_mut())
            .expect("array inserted above")
    }

    fn scrub<F: Fn(&str) -> bool>(arr: &mut Array, drop_if: F) {
        let kept: Vec<Value> = arr
            .iter()
            .filter(|v| match v {
                Value::String(s) => !drop_if(s.value()),
                _ => true,
            })
            .cloned()
            .collect();
        arr.clear();
        for v in kept {
            arr.push(v);
        }
    }

    // ---- [hooks] -----------------------------------------------------------
    let canonical: &[(&str, &[&str])] = &[
        (
            "on_create",
            &[
                "morph init --quiet 2>/dev/null || true",
                "morph add . && morph commit -m \"aoe-create: ${AOE_INSTANCE_ID:-unknown}\" --allow-empty-metrics 2>/dev/null || true",
            ],
        ),
        (
            "on_launch",
            &[
                "morph run record-session --prompt \"aoe-launch instance=${AOE_INSTANCE_ID:-unknown} branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo unknown)\" --response \"\" --model-name aoe --agent-id aoe 2>/dev/null || true",
            ],
        ),
        (
            "on_destroy",
            &[
                "morph add . && morph commit -m \"aoe-destroy: ${AOE_INSTANCE_ID:-unknown}\" --allow-empty-metrics 2>/dev/null || true",
                "morph run record-session --prompt \"aoe-destroy instance=${AOE_INSTANCE_ID:-unknown}\" --response \"\" --model-name aoe --agent-id aoe 2>/dev/null || true",
            ],
        ),
    ];

    {
        let hooks = ensure_table_mut(&mut doc, "hooks");
        for (key, morph_entries) in canonical {
            let arr = ensure_array(hooks, key);
            scrub(arr, is_morph_hook_command);
            for entry in *morph_entries {
                arr.push(*entry);
            }
        }
    }

    // ---- [sandbox] ---------------------------------------------------------
    {
        let sandbox = ensure_table_mut(&mut doc, "sandbox");

        let env = ensure_array(sandbox, "environment");
        scrub(env, |s| MORPH_ENV_KEYS.contains(&s));
        for k in MORPH_ENV_KEYS {
            env.push(*k);
        }

        let vols = ensure_array(sandbox, "extra_volumes");
        scrub(vols, |s| {
            MORPH_VOLUME_SUFFIXES.iter().any(|suf| s.ends_with(*suf))
        });
        if bind_mount {
            vols.push("/usr/local/bin/morph:/usr/local/bin/morph:ro");
            vols.push("/usr/local/bin/morph-mcp:/usr/local/bin/morph-mcp:ro");
        }
    }

    std::fs::write(&path, doc.to_string())?;
    Ok(true)
}

fn write_aoe_dockerfile(aoe_dir: &Path) -> anyhow::Result<bool> {
    let path = aoe_dir.join("Dockerfile.morph-aoe");
    std::fs::write(&path, assets::AOE_DOCKERFILE)?;
    Ok(true)
}

// === OpenCode ===============================================================

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
        // Phase 5b: surface unaddressed eval gaps after the
        // recording hook so the warning lands without breaking
        // the recording pipeline.
        ("stop", ".cursor/morph-record-checks.sh"),
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
        assert_eq!(report.hooks_written.len(), assets::HOOK_SCRIPTS.len());
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

    // --- Claude Code tests ---

    #[test]
    fn claude_code_requires_morph_init() {
        let tmp = tempfile::tempdir().unwrap();
        let result = setup_claude_code(tmp.path());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("morph") || msg.contains(".morph"),
            "error should mention morph init: {msg}"
        );
    }

    #[test]
    fn claude_code_settings_json_created() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        setup_claude_code(tmp.path()).unwrap();

        let path = tmp.path().join(".claude/settings.json");
        assert!(path.exists(), ".claude/settings.json should exist");

        let val: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let morph = &val["mcpServers"]["morph"];
        assert_eq!(morph["command"].as_str().unwrap(), "morph-mcp");
        let ws = morph["env"]["MORPH_WORKSPACE"].as_str().unwrap();
        let expected = tmp.path().canonicalize().unwrap();
        assert_eq!(ws, expected.to_str().unwrap());
    }

    #[test]
    fn claude_code_hook_scripts_written_and_executable() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        let report = setup_claude_code(tmp.path()).unwrap();

        for (name, _) in assets::CLAUDE_HOOK_SCRIPTS {
            let path = tmp.path().join(".claude/hooks").join(name);
            assert!(path.exists(), "hook script should exist: {name}");
            let content = fs::read_to_string(&path).unwrap();
            assert!(
                content.starts_with("#!/usr/bin/env bash"),
                "hook script should be a bash script: {name}"
            );

            #[cfg(unix)]
            {
                let mode = fs::metadata(&path).unwrap().permissions().mode();
                assert!(
                    mode & 0o111 != 0,
                    "hook script should be executable: {name} (mode: {mode:o})"
                );
            }
        }
        assert_eq!(report.hooks_written.len(), assets::CLAUDE_HOOK_SCRIPTS.len());
    }

    #[test]
    fn claude_code_hooks_registered_for_userpromptsubmit_and_stop() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        setup_claude_code(tmp.path()).unwrap();

        let val: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(tmp.path().join(".claude/settings.json")).unwrap(),
        )
        .unwrap();
        let user = val["hooks"]["UserPromptSubmit"].as_array().unwrap();
        let stop = val["hooks"]["Stop"].as_array().unwrap();
        assert!(!user.is_empty());
        assert!(!stop.is_empty());

        let user_cmds: Vec<&str> = user
            .iter()
            .flat_map(|m| m["hooks"].as_array().unwrap().iter())
            .filter_map(|h| h["command"].as_str())
            .collect();
        assert!(
            user_cmds.iter().any(|c| c.contains("morph-record-prompt")),
            "UserPromptSubmit should reference morph-record-prompt: {user_cmds:?}"
        );

        let stop_cmds: Vec<&str> = stop
            .iter()
            .flat_map(|m| m["hooks"].as_array().unwrap().iter())
            .filter_map(|h| h["command"].as_str())
            .collect();
        assert!(
            stop_cmds.iter().any(|c| c.contains("morph-record-stop")),
            "Stop should reference morph-record-stop: {stop_cmds:?}"
        );
    }

    #[test]
    fn claude_code_settings_json_merge_preserves_existing() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::write(
            claude_dir.join("settings.json"),
            r#"{
              "model": "opus",
              "mcpServers": {
                "other-server": {"command": "other-mcp"}
              },
              "hooks": {
                "UserPromptSubmit": [
                  {"hooks": [{"type": "command", "command": "my-existing.sh"}]}
                ]
              }
            }"#,
        )
        .unwrap();

        setup_claude_code(tmp.path()).unwrap();

        let val: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(claude_dir.join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(val["model"].as_str().unwrap(), "opus");
        assert!(
            val["mcpServers"]["other-server"].is_object(),
            "existing MCP server should be preserved"
        );
        assert!(
            val["mcpServers"]["morph"].is_object(),
            "morph MCP server should be added"
        );
        let user = val["hooks"]["UserPromptSubmit"].as_array().unwrap();
        let user_cmds: Vec<&str> = user
            .iter()
            .flat_map(|m| m["hooks"].as_array().unwrap().iter())
            .filter_map(|h| h["command"].as_str())
            .collect();
        assert!(
            user_cmds.contains(&"my-existing.sh"),
            "user hook should be preserved: {user_cmds:?}"
        );
        assert!(
            user_cmds.iter().any(|c| c.contains("morph-record-prompt")),
            "morph hook should be added: {user_cmds:?}"
        );
    }

    #[test]
    fn claude_code_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());

        setup_claude_code(tmp.path()).unwrap();
        let settings_first =
            fs::read_to_string(tmp.path().join(".claude/settings.json")).unwrap();
        let prompt_first =
            fs::read_to_string(tmp.path().join(".claude/hooks/morph-record-prompt.sh")).unwrap();

        setup_claude_code(tmp.path()).unwrap();
        let settings_second =
            fs::read_to_string(tmp.path().join(".claude/settings.json")).unwrap();
        let prompt_second =
            fs::read_to_string(tmp.path().join(".claude/hooks/morph-record-prompt.sh")).unwrap();

        assert_eq!(
            settings_first, settings_second,
            "settings.json should be stable across runs"
        );
        assert_eq!(
            prompt_first, prompt_second,
            "hook script should be stable across runs"
        );

        let val: serde_json::Value = serde_json::from_str(&settings_second).unwrap();
        let user = val["hooks"]["UserPromptSubmit"].as_array().unwrap();
        let morph_count = user
            .iter()
            .flat_map(|m| m["hooks"].as_array().unwrap().iter())
            .filter_map(|h| h["command"].as_str())
            .filter(|c| c.contains("morph-record-prompt"))
            .count();
        assert_eq!(
            morph_count, 1,
            "should not duplicate morph hook on re-run"
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

    // --- Agent of Empires (`morph setup aoe`) tests ---

    #[test]
    fn aoe_requires_morph_init() {
        let tmp = tempfile::tempdir().unwrap();
        let result = setup_aoe(tmp.path(), &AoeSetupOpts::default());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("morph") || msg.contains(".morph"),
            "error should mention morph init: {msg}"
        );
    }

    #[test]
    fn aoe_writes_config_dockerfile_and_agents_md() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());

        let report = setup_aoe(tmp.path(), &AoeSetupOpts::default()).unwrap();

        assert!(report.config_toml_updated);
        assert!(report.dockerfile_written);
        assert!(report.agents_md_written);
        assert!(tmp.path().join(".agent-of-empires/config.toml").exists());
        assert!(tmp.path().join(".agent-of-empires/Dockerfile.morph-aoe").exists());
        assert!(tmp.path().join("AGENTS.md").exists());
    }

    #[test]
    fn aoe_config_toml_has_lifecycle_hooks() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        setup_aoe(tmp.path(), &AoeSetupOpts::default()).unwrap();

        let cfg = fs::read_to_string(tmp.path().join(".agent-of-empires/config.toml")).unwrap();
        for needle in [
            "[hooks]",
            "on_create",
            "on_launch",
            "on_destroy",
            "morph commit",
            "aoe-create",
            "aoe-destroy",
            "morph run record-session",
        ] {
            assert!(
                cfg.contains(needle),
                "config.toml should mention {needle:?}; got:\n{cfg}"
            );
        }
    }

    #[test]
    fn aoe_config_toml_seeds_sandbox_env_and_volumes() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        setup_aoe(tmp.path(), &AoeSetupOpts::default()).unwrap();

        let cfg = fs::read_to_string(tmp.path().join(".agent-of-empires/config.toml")).unwrap();
        for needle in [
            "[sandbox]",
            "MORPH_WORKSPACE",
            "AOE_INSTANCE_ID",
            "/usr/local/bin/morph:/usr/local/bin/morph:ro",
            "/usr/local/bin/morph-mcp:/usr/local/bin/morph-mcp:ro",
        ] {
            assert!(
                cfg.contains(needle),
                "config.toml should mention {needle:?}; got:\n{cfg}"
            );
        }
    }

    #[test]
    fn aoe_no_bind_mount_omits_morph_volume_entries() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        let opts = AoeSetupOpts {
            bind_mount: false,
            ..AoeSetupOpts::default()
        };
        setup_aoe(tmp.path(), &opts).unwrap();

        let cfg = fs::read_to_string(tmp.path().join(".agent-of-empires/config.toml")).unwrap();
        // Env passthrough still seeded — that's needed regardless of which
        // sandbox layout the user chose.
        assert!(cfg.contains("MORPH_WORKSPACE"));
        assert!(cfg.contains("AOE_INSTANCE_ID"));
        // But no host bind-mounts of the morph binaries.
        assert!(
            !cfg.contains("/usr/local/bin/morph:ro"),
            "should not bind-mount morph binary: {cfg}"
        );
        assert!(
            !cfg.contains("/usr/local/bin/morph-mcp:ro"),
            "should not bind-mount morph-mcp binary: {cfg}"
        );
    }

    #[test]
    fn aoe_default_delegates_to_all_three_agents() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        let report = setup_aoe(tmp.path(), &AoeSetupOpts::default()).unwrap();

        assert_eq!(
            report.delegated,
            vec!["cursor", "opencode", "claude-code"],
            "default delegation should hit all three agents"
        );
        assert!(tmp.path().join(".cursor/mcp.json").exists());
        assert!(tmp.path().join("opencode.json").exists());
        assert!(tmp.path().join(".claude/settings.json").exists());
    }

    #[test]
    fn aoe_skip_agents_only_writes_glue() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        let opts = AoeSetupOpts {
            skip_agents: true,
            ..AoeSetupOpts::default()
        };
        let report = setup_aoe(tmp.path(), &opts).unwrap();

        assert!(report.delegated.is_empty());
        assert!(tmp.path().join(".agent-of-empires/config.toml").exists());
        // AGENTS.md is *always* seeded so AoE-launched agents still see
        // morph guidance even when per-agent setup is skipped.
        assert!(tmp.path().join("AGENTS.md").exists());
        assert!(!tmp.path().join(".cursor/mcp.json").exists());
        assert!(!tmp.path().join("opencode.json").exists());
        assert!(!tmp.path().join(".claude/settings.json").exists());
    }

    #[test]
    fn aoe_unknown_agent_errors() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        let opts = AoeSetupOpts {
            agents: vec!["totally-fake-agent".to_string()],
            ..AoeSetupOpts::default()
        };
        let err = setup_aoe(tmp.path(), &opts).unwrap_err().to_string();
        assert!(
            err.contains("totally-fake-agent"),
            "error should name the bad agent: {err}"
        );
    }

    #[test]
    fn aoe_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());

        setup_aoe(tmp.path(), &AoeSetupOpts::default()).unwrap();
        let cfg_first =
            fs::read_to_string(tmp.path().join(".agent-of-empires/config.toml")).unwrap();

        setup_aoe(tmp.path(), &AoeSetupOpts::default()).unwrap();
        let cfg_second =
            fs::read_to_string(tmp.path().join(".agent-of-empires/config.toml")).unwrap();

        assert_eq!(
            cfg_first, cfg_second,
            "config.toml should be stable across runs"
        );

        // Each morph hook line should appear exactly once.
        let occ = |s: &str| cfg_second.matches(s).count();
        assert_eq!(occ("morph init --quiet"), 1, "morph init line should not duplicate");
        assert_eq!(
            occ("aoe-create:"),
            1,
            "aoe-create commit line should not duplicate"
        );
        assert_eq!(
            occ("aoe-destroy:"),
            1,
            "aoe-destroy commit line should not duplicate (one in on_destroy, distinct from record-session)"
        );
        assert_eq!(
            occ("MORPH_WORKSPACE"),
            1,
            "MORPH_WORKSPACE env entry should not duplicate"
        );
        assert_eq!(
            occ("/usr/local/bin/morph:/usr/local/bin/morph:ro"),
            1,
            "morph binary mount should not duplicate"
        );
    }

    #[test]
    fn aoe_preserves_existing_user_config() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());
        let aoe_dir = tmp.path().join(".agent-of-empires");
        fs::create_dir_all(&aoe_dir).unwrap();
        fs::write(
            aoe_dir.join("config.toml"),
            r#"# user comment
[hooks]
on_launch = ["npm install"]

[session]
default_tool = "claude"

[sandbox]
enabled_by_default = true
default_image = "my-org/my-sandbox:latest"
environment = ["MY_API_KEY"]
extra_volumes = ["/data:/data:ro"]
"#,
        )
        .unwrap();

        setup_aoe(tmp.path(), &AoeSetupOpts::default()).unwrap();

        let cfg = fs::read_to_string(aoe_dir.join("config.toml")).unwrap();

        assert!(cfg.contains("npm install"), "user on_launch hook preserved");
        assert!(
            cfg.contains("default_tool"),
            "user [session] table preserved"
        );
        assert!(cfg.contains("\"claude\""), "user default_tool value preserved");
        assert!(
            cfg.contains("default_image"),
            "user sandbox.default_image preserved"
        );
        assert!(
            cfg.contains("my-org/my-sandbox"),
            "user sandbox image value preserved"
        );
        assert!(
            cfg.contains("MY_API_KEY"),
            "user sandbox.environment entry preserved"
        );
        assert!(
            cfg.contains("/data:/data:ro"),
            "user sandbox.extra_volumes entry preserved"
        );

        // Morph block layered alongside the user content.
        assert!(cfg.contains("MORPH_WORKSPACE"));
        assert!(cfg.contains("aoe-create"));
        assert!(cfg.contains("/usr/local/bin/morph:/usr/local/bin/morph:ro"));

        // Reparse to confirm the file is still valid TOML.
        let parsed: toml_edit::DocumentMut = cfg.parse().expect("valid TOML");
        let env = parsed["sandbox"]["environment"].as_array().unwrap();
        let env_strings: Vec<String> = env
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        assert!(
            env_strings.iter().any(|s| s == "MY_API_KEY"),
            "MY_API_KEY survived merge: {env_strings:?}"
        );
        assert!(
            env_strings.iter().any(|s| s == "MORPH_WORKSPACE"),
            "MORPH_WORKSPACE added: {env_strings:?}"
        );
    }

    #[test]
    fn aoe_re_run_does_not_duplicate_morph_entries_with_user_config() {
        let tmp = tempfile::tempdir().unwrap();
        make_morph_repo(tmp.path());

        setup_aoe(tmp.path(), &AoeSetupOpts::default()).unwrap();
        setup_aoe(tmp.path(), &AoeSetupOpts::default()).unwrap();
        setup_aoe(tmp.path(), &AoeSetupOpts::default()).unwrap();

        let cfg =
            fs::read_to_string(tmp.path().join(".agent-of-empires/config.toml")).unwrap();
        let parsed: toml_edit::DocumentMut = cfg.parse().expect("valid TOML");

        let env = parsed["sandbox"]["environment"].as_array().unwrap();
        let count_morph_workspace = env
            .iter()
            .filter(|v| v.as_str() == Some("MORPH_WORKSPACE"))
            .count();
        assert_eq!(count_morph_workspace, 1, "MORPH_WORKSPACE should appear once");

        let vols = parsed["sandbox"]["extra_volumes"].as_array().unwrap();
        let count_mount = vols
            .iter()
            .filter(|v| {
                v.as_str()
                    .map(|s| s == "/usr/local/bin/morph:/usr/local/bin/morph:ro")
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(count_mount, 1, "morph binary mount should appear once");

        let on_create = parsed["hooks"]["on_create"].as_array().unwrap();
        let aoe_create_lines = on_create
            .iter()
            .filter(|v| {
                v.as_str()
                    .map(|s| s.contains("aoe-create:"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(aoe_create_lines, 1, "aoe-create commit line should appear once");
    }
}
