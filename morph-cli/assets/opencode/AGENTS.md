# Morph — Behavioral Version Control

This project uses [Morph](https://github.com/morphcloud/morph) for behavioral version control. Morph tracks every prompt, response, and file-tree commit independently of Git.

## Recording sessions

When you complete a substantive task in this project and the project has a `.morph` directory:

1. **You MUST call** the MCP tool **morph_record_session** as the **last action** before you end your reply. Do not conclude your turn without calling it.

2. **Parameters:**
   - **prompt**: The user's exact request (the message that started this turn or task). Copy it verbatim.
   - **response**: Your **entire** reply to the user in this turn — from the first character to the last. Do not truncate, summarize, or omit any part.
   - Optionally set **workspace_path** to the project root if the tool returns "not a morph repository".

3. A successful call stores objects in `.morph/objects/`, `.morph/runs/`, `.morph/traces/`, and `.morph/prompts/`.

Do not skip this step when the user asked for code or changes and you have finished the work.

## Behavioral commits

When committing via MCP (`morph_commit`), include metrics from recent test/eval runs as a JSON object:

```json
{"tests_passed": 42, "tests_total": 42, "pass_rate": 1.0}
```

## Eval-driven development

Every code change should include tests. After running tests, record evaluation metrics via `morph_record_eval` or `morph eval record`. Include `tests_total`, `tests_passed`, and `pass_rate` at minimum. Commits without metrics bypass Morph's behavioral merge gating.

## Available MCP tools

| Tool | Purpose |
|------|---------|
| `morph_record_session` | Record prompt + response as Run + Trace |
| `morph_stage` | Stage files (like `git add`) |
| `morph_commit` | Create a commit with optional metrics |
| `morph_status` | Show working-space status |
| `morph_log` | Show commit history |
| `morph_diff` | Compare two commits |
| `morph_branch` | Create a branch |
| `morph_checkout` | Switch branches |
| `morph_merge` | Merge (requires behavioral dominance) |
