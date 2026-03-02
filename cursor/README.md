# Cursor Integration Scripts

Hook scripts for recording Morph sessions from Cursor lifecycle events. **Canonical location:** `.cursor/` in your project (so the repo root stays Git-style: only `.cursor/`, `.morph/`, `.git/`). Run `morph setup cursor` to install them there.

**Full setup guide:** [docs/CURSOR-SETUP.md](../docs/CURSOR-SETUP.md)

## Files (installed under `.cursor/` by `morph setup cursor`)

| File | Referenced by | Purpose |
|------|--------------|---------|
| `morph-record-prompt.sh` | `.cursor/hooks.json` (`beforeSubmitPrompt`) | Saves prompt to `.morph/hooks/pending-<conversation_id>.jsonl` |
| `morph-record-response.sh` | `.cursor/hooks.json` (`afterAgentResponse`) | Builds Run + Trace with full response text, calls `morph run record` |
| `morph-record-stop.sh` | `.cursor/hooks.json` (`stop`) | Fallback: builds Run + Trace from pending if no `afterAgentResponse` yet |

The copies in this `cursor/` directory mirror `morph-cli/assets/cursor/hooks/` and are kept for reference when developing Morph.

## Related config

| File | Purpose |
|------|---------|
| `.cursor/hooks.json` | Tells Cursor to run the scripts above on lifecycle events |
| `.cursor/rules/morph-record.mdc` | Cursor rule for agent-driven recording via `morph_record_session` |
| `.cursor/mcp.json` | MCP server config (optional `MORPH_WORKSPACE` env) |

## Debug output

Hook scripts write diagnostics to `.morph/hooks/logs/` and `.morph/hooks/debug/`. See [CURSOR-SETUP.md § Debugging hooks](../docs/CURSOR-SETUP.md#debugging-hooks) for details.
