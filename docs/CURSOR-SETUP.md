# Setting Up Morph with Cursor

Build, install, and configure Morph so Cursor can record sessions and commit the filesystem.

**What you get:**
- Prompts and model replies stored as immutable **Runs** with **Traces** in the Morph object store.
- File tree commits via MCP tools or CLI, independent of Git.

---

## 1. Build and Install

You need two binaries: `morph` (CLI) and `morph-mcp` (MCP server).

```bash
cargo install --path morph-cli
cargo install --path morph-mcp
```

This puts them in `~/.cargo/bin`. Alternatively, `cargo build -r` and copy `target/release/morph` and `target/release/morph-mcp` to any directory on your PATH.

**Verify:** `morph --help` prints CLI usage. `morph-mcp --version` prints the version. (Running `morph-mcp` with no args appears to hang -- it's waiting for Cursor to connect over stdio. That's normal.)

---

## 2. Configure the MCP Server in Cursor

Open **Cursor Settings -> MCP** and add the Morph server. If `morph-mcp` is on your PATH:

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

Resolution order: tool call `workspace_path` argument -> `MORPH_WORKSPACE` env -> `CURSOR_WORKSPACE_FOLDER` env -> process cwd.

---

## 3. Initialize a Morph Repo

```bash
cd /path/to/your/project
morph init
```

Creates `.morph/` (objects, refs, config, prompts, evals, runs, traces). No other directories are modified.

### Verify everything works

1. **Cursor**: Settings -> MCP -- the morph server should show as connected.
2. **Quick test**: In a chat, ask *"Call morph_record_session with prompt 'test' and response 'test'."*
3. **Confirm**: `ls .morph/objects/` should show stored object files.

---

## 4. Recording Sessions

### Agent-driven recording (recommended)

Add a Cursor rule so the agent records after each task. Create `.cursor/rules/morph-record.mdc`:

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

This stores a Run (with Trace) containing the full prompt and response text.

### Hook-based recording (automatic, no agent cooperation needed)

Cursor hooks run scripts on lifecycle events. The agent cannot skip them. Configure in `.cursor/hooks.json`:

```json
{
  "version": 1,
  "hooks": {
    "beforeSubmitPrompt": [{"command": "cursor/morph-record-prompt.sh"}],
    "stop": [{"command": "cursor/morph-record-stop.sh"}]
  }
}
```

- `beforeSubmitPrompt` saves the prompt text to a pending file under `.morph/hooks/`.
- `stop` builds a Run + Trace from pending prompts and calls `morph run record`.

**Limitation:** Hook payloads do not include the model's response text. For full responses, use agent-driven recording (the rule above). The two approaches complement each other.

### Debugging hooks

When hooks run, they write to `.morph/hooks/` for diagnostics:

| File | What it proves |
|------|---------------|
| `.morph/hooks/logs/cursor-invoke.log` | Cursor is calling the hook scripts (one line per invocation) |
| `.morph/hooks/logs/morph-record.log` | Morph stored the run (one line per successful record, with run hash) |
| `.morph/hooks/debug/last-beforeSubmitPrompt.json` | Last payload from `beforeSubmitPrompt` |
| `.morph/hooks/debug/last-stop.json` | Last payload from `stop` |

---

## 5. Committing the Filesystem

When you want a snapshot:

1. **Stage:** `morph add .` (CLI) or `morph_stage` (MCP tool)
2. **Commit:** `morph commit -m "message"` (CLI) or `morph_commit` (MCP tool)

`--program` and `--eval-suite` are optional; they default to the identity program and empty eval suite, making Morph work as a plain VCS.

---

## 6. MCP Tool Reference

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

All tools accept optional `workspace_path`. If omitted, uses the resolved workspace (see section 2).

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| "not a morph repository" | Run `morph init` in the project root, or set `MORPH_WORKSPACE` in `.cursor/mcp.json` |
| MCP tools not available | Check Cursor Settings -> MCP for server status. Fix `command` path. Restart Cursor. |
| `spawn ... ENOENT` | Path in `mcp.json` doesn't exist. Run `which morph-mcp` to find it. |
| Sessions not recorded | Ensure `.cursor/rules/morph-record.mdc` exists with `alwaysApply: true`. |
| Empty `.morph/prompts/` | A successful `morph_record_session` writes to `prompts/`, `runs/`, `traces/`, and `objects/`. If all empty, the tool isn't being called. |
