# Claude Code Integration Scripts

Hook scripts for recording Morph sessions from Claude Code lifecycle events.

**Full setup guide:** [docs/CLAUDE-CODE-SETUP.md](../docs/CLAUDE-CODE-SETUP.md)

## Files

| File | Event | Purpose |
|------|--------|---------|
| `hooks/morph-record-prompt.sh` | `UserPromptSubmit` | Appends prompt to `.morph/hooks/pending-<session_id>.jsonl` |
| `hooks/morph-record-stop.sh` | `Stop` | Parses structured data from the payload (transcript_path or conversation array), builds Run + Trace, runs `morph run record` |

## What the hooks record

The stop hook checks for structured data in the Claude Code payload:

- **`transcript_path`** — JSONL file with the full conversation, parsed the same way as Cursor hooks (tool calls, file reads/edits, shell commands → structured events)
- **`conversation`** — Array of message objects with tool use/result content blocks
- **Token usage** (`input_tokens`, `output_tokens`, `total_tokens`) → stored in `run.environment.parameters`

Falls back to `last_assistant_message` if no structured data is available.

## Config

- **`.claude/settings.json`** — Add the `hooks` block and ensure `morph-mcp` is in `mcpServers` (see [CLAUDE-CODE-SETUP.md](../docs/CLAUDE-CODE-SETUP.md)).
- Copy (or symlink) the contents of `hooks/` into your project's **`.claude/hooks/`** so the paths in settings resolve.

## Logs

Hook scripts write to `.morph/hooks/logs/` and `.morph/hooks/debug/`. See [CLAUDE-CODE-SETUP.md § Debugging](../docs/CLAUDE-CODE-SETUP.md#debugging).
