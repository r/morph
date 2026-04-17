# morph-e2e

End-to-end tests for morph: **human-readable specs in Gherkin**, executed by **Cucumber**.

You write `.feature` files (Given/When/Then); the harness runs the real `morph` CLI and asserts on exit codes, stdout/stderr, and filesystem.

## Run

From the **repo root** (morph-e2e is in the workspace):

```bash
cargo test -p morph-e2e --test cucumber
```

The workspace builds the `morph` binary from morph-cli when needed. To run from inside `morph-e2e/`, ensure `morph` is on PATH (e.g. `cargo build -p morph-cli` from root first).

## Layout

- **features/** — Gherkin specs (what we're testing, readable by anyone)
  - `init_and_status.feature` — init repo, status, empty repo
  - `add_and_commit.feature` — stage, commit with pipeline and eval suite
  - `prompt_create_materialize.feature` — prompt blob create and materialize
  - `run_record_session.feature` — single agent records a session
  - `concurrent_agents.feature` — two agents record sessions at the same time
  - `branch_and_checkout.feature` — branching and checkout
  - `merge_workflow.feature` — behavioral merge with dominance
  - `behavioral_merge_workflow.feature` — advanced merge scenarios
  - `evidence_backed_commit.feature` — commits with `--from-run` provenance
  - `rollup_workflow.feature` — squash/rollup of commit sequences
  - `run_to_pipeline_extraction.feature` — extract pipelines from runs
  - `remote_sync_workflow.feature` — push/fetch/pull across repos
  - `hook_scripts.feature` — Cursor hook script integration
  - `hosted_inspection_workflow.feature` — `morph serve` API
  - `team_ci_workflow.feature` — certification and gating
- **tests/cucumber.rs** — step definitions (run morph, assert output and paths)

## Adding tests

1. Add or edit a `.feature` file under `features/` using Given/When/Then.
2. If you use new phrases, add matching step definitions in `tests/cucumber.rs` (e.g. `#[given(...)]`, `#[when(...)]`, `#[then(...)]`).
3. Run `cargo test -p morph-e2e --test cucumber`.

## Concurrent agents (Phase 2)

Gherkin is sequential; we simulate simultaneous agents by running multiple morph processes **inside one step** (e.g. "When 2 agents run record-session concurrently"). See `features/concurrent_agents.feature` and the matching step in `tests/cucumber.rs`.
