# Cursor Integration Scripts

Hook scripts for recording Morph sessions from Cursor lifecycle events.

**Full setup guide:** [docs/CURSOR-SETUP.md](../docs/CURSOR-SETUP.md)

## Files

| File | Referenced by | Purpose |
|------|--------------|---------|
| `morph-record-prompt.sh` | `.cursor/hooks.json` (`beforeSubmitPrompt`) | Saves prompt text to `.morph/hooks/pending/` |
| `morph-record-stop.sh` | `.cursor/hooks.json` (`stop`) | Builds Run + Trace from pending prompts, calls `morph run record` |

## Related config

| File | Purpose |
|------|---------|
| `.cursor/hooks.json` | Tells Cursor to run the scripts above on lifecycle events |
| `.cursor/rules/morph-record.mdc` | Cursor rule for agent-driven recording via `morph_record_session` |
| `.cursor/mcp.json` | MCP server config (optional `MORPH_WORKSPACE` env) |

## Debug output

Hook scripts write diagnostics to `.morph/hooks/logs/` and `.morph/hooks/debug/`. See [CURSOR-SETUP.md section 4](../docs/CURSOR-SETUP.md#debugging-hooks) for details.
