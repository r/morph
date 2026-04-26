# Installation

This guide gets you from zero to a working Morph setup: binaries installed, repo initialized, and your IDE (Cursor, Claude Code, or OpenCode) recording every prompt and response and able to commit the filesystem.

**What you get:** Rich structured traces of every agent interaction -- tool calls, file reads/edits, shell commands, prompts, and responses -- stored as immutable **Runs** with **Traces** in the Morph object store. File tree snapshots via `morph commit` or MCP tools, independent of Git. Always-on recording via IDE hooks so you don't depend on the agent calling a tool.

---

## 1. Install the Morph binaries

You need two executables: **`morph`** (CLI) and **`morph-mcp`** (MCP server used by Cursor, Claude Code, and OpenCode).

**With Homebrew (macOS):**

```bash
brew tap r/morph
brew install morph
```

This installs both `morph` and `morph-mcp`.

**From source (Rust):**

```bash
git clone <morph-repo-url>

cd morph
cargo install --path morph-cli
cargo install --path morph-mcp
```

This installs to `~/.cargo/bin`. Ensure that directory is on your PATH.

**Verify:** Run `morph --help` and `morph-mcp --version`. Running `morph-mcp` with no arguments will appear to hang — it's waiting for an IDE to connect over stdio; that's expected.

---

## 2. Initialize a Morph repo

In the root of the project you want to track:

```bash
cd /path/to/your/project
morph init
```

This creates a `.morph/` directory (objects, refs, config, prompts, runs, traces). Nothing else is modified. You only need to do this once per project.

### Quick path: one command for Cursor (recommended)

After `morph init`, you can install Cursor MCP config, hooks, and rules in one step:

```bash
morph setup cursor
```

This writes (or merges into) `.cursor/mcp.json`, `.cursor/hooks.json`, `.cursor/rules/*.mdc`, and the hook scripts into `.cursor/` in your project (so only dot-directories like `.cursor/` and `.morph/` are visible, Git-style). Then open the project in Cursor; ensure `morph` and `morph-mcp` are on your PATH. No manual MCP or hook setup needed.

### Quick path: one command for OpenCode

After `morph init`, you can install OpenCode MCP config, agent instructions, and the recording plugin in one step:

```bash
morph setup opencode
```

This writes (or merges into) `opencode.json`, `AGENTS.md`, and `.opencode/plugins/morph-record.ts` in your project. Then open the project in OpenCode; ensure `morph` and `morph-mcp` are on your PATH.

---

## 3. Set up your IDE

Morph works with **Cursor**, **Claude Code**, and **OpenCode**. Each IDE uses the same `morph-mcp` server for MCP tools and provides recording mechanisms so every prompt and response is captured.

| IDE | Full guide | What you configure |
|-----|------------|--------------------|
| **Cursor** | [CURSOR-SETUP.md](CURSOR-SETUP.md) | MCP server in Cursor settings; hooks (beforeSubmitPrompt, afterAgentResponse, stop) and hook scripts in the project. |
| **Claude Code** | [CLAUDE-CODE-SETUP.md](CLAUDE-CODE-SETUP.md) | MCP server in Claude Code config; hooks (UserPromptSubmit, Stop) and hook scripts in the project. |
| **OpenCode** | [OPENCODE-SETUP.md](OPENCODE-SETUP.md) | MCP server in `opencode.json`; `AGENTS.md` for agent instructions; recording plugin in `.opencode/plugins/`. |

### Cursor

1. **Add the Morph MCP server** in Cursor (Settings → MCP) so the agent can use `morph_record_session`, `morph_stage`, `morph_commit`, etc.
2. **Enable hooks** so Cursor records every prompt and response: add `.cursor/hooks.json` and the three hook scripts (see [CURSOR-SETUP.md](CURSOR-SETUP.md#3-recording-sessions)). Hooks use **beforeSubmitPrompt** (capture prompt), **afterAgentResponse** (capture full response), and **stop** (fallback).

**Cursor Marketplace:** A Morph **plugin** can bundle rules, hooks config, and hook scripts so "add Morph to Cursor" is one step after the plugin is installed. The [Cursor Marketplace](https://cursor.com/marketplace) allows plugins to ship scripts and MCP config but **not binaries** — so you still install the Morph binaries (step 1) and run `morph init` (step 2) yourself. The plugin only configures Cursor. The marketplace is curated; plugins are submitted for review. When a Morph plugin is listed, you'll be able to install it from the marketplace and then add the binaries + init if you haven't already.

### Claude Code

1. **Add the Morph MCP server** in Claude Code's MCP configuration (e.g. in `.claude/settings.json`).
2. **Enable hooks** so Claude Code records every prompt and response: add the Morph hook scripts under `.claude/hooks/` and a `hooks` section in `.claude/settings.json` for **UserPromptSubmit** and **Stop** (see [CLAUDE-CODE-SETUP.md](CLAUDE-CODE-SETUP.md)).

### OpenCode

1. **Add the Morph MCP server** in `opencode.json` under the `mcp` key.
2. **Add `AGENTS.md`** so the agent records sessions via MCP after every task.
3. **Install the recording plugin** at `.opencode/plugins/morph-record.ts` for supplementary always-on recording.

Or run `morph setup opencode` to do all three in one step (see [OPENCODE-SETUP.md](OPENCODE-SETUP.md)).

---

## 4. Verify

- **MCP:** In your IDE, confirm the Morph MCP server is connected (Cursor: Settings → MCP; Claude Code: your MCP config; OpenCode: `opencode mcp list`).
- **Recording:** Send a prompt and let the agent respond. Check that a run was stored: e.g. `ls .morph/objects/` or inspect `.morph/hooks/logs/morph-record.log`.
- **Commit:** Run `morph add .` and `morph commit -m "first snapshot"` (or use the MCP tools from the IDE).

---

## 5. Run the hosted service (optional)

For team-wide inspection and collaboration, run the Morph hosted service:

```bash
morph serve                    # serve current repo at http://127.0.0.1:8765
morph serve --port 9000        # custom port
morph serve --repo team=/path  # named multi-repo
```

The service exposes a stable JSON API for browsing commits (with behavioral status), runs, traces, pipelines, certifications, and policy. See [v0-spec.md § 16](v0-spec.md#16-hosted-service-phase-7) for the full API reference.

---

## 6. Next steps

- **Commit the filesystem:** [CURSOR-SETUP.md § Committing](CURSOR-SETUP.md#5-committing-the-filesystem) / [CLAUDE-CODE-SETUP.md § Committing](CLAUDE-CODE-SETUP.md#4-committing-the-filesystem) / [OPENCODE-SETUP.md § Committing](OPENCODE-SETUP.md#5-committing-the-filesystem).
- **Use Morph with Git:** [MORPH-AND-GIT.md](MORPH-AND-GIT.md).
- **Sync between machines:** [MULTI-MACHINE.md](MULTI-MACHINE.md) walks through bare repos, SSH transport, `morph push`/`morph pull`/`morph sync`. If you're hosting the server, see [SERVER-SETUP.md](SERVER-SETUP.md). The merge engine those workflows depend on is documented in [MERGE.md](MERGE.md).
- **MCP tool reference:** [CURSOR-SETUP.md § MCP Tool Reference](CURSOR-SETUP.md#6-mcp-tool-reference) (same tools in all IDEs).
- **Hosted service API:** [v0-spec.md § 16](v0-spec.md#16-hosted-service-phase-7).

---

## Troubleshooting

| Symptom | What to do |
|--------|------------|
| **"not a morph repository"** | Run `morph init` in the project root. If the IDE uses a different cwd, set `MORPH_WORKSPACE` in the MCP config to the project path (see IDE setup guide). |
| **MCP server not found / ENOENT** | Ensure `morph-mcp` is on PATH or use the full path in MCP config. Restart the IDE after changing config. |
| **Sessions not recorded** | Confirm hooks are configured and hook script paths are correct. Cursor: `.cursor/hooks.json` with beforeSubmitPrompt, afterAgentResponse, stop; scripts in `.cursor/`. Claude Code: `hooks` in `.claude/settings.json`; scripts in `.claude/hooks/`. OpenCode: ensure `AGENTS.md` exists and morph MCP server is connected. See the IDE guide's debugging section. |
| **Empty `.morph/prompts/` or no new runs** | If you rely on the agent calling `morph_record_session`, ensure the Cursor rule / `AGENTS.md` is present and applied. Prefer hook-based recording so every turn is captured without agent cooperation. |

For IDE-specific issues (workspace path, hook payloads, script paths), see the full [Cursor](CURSOR-SETUP.md#troubleshooting), [Claude Code](CLAUDE-CODE-SETUP.md#debugging), or [OpenCode](OPENCODE-SETUP.md#troubleshooting) setup guide.
