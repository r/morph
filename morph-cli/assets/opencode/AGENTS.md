# Morph — Behavioral Version Control

This project uses [Morph](https://github.com/r/morph) for behavioral version
control. Morph tracks every prompt, response, and file-tree commit
independently of Git, and gates merges on **behavioral evidence** — the
metrics produced by the repo's eval suite. Project website:
[r.github.io/morph](https://r.github.io/morph).

## Recording sessions

Session recording is handled automatically by the Morph plugin
(`.opencode/plugins/morph-record.ts`). It runs on `session.idle` and fans
each OpenCode message part out to a structured Morph trace event — `user`,
`assistant`, `reasoning`, `file_read`, `file_edit`, `tool_call`,
`tool_result`, `error`, plus a `usage` event with per-step tokens/cost. The
plugin also opportunistically calls `morph eval from-output --record` when
it sees a `bash` tool call running a known test runner, so test output
turns into metric-bearing `Run` objects without any agent action.

**Do NOT call `morph_record_session` yourself** — the plugin captures
everything. (It actively blocks that tool, so any attempt will fail.) If
you need to verify recording, look at `.morph/hooks/logs/opencode-plugin.log`
and `.morph/hooks/debug/last-record.json`.

## Spec-first, eval-driven development

The **eval suite is the canonical specification**. Unit tests are an
implementation detail. Every behavioral change must:

1. **Start as an acceptance case** — a YAML spec (one case per top-level
   entry) or a cucumber `.feature` (one case per `Scenario:`). The case
   describes user-visible behavior, not implementation.
2. **Be registered in the suite** with `morph eval add-case <file>` (or
   `morph_add_eval_case` via MCP). The first call also wires
   `policy.default_eval_suite`, so subsequent commits inherit it.
3. **Fail before you implement.** Run `morph eval run -- <test command>`
   (or `morph_eval_run` via MCP). The runner captures stdout, parses
   cargo / pytest / vitest / jest / go output, and writes a metric-bearing
   `Run` linked to HEAD.
4. **Pass after you implement.** Iterate until the latest run shows
   `pass_rate: 1.0`.
5. **Commit from the run** — `morph commit -m "..." --from-run <hash>
   --new-cases <case_ids>`. `--from-run` attaches the run's metrics as the
   commit's `observed_metrics`; `--new-cases` records which acceptance
   cases the commit introduced so merge plans can surface case provenance.

Repo policy (the `morph init` default) requires `tests_total` and
`tests_passed` on every commit. `--allow-empty-metrics` exists only as an
audited escape hatch.

## Mid-task self-check

Before you finish a turn, call:

- **`morph_status`** — returns an `Evidence:` block with HEAD metrics,
  default suite case count, recent runs (with/without metrics), and
  working-tree freshness.
- **`morph_eval_gaps`** — returns a structured JSON list of unaddressed
  gaps: `empty_head_metrics`, `empty_default_suite`, `no_recent_run`. Cheap
  to call, easy to read. If it returns anything non-empty, you're not
  done.

## Commit metric shape

When committing via MCP (`morph_commit`), pass metrics as a JSON object
(or use `--from-run` to inherit them from a recorded run):

```json
{"tests_passed": 42, "tests_total": 42, "pass_rate": 1.0}
```

Optional: `build_time_secs`, `coverage_pct`, any domain-specific metrics.

## Available MCP tools

| Tool | Purpose |
|------|---------|
| `morph_stage` | Stage files (like `git add`) |
| `morph_commit` | Create a commit with optional metrics / `--from-run` / `--new-cases` |
| `morph_status` | Show working-space status with the `Evidence:` block |
| `morph_log` | Show commit history |
| `morph_diff` | Compare two commits |
| `morph_branch` | Create a branch |
| `morph_checkout` | Switch branches |
| `morph_merge` | Merge (requires behavioral dominance on every metric) |
| `morph_record_eval` | Attach metrics from a JSON file |
| `morph_eval_from_output` | Parse captured test stdout into metrics; optional `--record` writes a `Run` linked to HEAD |
| `morph_eval_run` | Shell out to a test command, capture+parse, write a `Run` |
| `morph_add_eval_case` | Ingest YAML / cucumber specs into the default `EvalSuite` |
| `morph_eval_suite_from_specs` | Bulk-rebuild the default suite from a directory tree |
| `morph_eval_suite_show` | Print the cases in the default (or specified) suite |
| `morph_eval_gaps` | Structured list of unaddressed behavioral-evidence gaps |

## Why this matters

Morph merges require the merged program to dominate both parents on every
metric in the eval suite. If one branch carries acceptance cases and
metric-bearing runs and the other doesn't, the merge has nothing to compare
against — the gate is bypassed and the entire point of behavioral version
control is lost. Spec-first development makes that comparison possible.
