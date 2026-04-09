# Morph — Behavioral Version Control

This project uses [Morph](https://github.com/morphcloud/morph) for behavioral version control. Morph tracks every prompt, response, and file-tree commit independently of Git.

## Recording sessions

Session recording is handled automatically by the Morph plugin. **Do NOT call `morph_record_session` yourself** — the plugin captures prompts and responses without any agent action.

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
| `morph_stage` | Stage files (like `git add`) |
| `morph_commit` | Create a commit with optional metrics |
| `morph_status` | Show working-space status |
| `morph_log` | Show commit history |
| `morph_diff` | Compare two commits |
| `morph_branch` | Create a branch |
| `morph_checkout` | Switch branches |
| `morph_merge` | Merge (requires behavioral dominance) |
