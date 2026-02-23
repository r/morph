# Claude Code Integration Scripts

Hook scripts for recording Morph sessions from Claude Code lifecycle events.

**Full setup guide:** [docs/CLAUDE-CODE-SETUP.md](../docs/CLAUDE-CODE-SETUP.md)

## Files

| File | Event | Purpose |
|------|--------|---------|
| `hooks/morph-record-prompt.sh` | `UserPromptSubmit` | Appends prompt to `.morph/hooks/pending-<session_id>.jsonl` |
| `hooks/morph-record-stop.sh` | `Stop` | Builds Run + Trace with `last_assistant_message`, runs `morph run record` |

## Config

- **`.claude/settings.json`** — Add the `hooks` block and ensure `morph-mcp` is in `mcpServers` (see [CLAUDE-CODE-SETUP.md](../docs/CLAUDE-CODE-SETUP.md)).
- Copy (or symlink) the contents of `hooks/` into your project’s **`.claude/hooks/`** so the paths in settings resolve.

## Logs

Hook scripts write to `.morph/hooks/logs/` and `.morph/hooks/debug/`. See [CLAUDE-CODE-SETUP.md § Debugging](../docs/CLAUDE-CODE-SETUP.md#debugging).
