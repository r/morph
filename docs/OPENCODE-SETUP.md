# Setting Up Morph with OpenCode

This page is the full reference for using Morph in [OpenCode](https://open-code.ai). For a single canonical installation flow (binaries, init, then IDE), see **[Installation](INSTALLATION.md)**.

**What you get:** Prompts and model replies stored as immutable **Runs** with **Traces** in the Morph object store. File tree commits via MCP tools or CLI, independent of Git. **Agent-driven recording** via `AGENTS.md` instructions, plus an optional **plugin** for always-on recording.

---

## Quick start (installation order)

1. **Install the Morph binaries** — see [Installation § Install the Morph binaries](INSTALLATION.md#1-install-the-morph-binaries).
2. **Initialize** a Morph repo: `morph init` — see [Installation § Initialize a Morph repo](INSTALLATION.md#2-initialize-a-morph-repo).
3. **Run setup** to install MCP config, agent instructions, and the recording plugin:

```bash
morph setup opencode
```

This writes (or merges into) `opencode.json`, `AGENTS.md`, and `.opencode/plugins/morph-record.ts` in your project. Then open the project in OpenCode; ensure `morph` and `morph-mcp` are on your PATH. No manual MCP or agent config needed.

---

## 1. Configure the MCP Server in OpenCode

OpenCode connects to tools via [MCP](https://open-code.ai/docs/en/mcp-servers). `morph setup opencode` adds the Morph server automatically. If you prefer to do it manually, add the following to `opencode.json` in your project root:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "morph": {
      "type": "local",
      "command": ["morph-mcp"],
      "environment": {
        "MORPH_WORKSPACE": "/absolute/path/to/your/project"
      }
    }
  }
}
```

If OpenCode can't find `morph-mcp`, use the full path (e.g. `["/usr/local/bin/morph-mcp"]` or `["/Users/you/.cargo/bin/morph-mcp"]`).

### Setting a default workspace

`MORPH_WORKSPACE` tells the MCP server where your `.morph/` directory lives. Without it, the server uses its working directory, which may not be your project root.

Resolution order: tool call `workspace_path` argument → `MORPH_WORKSPACE` env → process cwd.

---

## 2. Agent instructions (AGENTS.md)

OpenCode uses `AGENTS.md` for project-level instructions (similar to Cursor rules or Claude Code's `CLAUDE.md`). `morph setup opencode` writes an `AGENTS.md` that tells the agent to:

- Call `morph_record_session` after every substantive task
- Include metrics when committing
- Follow eval-driven development practices

If you already have an `AGENTS.md`, the Morph section is appended (not duplicated on re-run).

To reference it from `opencode.json`, ensure the instructions array includes it:

```json
{
  "instructions": ["AGENTS.md"]
}
```

`morph setup opencode` adds this automatically.

---

## 3. Always-on recording with the plugin

OpenCode supports [plugins](https://open-code.ai/docs/en/plugins) that hook into session lifecycle events. `morph setup opencode` installs a plugin at `.opencode/plugins/morph-record.ts` that:

- Tracks prompts and responses via `message.updated` events
- On `session.idle`, calls `morph run record-session` to persist the turn as a Morph Run + Trace

This is **best-effort** supplementary recording. The primary recording path is agent-driven (the agent calls `morph_record_session` via MCP as instructed in `AGENTS.md`). The plugin catches turns where the agent doesn't call the tool.

---

## 4. Initialize a Morph repo (if needed)

If you haven't already, run `morph init` in your project root (see [Installation](INSTALLATION.md#2-initialize-a-morph-repo)). Then verify: ask the agent to call `morph_record_session` with a test prompt/response and confirm files under `.morph/objects/`.

---

## 5. Committing the filesystem

When you want a snapshot:

1. **Stage:** `morph add .` (CLI) or `morph_stage` (MCP tool)
2. **Commit:** `morph commit -m "message"` (CLI) or `morph_commit` (MCP tool)

`--pipeline` and `--eval-suite` are optional; they default to the identity pipeline and empty eval suite, making Morph work as a plain VCS.

---

## 6. MCP tool reference

| Tool | Purpose |
|------|---------|
| `morph_init` | Create a Morph repo |
| `morph_record_session` | Record prompt + response as Run + Trace (one call, no files needed) |
| `morph_record_run` | Ingest a Run from a JSON file (with optional trace/artifact files) |
| `morph_record_eval` | Ingest metrics from a JSON file |
| `morph_stage` | Stage files into the object store (like `git add`) |
| `morph_commit` | Create a commit (file tree + optional pipeline/eval contract) |
| `morph_annotate` | Attach metadata to any object |
| `morph_branch` | Create a branch at HEAD |
| `morph_checkout` | Switch HEAD and restore working tree |
| `morph_status` | Show working-space status (new/tracked files) |
| `morph_log` | Show commit history from HEAD or a named ref |
| `morph_show` | Show a stored object as pretty JSON |
| `morph_diff` | Compare two commits/refs and show file-level changes |
| `morph_merge` | Merge a branch (requires behavioral dominance) |

All tools accept optional `workspace_path`.

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| "not a morph repository" | Run `morph init` in the project root, or set `MORPH_WORKSPACE` in `opencode.json` under `mcp.morph.environment`. |
| MCP tools not available | Check `opencode mcp list` for server status. Ensure `morph-mcp` is on PATH or use the full path in `opencode.json`. |
| Sessions not recorded | See [Debugging: nothing recorded](#debugging-nothing-recorded) below. |
| Plugin not loaded | Ensure `.opencode/plugins/morph-record.ts` exists. OpenCode loads plugins from `.opencode/plugins/` at startup; restart if you added the file after launch. |

### Debugging: nothing recorded

If OpenCode detects the MCP server but `.morph/runs/` stays empty after a prompt, work through these checks:

1. **Verify the repo is initialized:**
   ```bash
   ls .morph/
   ```
   You should see `objects/`, `runs/`, `traces/`, `prompts/`, and `version`. If missing, run `morph init`.

2. **Test recording manually from the project root:**
   ```bash
   morph run record-session --prompt "test" --response "test"
   ```
   This should print a 64-character hash. If it errors, fix the reported issue first (usually a missing `.morph/` directory or `morph` not on PATH).

3. **Check the plugin log.** After at least one OpenCode prompt+response cycle, check:
   ```bash
   cat .morph/hooks/logs/opencode-plugin.log
   ```
   - **"plugin loaded"** — confirms OpenCode loaded the plugin.
   - **"captured prompt" / "captured response"** — the plugin saw `message.updated` events.
   - **"recorded session via CLI"** — the CLI call succeeded.
   - **"error recording session: ..."** — the CLI call failed; the error message tells you why.
   - **"session.idle fired but no pending prompt/response"** — the plugin received the idle event but had no prompt/response to record. This means `message.updated` events aren't arriving as expected; check that `.opencode/plugins/morph-record.ts` is up to date (`morph setup opencode` to refresh).

4. **Reinstall the plugin** if the log file doesn't appear at all:
   ```bash
   morph setup opencode
   ```
   Then restart OpenCode. The setup command overwrites `.opencode/plugins/morph-record.ts` with the latest version.

5. **Check the agent-driven path.** The plugin is a fallback; the primary recording path is the agent calling `morph_record_session` via MCP as instructed in `AGENTS.md`. If the agent model isn't following those instructions, rely on the plugin. If both fail, test the MCP tool directly by asking the agent: *"Call morph_record_session with prompt 'test' and response 'test' and tell me what it returns."*

---

## Reference

- [OpenCode MCP Servers](https://open-code.ai/docs/en/mcp-servers) — MCP setup and management.
- [OpenCode Rules (AGENTS.md)](https://open-code.ai/docs/en/rules) — Custom instructions.
- [OpenCode Plugins](https://open-code.ai/docs/en/plugins) — Plugin system and events.
- [Installation](INSTALLATION.md) — Canonical install flow for all IDEs.
- [CURSOR-SETUP.md](CURSOR-SETUP.md) — Cursor-specific setup (same binaries and MCP server).
- [CLAUDE-CODE-SETUP.md](CLAUDE-CODE-SETUP.md) — Claude Code-specific setup.
