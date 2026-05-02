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
morph init                          # writes a relaxed RepoPolicy:
                                    #   required_metrics: []
                                    #   merge_policy: dominance
                                    #   exempt_origins: ["git-hook"]
```

Reference-mode `morph init` (the only working-tree mode in v0.40+)
ships with a **relaxed** policy: commits without metrics are allowed
to land, but the merge gate still requires evidence on each parent.
This is the right default for getting started — you can run
`morph commit -m "..."` immediately after init without having to
attach metrics every time.

When you're ready to enforce evidence on every commit, opt in:

```bash
morph policy init                   # required_metrics = [tests_total, tests_passed]
# or set your own list:
morph policy require-metrics tests_total tests_passed pass_rate
```

`morph init --bare` (server-side bare repos) keeps the strict default
because servers receive evidence-bearing commits, not author them.

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
re-running. `from-output` takes a positional file argument; pass `-` to
read from stdin:

```bash
morph eval from-output --runner pytest --record ci-output.txt
# or
morph eval from-output --runner pytest --record - < ci-output.txt
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

The everyday flow is **one command**: tell Morph your test suite once,
then plain `morph commit` runs it and attaches the metrics.

```bash
# One-time per repo:
morph config commit.test_command "cargo test --workspace"

# Every commit thereafter:
morph commit -m "implement login"
# running configured test command: cargo test --workspace
# attaching evidence from run a3f2c…: pass_rate=1, tests_passed=42, tests_total=42
# [d4e5f6a7 (cli)] implement login
```

Behavior:

- `commit.test_command` is read from `.morph/config.json` (set per
  repo). The string is split with POSIX-shell quoting, so quoted
  arguments survive (`"pytest -k 'fast and not slow'"`).
- A failing test (non-zero exit) aborts the commit: a failing
  suite is evidence the code is not in a committable state. The
  failing run is still stored, accessible via `morph show <hash>`.
- `morph commit` writes a fresh `LAST_RUN.json` breadcrumb after
  the run and consumes it during the same commit. After success,
  the breadcrumb is cleared so the next commit won't accidentally
  re-attach stale metrics.
- `morph commit` also auto-detects acceptance cases this commit
  added to the default suite (vs HEAD's) and records them as
  `introduces_cases` for `morph merge-plan` provenance.

Flags that gate the auto-run:

- `--no-test` skips the configured command for this commit (audit
  escape hatch when the suite is too slow or you've already
  certified out-of-band).
- `--rerun` forces a fresh test run even when an existing breadcrumb
  is still current. Use after an external state change (env var,
  fixture refresh) means the cached metrics no longer reflect
  reality.
- `--no-auto-run` disables both the configured command **and** the
  breadcrumb pickup. The commit lands metrics-less unless
  `--metrics` / `--from-run` is also supplied.

### When you've already run the suite

Skip the per-commit auto-run by either passing `--no-test` or by
running the suite yourself first; the resulting breadcrumb is
picked up the same way:

```bash
morph eval run -- cargo test --workspace
morph commit -m "implement login"
# (no auto-run because breadcrumb is fresh; metrics come from
#  the breadcrumb you already produced)
```

The breadcrumb is single-use: after a successful commit it's cleared.
If HEAD changes between the run and the commit, the breadcrumb is
invalidated automatically.

### When you need precision

Use the explicit form when you want to attach a specific run, override
metrics, or commit without auto-pickup:

```bash
morph eval run -- cargo test --workspace
# Note the printed run hash, e.g. 3a7b9c…

morph commit \
  -m "implement login" \
  --from-run 3a7b9c… \
  --new-cases login:user_can_login
```

- `--from-run <hash>` attaches that specific run.
- `--new-cases <ids>` overrides the auto-detection. Pass `""` to
  suppress entirely.
- `--no-auto-run` disables the breadcrumb pickup so the commit is
  metrics-less unless `--metrics` / `--from-run` is also passed.

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
