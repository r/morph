# Eval-Driven Development with Morph

Morph treats the **eval suite as the canonical specification** of what a
project should do. Every commit carries an evaluation contract — the suite
hash plus the metrics observed for that commit — and merges only succeed
when the merged program **dominates** both parents on every metric. The
suite is therefore not a CI nicety; it's the substrate that makes
behavioral version control work.

This document walks the spec-first workflow from a fresh repo to a merged
feature branch.

## 0. Initial setup

```bash
morph init                          # writes a default RepoPolicy:
                                    #   required_metrics: [tests_total, tests_passed]
                                    #   merge_policy: dominance
                                    #   push_gated_branches: []
```

The default policy is opinionated: every commit must carry `tests_total`
and `tests_passed` unless `--allow-empty-metrics` is passed. To relax
or change it, use `morph policy require-metrics <name>...` (pass no
names to disable the gate entirely). Tests can opt out at init time
with the hidden `--no-default-policy` flag.

## 1. Write the acceptance case before the code

User-visible behavior lives in the eval suite. Two formats are supported
out of the box:

- **YAML** — one case per top-level entry. A case minimally needs a
  `name`; the rest is captured into `expected.raw` for human review.
- **Cucumber `.feature`** — one case per `Scenario:`. The feature
  background is preserved per case.

Example (`specs/login.yaml`):

```yaml
- name: user_can_login
  steps:
    - given: a user with email alice@example.com
    - when: they POST /login with their password
    - then: the response body contains an auth token
```

## 2. Register the case in the eval suite

```bash
morph eval add-case specs/login.yaml
```

The first call creates a fresh `EvalSuite`, appends the case, and wires
the suite hash into `policy.default_eval_suite`. Subsequent calls extend
the same suite (deduping by case `id`).

For bulk ingestion of an entire directory tree (mixed YAML + cucumber):

```bash
morph eval suite-from-specs specs/
```

`suite-from-specs` rebuilds the suite from scratch — useful when the
canonical spec lives in version control and you want the suite to track
it exactly.

Sanity-check the suite at any time:

```bash
morph eval suite-show          # human-readable
morph eval suite-show --json   # structured for tooling
```

## 3. Watch the case fail

Run the eval suite via `morph eval run`. This shells out, captures stdout,
parses the runner output (cargo / pytest / vitest / jest / go), and writes
a `Run` object linked to HEAD with the parsed metrics.

```bash
morph eval run -- cargo test --workspace
# prints the new run hash; metrics are stored on the Run.
```

A red run is a healthy starting point — it proves the spec is real.

If you already have captured stdout (e.g. from CI logs), parse it without
re-running:

```bash
cat ci-output.txt | morph eval from-output --runner pytest --record
```

## 4. Implement until it goes green

Iterate. `morph eval run` after each change. Aim for `pass_rate: 1.0` in
the latest run.

Mid-task self-checks:

```bash
morph status                   # includes the Evidence: block
morph eval gaps                # human-readable gap report
morph eval gaps --json         # structured for tooling / hooks
```

`morph_status` (MCP) and `morph_eval_gaps` (MCP) expose the same
information to AI agents. The optional Cursor stop-hook
(`~/.cursor/hooks/morph-record-checks.sh`, installed by `morph setup
cursor`) prints any outstanding gaps to stderr at the end of every agent
turn.

## 5. Commit from the recorded run

```bash
morph eval run -- cargo test --workspace
# Note the printed run hash, e.g. 3a7b9c…

morph commit \
  -m "implement login" \
  --from-run 3a7b9c… \
  --new-cases login:user_can_login
```

- `--from-run` attaches the run's metrics as the commit's
  `observed_metrics`, satisfying the policy and giving the merge gate
  evidence.
- `--new-cases` records which acceptance cases this commit *introduced*
  via an `introduces_cases` annotation. The annotation surfaces in
  `morph merge-plan` so a reviewer can see which cases each branch
  contributed.

If repo policy requires metrics and you genuinely need to commit without
them (rare — e.g. rebasing or backporting), use `--allow-empty-metrics`.
That flag is audited; don't make it your default.

## 6. Merge with case provenance

```bash
morph merge-plan feature
```

```
Merge plan: feature -> main

Current branch (main):
  commit: ab12…
  suite: 5abb5131…
  metrics: tests_passed=812, tests_total=812

Other branch (feature):
  commit: cd34…
  suite: 5abb5131…
  metrics: tests_passed=825, tests_total=825

Union eval suite (3 metrics): …

Reference bar:
  tests_passed >= 825 (maximize)
  tests_total  >= 825 (maximize)

Retired metrics: none

Case provenance:
  main introduces 4 case(s): auth:logout_clears_session, …
  feature introduces 8 case(s): login:user_can_login, …
  Merged candidate must pass all 12 (union) plus existing suite.
```

```bash
morph merge feature \
  -m "merge feature into main" \
  --metrics '{"tests_passed": 825, "tests_total": 825, "pass_rate": 1.0}'
```

If the merged metrics don't dominate both parents on every metric in the
union suite, the merge is rejected with a per-metric explanation —
analogous to a text conflict in Git, but at the semantic level.

## Reference: relevant CLI / MCP surface

| CLI | MCP tool | Purpose |
|-----|----------|---------|
| `morph eval add-case <file>...` | `morph_add_eval_case` | Add YAML/cucumber cases to the default suite. |
| `morph eval suite-from-specs <dir>` | `morph_eval_suite_from_specs` | Rebuild the default suite from a directory. |
| `morph eval suite-show [--suite H] [--json]` | `morph_eval_suite_show` | Print the cases in a suite. |
| `morph eval run -- <cmd>` | `morph_eval_run` | Run a test command and record a metric-bearing `Run`. |
| `morph eval from-output [--runner R] [--record] <file>` | `morph_eval_from_output` | Parse captured stdout into metrics; optional `--record` writes a `Run`. |
| `morph eval record <file.json>` | `morph_record_eval` | Ingest precomputed `{"metrics": {...}}` JSON. |
| `morph eval gaps [--json] [--fail-on-gap]` | `morph_eval_gaps` | Structured list of unaddressed evidence gaps. |
| `morph policy require-metrics <name>...` | (no MCP yet) | Set or clear `policy.required_metrics`. |
| `morph commit ... --from-run H --new-cases ids` | `morph_commit` (params `from_run`, `new_cases`) | Commit with run-derived metrics and case provenance. |
| `morph status` | `morph_status` | Human-readable working-state plus an `Evidence:` block. |
| `morph merge-plan <branch>` | (read-only via `morph_status`-adjacent tooling) | Pre-merge inspection including case provenance. |

## Why this pipeline matters

Behavioral merges require a comparison: the merged program must dominate
both parents on every metric in the union of their suites. That
comparison is only meaningful when each branch carries acceptance cases
*and* metric-bearing runs. Spec-first development is the engineering
practice that makes this guarantee possible. Skip the spec, and the merge
gate has nothing to enforce.
