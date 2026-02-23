# Setting Up Morph with Claude Code

This page is the full reference for using Morph in Claude Code. For a single canonical installation flow (binaries, init, then IDE), see **[Installation](INSTALLATION.md)**.

**What you get:** Prompts and Claude’s replies stored as **Runs** with **Traces** in the Morph object store. Always-on recording via hooks (no need for the agent to call a tool). File tree commits via MCP tools or CLI, independent of Git.

---

## Quick start (installation order)

1. **Install the Morph binaries** — see [Installation § Install the Morph binaries](INSTALLATION.md#1-install-the-morph-binaries).
2. **Configure MCP** in Claude Code so the morph server is connected (Section 1 below).
3. **Initialize** a Morph repo: `morph init` — see [Installation § Initialize a Morph repo](INSTALLATION.md#2-initialize-a-morph-repo).
4. **Enable hooks** so Claude Code records every prompt and response (Section 2 below).

---

## 1. MCP configuration

Claude Code connects to tools via [MCP](https://code.claude.com/docs/en/mcp). Add the Morph server so the agent can use `morph_record_session`, `morph_stage`, `morph_commit`, etc.

If `morph-mcp` is on your PATH, add it in your MCP configuration (e.g. via `claude mcp add` or your settings file). Example for a project-level config:

**Project:** `.claude/settings.json` (or merge into existing):

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

If Claude Code can’t find the binary, use the full path (e.g. `"/usr/local/bin/morph-mcp"` or `"$HOME/.cargo/bin/morph-mcp"`). Restart or reload Claude Code after changing MCP config.

### Workspace path

If the MCP server’s working directory isn’t your project root, set `MORPH_WORKSPACE` in the server config so Morph finds the repo:

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

---

## 2. Always-on recording with hooks

Claude Code hooks run on lifecycle events. Use **UserPromptSubmit** and **Stop** so every prompt and full response is recorded without the agent calling a tool.

- **UserPromptSubmit** — Fires when you submit a prompt; payload includes `prompt` and `session_id`.
- **Stop** — Fires when Claude finishes responding; payload includes **`last_assistant_message`** (the full response text).

### Hook scripts

Copy the Morph hook scripts into your project so Claude Code can run them:

- From the Morph repo, copy the contents of `claude-code/hooks/` into your project’s **`.claude/hooks/`** directory (e.g. `morph-record-prompt.sh`, `morph-record-stop.sh`).
- Make the scripts executable: `chmod +x .claude/hooks/morph-record-prompt.sh .claude/hooks/morph-record-stop.sh`.

### Hooks configuration

In **`.claude/settings.json`** (or `~/.claude/settings.json`), add a `hooks` section. Paths are relative to the project root:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": ".claude/hooks/morph-record-prompt.sh"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": ".claude/hooks/morph-record-stop.sh"
          }
        ]
      }
    ]
  }
}
```

If you already have other hooks, merge these entries into the existing `UserPromptSubmit` and `Stop` arrays.

- **UserPromptSubmit** appends the prompt to `.morph/hooks/pending-<session_id>.jsonl`.
- **Stop** reads that file, builds a Run + Trace using the prompts and `last_assistant_message`, runs `morph run record`, then deletes the pending file.

Hook output and logs go under `.morph/hooks/logs/` and `.morph/hooks/debug/` (see [Debugging](#debugging) below).

---

## 3. Initialize a Morph repo

In your project root:

```bash
morph init
```

This creates `.morph/` (objects, refs, config, prompts, runs, traces, etc.). No other directories are modified.

### Verify

1. In Claude Code, confirm the Morph MCP server is connected (per your MCP setup).
2. Submit a prompt and let Claude respond; then check that a new run appears, e.g. `ls .morph/objects/` or inspect `.morph/runs/` and `.morph/hooks/logs/morph-record.log`.

---

## 4. Committing the filesystem

Same as with Cursor:

- **Stage:** `morph add .` (CLI) or `morph_stage` (MCP)
- **Commit:** `morph commit -m "message"` (CLI) or `morph_commit` (MCP)

---

## Debugging

When hooks run, they write under `.morph/hooks/`:

| File | Purpose |
|------|---------|
| `.morph/hooks/logs/claude-invoke.log` | Claude Code invoked the hook (one line per event) |
| `.morph/hooks/logs/morph-record.log` | Morph stored a run (one line per successful record, with run hash) |
| `.morph/hooks/debug/last-UserPromptSubmit.json` | Last UserPromptSubmit payload (prompt may be truncated) |
| `.morph/hooks/debug/last-Stop.json` | Last Stop payload (response may be truncated) |

If recording doesn’t happen, ensure:

- `morph` and `morph-mcp` are on PATH (or use full paths in config).
- The project has been initialized with `morph init` (`.morph/` exists).
- Hook script paths in `.claude/settings.json` are correct and the scripts are executable.
- Claude Code was restarted or hooks were reloaded after config changes (Claude Code snapshots hooks at startup; some changes require a new session).

---

## Reference

- [Hooks reference](https://code.claude.com/docs/en/hooks) — Event list, input schemas, and config.
- [Connect Claude Code to tools via MCP](https://code.claude.com/docs/en/mcp) — MCP setup.
- [Installation](INSTALLATION.md) — Canonical install flow for all IDEs.
- [CURSOR-SETUP.md](CURSOR-SETUP.md) — Cursor-specific setup (same binaries and MCP server).
