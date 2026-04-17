# Cursor Integration Scripts

Hook scripts for recording Morph sessions from Cursor lifecycle events. **Canonical location:** `.cursor/` in your project (so the repo root stays Git-style: only `.cursor/`, `.morph/`, `.git/`). Run `morph setup cursor` to install them there.

**Full setup guide:** [docs/CURSOR-SETUP.md](../docs/CURSOR-SETUP.md)

## Files (installed under `.cursor/` by `morph setup cursor`)

| File | Referenced by | Purpose |
|------|--------------|---------|
| `morph-record-prompt.sh` | `.cursor/hooks.json` (`beforeSubmitPrompt`) | Saves prompt + composer mode to `.morph/hooks/pending-<conversation_id>.jsonl` |
| `morph-record-response.sh` | `.cursor/hooks.json` (`afterAgentResponse`) | Parses the agent transcript for structured events (tool calls, file reads/edits, shell commands), builds Run + Trace, calls `morph run record` |
| `morph-record-stop.sh` | `.cursor/hooks.json` (`stop`) | Fallback: same structured parsing as response hook; fires if `afterAgentResponse` didn't |

The copies in this `cursor/` directory mirror `morph-cli/assets/cursor/hooks/` and are kept for reference when developing Morph.

## What the hooks record

When `transcript_path` is available in the Cursor payload (the JSONL file containing the full conversation), the hooks parse it to produce rich structured traces:

- **`tool_use`** items (e.g. `Read`, `Grep`, `Glob`) → `file_read` events with tool name and path
- **`tool_use`** items (e.g. `StrReplace`, `Write`) → `file_edit` events with tool name and path
- **`tool_use`** items (e.g. `Shell`, `Task`, `CallMcpTool`) → `tool_call` events with tool name and input
- **`tool_result`** items → `tool_result` events
- **Text parts** → `user` / `assistant` events
- **Token usage** (`input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`) → stored in `run.environment.parameters`

If `transcript_path` is unavailable or unreadable, the hooks fall back to pending prompts + response text.

## Related config

| File | Purpose |
|------|---------|
| `.cursor/hooks.json` | Tells Cursor to run the scripts above on lifecycle events |
| `.cursor/rules/morph-record.mdc` | Cursor rule for agent-driven recording via `morph_record_session` |
| `.cursor/mcp.json` | MCP server config (optional `MORPH_WORKSPACE` env) |

## Debug output

Hook scripts write diagnostics to `.morph/hooks/logs/` and `.morph/hooks/debug/`. See [CURSOR-SETUP.md § Debugging hooks](../docs/CURSOR-SETUP.md#debugging-hooks) for details.
