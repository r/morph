# End-to-end testing plan

## Vision

An E2E harness that can grow from a single user in Cursor to full multi-instance sync:

| Phase | Scope | What we test |
|-------|--------|----------------|
| **1** | Single user, single agent (Cursor-like) | One workspace, one agent; CLI and MCP flows; disk and CLI output. |
| **2** | Multiple agents, concurrent | Same repo, multiple agents recording runs/sessions; ordering, no corruption. *(Supported: one step runs N morph processes in parallel; see `features/concurrent_agents.feature`.)* |
| **3** | Multiple instances, multiple agents | Several repos (or worktrees), each with agents; cross-repo consistency if applicable. |
| **4** | Client–server sync protocol | Server process, clients push/pull; sync correctness and conflict handling. |

We start with **Phase 1**: tests that mimic a single user working in Cursor with a single agent.

---

## Phase 1: Single user, single agent

### Goals

- **End-to-end**: drive the real `morph` CLI in a temp directory; no mocks.
- **Human-readable**: describe tests in **Gherkin** (Given/When/Then) so anyone can read and understand what is being tested.
- **Observable**: assert on CLI exit code, stdout/stderr, filesystem state.

### Test format: Cucumber (Gherkin)

All E2E tests are written in **Gherkin** (`.feature` files) and run by the **Cucumber** framework. No custom YAML or runner.

- **Specs**: `morph-e2e/features/*.feature` — plain-language scenarios (Given a morph repo, When I run "morph status", Then stdout contains "new", etc.).
- **Step definitions**: `morph-e2e/tests/cucumber.rs` — Rust that runs the morph binary (via `assert_cmd`), manages a temp dir, and asserts. Placeholders like `<prog_hash>` are filled from earlier "I capture the last output as \"prog_hash\"".
- **Run**: `cargo test -p morph-e2e --test cucumber`.

The harness is Cucumber; we only implement the steps that run morph and check results.

### What we assert (Phase 1)

- **CLI**: exit code, stdout/stderr content (substrings or regex).
- **Disk**: presence and content of files under the workspace (including `.morph/objects/`, `.morph/refs/`, `.morph/prompts/`, `.morph/runs/`, etc.).
- **Timing**: optional per-step `max_duration_secs` to catch hangs or severe regressions.

### Example single-user flows to cover

1. **Init and status**  
   Init repo, create a file, `morph status` → file listed as new.

2. **Stage and commit**  
   Init, create file, `add`, `commit` with message and optional program/eval_suite → `log` shows commit; `.morph/refs/heads/main` and objects exist.

3. **Prompt create and materialize**  
   Init, create `.morph/prompts/foo.txt`, `prompt create` → hash on stdout; `prompt materialize` → file content matches.

4. **Session recording (simulate Cursor agent)**  
   Init, `run record-session --prompt "user request" --response "agent reply"` → run hash on stdout; `.morph/objects/` and `.morph/runs/` (and traces) contain expected objects.

5. **Run record from JSON**  
   Init, write run.json + trace.json, `run record run.json --trace trace.json` → hash; disk has run and trace objects.

6. **Eval record**  
   Init, write metrics JSON, `eval record` → stdout contains metrics; evals or objects updated as designed.

7. **Branch and checkout**  
   Init, commit, `branch feat`, `checkout feat`, create file, commit → `log` and refs reflect branch.

8. **Annotate and list**  
   Init, create prompt, add, prompt create, `annotate` with kind/data, `annotations` → list shows annotation.

These become the first set of E2E spec files; the harness runs them and reports success/failure and timing.

---

## Later phases (outline)

- **Phase 2**: Runner can spawn multiple processes (or threads) that each run morph in the same repo; steps can be interleaved (e.g. two “agents” each calling `run record-session`); assertions check object set and consistency.
- **Phase 3**: Multiple temp dirs (or worktrees), each with morph; steps specify which “instance” and which “agent”; assertions can span instances.
- **Phase 4**: Start a morph server (or use existing `morph-serve`), run clients that push/pull; assertions on server and client state and sync protocol behavior.

The same **spec format** can be extended with `instance`, `agent_id`, and `sync` sections so one harness drives all phases.

---

## Implementation layout

- **Plan**: this document.
- **Harness**: Cucumber (crate `cucumber`); step definitions in `morph-e2e/tests/cucumber.rs` use `assert_cmd` and `tempfile`.
- **Specs**: `morph-e2e/features/*.feature` (Gherkin).
- **Invocation**: `cargo build -p morph-cli && cargo test -p morph-e2e --test cucumber`.

The morph binary is built first; Cucumber runs the integration test, which invokes `Command::cargo_bin("morph")` so we always exercise the real CLI.
