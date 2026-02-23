# Setting Up Morph with Cursor

This page is the full reference for using Morph in Cursor. For a single canonical installation flow (binaries, init, then IDE), see **[Installation](INSTALLATION.md)**.

**What you get:** Prompts and model replies stored as immutable **Runs** with **Traces** in the Morph object store. File tree commits via MCP tools or CLI, independent of Git. **Always-on recording** via hooks so every prompt and response is captured without relying on the agent.

---

## Quick start (installation order)

1. **Install the Morph binaries** — see [Installation § Install the Morph binaries](INSTALLATION.md#1-install-the-morph-binaries).
2. **Configure MCP** in Cursor so the morph server is connected (Section 1 below).
3. **Initialize** a Morph repo: `morph init` — see [Installation § Initialize a Morph repo](INSTALLATION.md#2-initialize-a-morph-repo).
4. **Enable hooks** so Cursor records every prompt and response automatically (Section 3 below).

---

## 1. Configure the MCP Server in Cursor

Open **Cursor Settings → MCP** and add the Morph server. If `morph-mcp` is on your PATH:

```json
{
  "mcpServers": {
    "morph": {
      "command": "morph-mcp",
      "args": []
    }
  }
}
```

If Cursor can't find it, use the full path (e.g. `"/usr/local/bin/morph-mcp"`). Restart Cursor after changing MCP config.

### Setting a default workspace

Cursor may start the MCP server with a cwd that isn't your project root, causing "not a morph repository" errors. Fix by setting `MORPH_WORKSPACE` in your **project-level** `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "morph": {
      "command": "morph-mcp",
      "args": [],
      "env": {
        "MORPH_WORKSPACE": "/absolute/path/to/your/project"
      }
    }
  }
}
```

Resolution order: tool call `workspace_path` argument → `MORPH_WORKSPACE` env → `CURSOR_WORKSPACE_FOLDER` env → process cwd.

---

## 2. Initialize a Morph repo (if needed)

If you haven’t already, run `morph init` in your project root (see [Installation](INSTALLATION.md#2-initialize-a-morph-repo)). Then verify: Cursor Settings → MCP shows the morph server connected; you can ask the agent to call `morph_record_session` with test prompt/response and confirm files under `.morph/objects/`.

---

## 3. Recording sessions

### Hook-based recording (recommended — always-on)

Cursor hooks run on lifecycle events; the agent cannot skip them. Use **beforeSubmitPrompt**, **afterAgentResponse**, and **stop** so every prompt and full response is recorded without any agent cooperation.

Create or edit `.cursor/hooks.json` in your project (paths are relative to project root):

```json
{
  "version": 1,
  "hooks": {
    "beforeSubmitPrompt": [{"command": "cursor/morph-record-prompt.sh"}],
    "afterAgentResponse": [{"command": "cursor/morph-record-response.sh"}],
    "stop": [{"command": "cursor/morph-record-stop.sh"}]
  }
}
```

- **beforeSubmitPrompt** — Saves the prompt to `.morph/hooks/pending-<conversation_id>.jsonl`.
- **afterAgentResponse** — Receives the full agent response in the payload; builds a Run + Trace with prompt(s) and response, runs `morph run record`, then clears the pending file.
- **stop** — Fallback: if a pending file still exists (e.g. `afterAgentResponse` didn’t fire), records a run with placeholder response text so no prompt is lost.

Copy the hook scripts from the Morph repo into your project’s `cursor/` directory, or symlink them, so that `cursor/morph-record-prompt.sh` etc. resolve correctly. Make them executable: `chmod +x cursor/morph-record-*.sh`.

### Agent-driven recording (optional)

You can also add a Cursor rule so the agent calls `morph_record_session` after a task. Useful if you want the agent to record explicitly or if hooks are not configured. Create `.cursor/rules/morph-record.mdc`:

```markdown
---
description: Record Morph sessions with full prompt and response when a task is done
globs: ["**/*"]
alwaysApply: true
---

When you complete a substantive task in this project and the project has a `.morph` directory:

1. Call the MCP tool **morph_record_session** as the **last action** before you end your reply.
2. **Parameters:** **prompt** = the user's exact request; **response** = your **entire** reply (do not truncate).

Do not skip this step when the user asked for code or changes and you have finished the work.
```

Hooks and the rule can both be used; hooks guarantee recording even when the agent doesn’t call the tool.

### Debugging hooks

When hooks run, they write to `.morph/hooks/` for diagnostics:

| File | What it proves |
|------|---------------|
| `.morph/hooks/logs/cursor-invoke.log` | Cursor is calling the hook scripts (one line per invocation) |
| `.morph/hooks/logs/morph-record.log` | Morph stored the run (one line per successful record, with run hash) |
| `.morph/hooks/debug/last-beforeSubmitPrompt.json` | Last payload from `beforeSubmitPrompt` |
| `.morph/hooks/debug/last-afterAgentResponse.json` | Last payload from `afterAgentResponse` (response text truncated in file) |
| `.morph/hooks/debug/last-stop.json` | Last payload from `stop` |

---

## 4. Cursor Marketplace (plugin packaging)

The [Cursor Marketplace](https://cursor.com/marketplace) supports **plugins** that bundle rules, MCP config, and **hooks** (including scripts). A Morph plugin can provide rules (e.g. `morph-record.mdc`), `hooks.json`, and the three hook scripts so that “add Morph to Cursor” is one click after the plugin is installed.

- **What the plugin ships:** Rules, hooks config, and hook scripts. Marketplace policy allows scripts but **no binaries**.
- **What you still do:** Install the Morph binaries and run `morph init` (see [Installation](INSTALLATION.md)). The plugin only configures Cursor.
- **Listing:** The marketplace is curated; plugins are submitted for review. When a Morph plugin is available, you can install it from the marketplace and then ensure binaries + init are done.

---

## 5. Committing the filesystem

When you want a snapshot:

1. **Stage:** `morph add .` (CLI) or `morph_stage` (MCP tool)
2. **Commit:** `morph commit -m "message"` (CLI) or `morph_commit` (MCP tool)

`--program` and `--eval-suite` are optional; they default to the identity program and empty eval suite, making Morph work as a plain VCS.

---

## 6. MCP tool reference

| Tool | Purpose |
|------|---------|
| `morph_init` | Create a Morph repo |
| `morph_record_session` | Record prompt + response as Run + Trace (one call, no files needed) |
| `morph_record_run` | Ingest a Run from a JSON file (with optional trace/artifact files) |
| `morph_record_eval` | Ingest metrics from a JSON file |
| `morph_stage` | Stage files into the object store (like `git add`) |
| `morph_commit` | Create a commit (file tree + optional program/eval contract) |
| `morph_annotate` | Attach metadata to any object |
| `morph_branch` | Create a branch at HEAD |
| `morph_checkout` | Switch HEAD and restore working tree |

All tools accept optional `workspace_path`. To get a run's prompt from the CLI, run **`morph prompt latest [REF]`** in the repo. Ref is like Git: **`latest`** (default, most recent run), **`latest~N`** or **`latest-N`** (Nth run back, e.g. `latest~1` = previous), or a **64-char run hash** to show that run's prompt. If omitted, uses the resolved workspace (see section 2).

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| "not a morph repository" | Run `morph init` in the project root, or set `MORPH_WORKSPACE` in `.cursor/mcp.json` |
| MCP tools not available | Check Cursor Settings -> MCP for server status. Fix `command` path. Restart Cursor. |
| `spawn ... ENOENT` | Path in `mcp.json` doesn't exist. Run `which morph-mcp` to find it. |
| Sessions not recorded | Use hook-based recording (Section 3): add `.cursor/hooks.json` with `beforeSubmitPrompt`, `afterAgentResponse`, and `stop`. Or add `.cursor/rules/morph-record.mdc` with `alwaysApply: true` so the agent calls `morph_record_session`. |
| Empty `.morph/prompts/` | A successful `morph_record_session` writes to `prompts/`, `runs/`, `traces/`, and `objects/`. If all empty, the tool isn't being called. |
