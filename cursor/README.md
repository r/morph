# Cursor integration for Morph

This directory holds the **canonical** Cursor hook scripts and config used by the morph repo. All Cursor-related support lives in the tree here and under `.cursor/`.

## Layout

- **`cursor/`** (this dir): Hook scripts and this README. Referenced by `.cursor/hooks.json`.
- **`.cursor/hooks.json`**: Tells Cursor to run `../cursor/morph-record-prompt.sh` and `../cursor/morph-record-stop.sh` on lifecycle events.
- **`.cursor/rules/`**: Cursor rules (e.g. morph-record.mdc).
- **`.cursor/mcp.json`**: Optional MCP server config for this project.

## Debug and log files

When hooks run, they write under **`.morph/hooks/`** so you can separate “Cursor called us” from “Morph accepted the run.”

| Location | Purpose |
|----------|--------|
| **`.morph/hooks/logs/cursor-invoke.log`** | One line per hook run: timestamp, hook name, `conversation_id`. **Proves Cursor is invoking the scripts.** |
| **`.morph/hooks/logs/morph-record.log`** | One line per successful record: timestamp, `conversation_id`, `run_hash`. **Proves Morph received and stored the run.** |
| **`.morph/hooks/debug/last-beforeSubmitPrompt.json`** | Last payload Cursor sent to `beforeSubmitPrompt` (prompt truncated). Inspect to see shape and content. |
| **`.morph/hooks/debug/last-stop.json`** | Last payload Cursor sent to `stop`. |

**How to verify:**

1. **Cursor → scripts:** After submitting a prompt or stopping a task, check `cursor-invoke.log` for a new line. If it’s there, Cursor called the hook.
2. **Scripts → Morph:** After a stop that had pending prompts, check `morph-record.log` for a new line with a `run_hash`. If it’s there, the script called `morph run record` and Morph stored the run.

In the morph repo, `.morph/` is already in `.gitignore`, so these log and debug files are not committed.
