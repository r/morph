# Installation

This guide gets you from zero to a working Morph setup: binaries installed, repo initialized, and your IDE (Cursor or Claude Code) recording every prompt and response and able to commit the filesystem.

**What you get:** Prompts and model replies stored as immutable **Runs** with **Traces** in the Morph object store. File tree snapshots via `morph commit` or MCP tools, independent of Git. Always-on recording via IDE hooks so you don’t depend on the agent calling a tool.

---

## 1. Install the Morph binaries

You need two executables: **`morph`** (CLI) and **`morph-mcp`** (MCP server used by Cursor and Claude Code).

**From source (Rust):**

```bash
git clone <morph-repo-url>

cd morph
cargo install --path morph-cli
cargo install --path morph-mcp
```

This installs to `~/.cargo/bin`. Ensure that directory is on your PATH.

**Verify:** Run `morph --help` and `morph-mcp --version`. Running `morph-mcp` with no arguments will appear to hang — it’s waiting for an IDE to connect over stdio; that’s expected.

*Other distribution channels (e.g. npm, Homebrew) may be added later; the CLI and MCP server are the same regardless.*

---

## 2. Initialize a Morph repo

In the root of the project you want to track:

```bash
cd /path/to/your/project
morph init
```

This creates a `.morph/` directory (objects, refs, config, prompts, runs, traces). Nothing else is modified. You only need to do this once per project.

---

## 3. Set up your IDE

Morph works with **Cursor** and **Claude Code**. Each IDE uses the same `morph-mcp` server for MCP tools and provides **hooks** so every prompt and response is recorded automatically.

| IDE | Full guide | What you configure |
|-----|------------|--------------------|
| **Cursor** | [CURSOR-SETUP.md](CURSOR-SETUP.md) | MCP server in Cursor settings; hooks (beforeSubmitPrompt, afterAgentResponse, stop) and hook scripts in the project. |
| **Claude Code** | [CLAUDE-CODE-SETUP.md](CLAUDE-CODE-SETUP.md) | MCP server in Claude Code config; hooks (UserPromptSubmit, Stop) and hook scripts in the project. |

### Cursor

1. **Add the Morph MCP server** in Cursor (Settings → MCP) so the agent can use `morph_record_session`, `morph_stage`, `morph_commit`, etc.
2. **Enable hooks** so Cursor records every prompt and response: add `.cursor/hooks.json` and the three hook scripts (see [CURSOR-SETUP.md](CURSOR-SETUP.md#3-recording-sessions)). Hooks use **beforeSubmitPrompt** (capture prompt), **afterAgentResponse** (capture full response), and **stop** (fallback).

**Cursor Marketplace:** A Morph **plugin** can bundle rules, hooks config, and hook scripts so “add Morph to Cursor” is one step after the plugin is installed. The [Cursor Marketplace](https://cursor.com/marketplace) allows plugins to ship scripts and MCP config but **not binaries** — so you still install the Morph binaries (step 1) and run `morph init` (step 2) yourself. The plugin only configures Cursor. The marketplace is curated; plugins are submitted for review. When a Morph plugin is listed, you’ll be able to install it from the marketplace and then add the binaries + init if you haven’t already.

### Claude Code

1. **Add the Morph MCP server** in Claude Code’s MCP configuration (e.g. in `.claude/settings.json`).
2. **Enable hooks** so Claude Code records every prompt and response: add the Morph hook scripts under `.claude/hooks/` and a `hooks` section in `.claude/settings.json` for **UserPromptSubmit** and **Stop** (see [CLAUDE-CODE-SETUP.md](CLAUDE-CODE-SETUP.md)).

---

## 4. Verify

- **MCP:** In your IDE, confirm the Morph MCP server is connected (Cursor: Settings → MCP; Claude Code: your MCP config).
- **Recording:** Send a prompt and let the agent respond. Check that a run was stored: e.g. `ls .morph/objects/` or inspect `.morph/hooks/logs/morph-record.log`.
- **Commit:** Run `morph add .` and `morph commit -m "first snapshot"` (or use the MCP tools from the IDE).

---

## 5. Next steps

- **Commit the filesystem:** [CURSOR-SETUP.md § Committing](CURSOR-SETUP.md#5-committing-the-filesystem) / [CLAUDE-CODE-SETUP.md § Committing](CLAUDE-CODE-SETUP.md#4-committing-the-filesystem).
- **Use Morph with Git:** [MORPH-AND-GIT.md](MORPH-AND-GIT.md).
- **MCP tool reference:** [CURSOR-SETUP.md § MCP Tool Reference](CURSOR-SETUP.md#6-mcp-tool-reference) (same tools in Claude Code).

---

## Troubleshooting

| Symptom | What to do |
|--------|------------|
| **“not a morph repository”** | Run `morph init` in the project root. If the IDE uses a different cwd, set `MORPH_WORKSPACE` in the MCP config to the project path (see IDE setup guide). |
| **MCP server not found / ENOENT** | Ensure `morph-mcp` is on PATH or use the full path in MCP config. Restart the IDE after changing config. |
| **Sessions not recorded** | Confirm hooks are configured and hook script paths are correct. Cursor: `.cursor/hooks.json` with beforeSubmitPrompt, afterAgentResponse, stop; scripts in `cursor/`. Claude Code: `hooks` in `.claude/settings.json`; scripts in `.claude/hooks/`. See the IDE guide’s debugging section (e.g. `.morph/hooks/logs/`). |
| **Empty `.morph/prompts/` or no new runs** | If you rely on the agent calling `morph_record_session`, ensure the Cursor rule is present and applied. Prefer hook-based recording so every turn is captured without agent cooperation. |

For IDE-specific issues (workspace path, hook payloads, script paths), see the full [Cursor](CURSOR-SETUP.md#troubleshooting) or [Claude Code](CLAUDE-CODE-SETUP.md#debugging) setup guide.
