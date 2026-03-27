# Phase 2 Agent Prompt: Evidence-Backed Behavioral Commits

## Objective

Upgrade Morph from reliable local workflows to evidence-backed behavioral commits.

Phase 1 established confidence in local Git-like workflows (`branch`, `checkout`, `merge`, `rollup`, `upgrade`) through executable tests. Phase 2 must upgrade the commit path itself so a commit can carry provenance that links the file snapshot to the evidence that justifies it.

When you are done, a user must be able to:

1. Record a session or run into Morph.
2. Create a commit from that recorded evidence through a real user-facing path.
3. Inspect the resulting commit and see the persisted `evidence_refs`, `env_constraints`, and `contributors`.

This is not a metadata threading exercise. Implement one coherent provenance flow end to end.

---

## Product Contract

Use `docs/v0-spec.md` as the source of truth for commit provenance fields.

The contract to honor is:

- `evidence_refs`: hashes of supporting Run and Trace objects
- `env_constraints`: the environment in which the supporting evidence was captured
- `contributors`: the set of agents and humans that contributed to the commit

Relevant v0-spec sections:

- Commit object: `docs/v0-spec.md`
- Run object: `docs/v0-spec.md`
- CLI commit workflow: `docs/v0-spec.md`
- IDE/MCP write path: `docs/v0-spec.md`

Do not invent incompatible field names or shapes. If you need a helper struct in Rust, it must serialize back into the existing commit schema.

---

## Target Files

Read these before coding:

- `docs/v0-spec.md`
- `docs/TESTING.md`
- `morph-core/src/commit.rs`
- `morph-core/src/objects.rs`
- `morph-core/src/record.rs`
- `morph-cli/src/main.rs`
- `morph-mcp/src/main.rs`
- Existing YAML specs in `morph-cli/tests/specs/`
- Existing Gherkin features in `morph-e2e/features/`
- `morph-e2e/tests/cucumber.rs`

Phase 1 already added workflow coverage for branching, checkout, merge, rollup, and upgrade. Build on those tests; do not rewrite them unless you are fixing a real bug they exposed.

---

## What Exists Today

### Object model support already exists

`morph-core/src/objects.rs` already defines commit fields for:

- `contributors`
- `env_constraints`
- `evidence_refs`

`Run` already records:

- `environment`
- `trace`
- `agent`
- `contributors`

### The commit path drops provenance

Today, the commit constructors in `morph-core/src/commit.rs` still write:

- `contributors: None`
- `env_constraints: None`
- `evidence_refs: None`

This is true in the normal commit path and the merge path.

### Evidence ingestion already exists

`morph-core/src/record.rs` already records:

- session-backed Runs and Traces via `record_session`
- arbitrary Run objects via `record_run`

So the repository already has the evidence objects Phase 2 needs. The missing piece is connecting commit creation to that evidence.

### User-facing commit entrypoints do not expose provenance

Today:

- `morph commit` accepts `message`, `pipeline`, `eval_suite`, `metrics`, `author`
- `morph_commit` in the MCP server accepts the same shape

There is no coherent provenance path through either entrypoint.

### Inspection is too weak

There is no good CLI read path for inspecting a commit object's stored provenance after creation. Tests should not have to rely on reading `.morph/objects/<hash>.json` directly.

---

## The Provenance Flow To Implement

Implement this exact user-facing flow:

### Commit creation is run-backed

Add an optional `from_run` input to commit creation:

- CLI: `morph commit ... --from-run <run_hash>`
- MCP: `morph_commit` gets an optional `from_run` field

This is the single provenance flow for Phase 2.

When `from_run` is provided, load the stored Run and derive commit provenance as follows:

### 1. `evidence_refs`

Persist:

- the Run hash itself
- the Run's `trace` hash

Store them in deterministic order:

1. run hash
2. trace hash

If you discover duplicates, dedupe them while preserving that order.

### 2. `env_constraints`

Persist the Run environment in the commit using this shape:

```json
{
  "model": "...",
  "version": "...",
  "parameters": { ... },
  "toolchain": { ... }
}
```

This should be a direct mapping from `Run.environment`, not a lossy summary.

### 3. `contributors`

Derive commit contributors from the Run:

- include the primary `run.agent`
- include any `run.contributors`
- dedupe by contributor id
- preserve contributor roles when present
- if the primary run agent has no role, give it a stable role such as `"primary"`

Do not add version or policy fields to commit contributors; the commit schema does not support them.

### 4. Plain commits still work

`morph commit -m "message"` without `--from-run` must continue to work as a plain VCS commit. In that case, provenance fields may remain absent.

### 5. Missing evidence must fail clearly

If `--from-run` or MCP `from_run` points to:

- a missing hash
- a non-Run object
- a Run whose trace cannot be resolved

commit creation must fail with a clear error.

---

## Required Read Path

Add a real CLI inspection path for stored objects so provenance can be checked after commit creation.

Preferred path:

```bash
morph show <hash>
```

Behavior:

- accepts any Morph object hash
- prints pretty JSON for the stored object
- works for commit hashes so tests can inspect commit provenance through the real CLI

If you choose a different read path, it must be equally user-facing, equally testable, and not require tests to read raw object files from `.morph/objects/`.

---

## Implementation Requirements

Complete all of the following.

### 1. Core provenance abstraction

Introduce a small abstraction in `morph-core` for commit provenance, such as `CommitProvenance` or an equivalent helper. The goal is to model provenance once and pass it through commit creation as a single concept.

Do not thread three unrelated optional parameters through every function call if you can avoid it.

### 2. Commit creation wiring

Update the normal commit path so `create_tree_commit` can persist derived provenance.

You may also update merge or rollup paths if needed for consistency, but the required user-facing path is normal commit creation from a recorded Run.

### 3. Provenance resolution helper

Add helper logic that resolves a stored Run hash into commit provenance:

- validate the object type
- resolve and validate the trace
- construct deterministic `evidence_refs`
- map `Run.environment` into `env_constraints`
- map run agent/contributors into commit contributors

Place this helper where it is easiest to test and reuse cleanly.

### 4. CLI exposure

Extend `morph-cli/src/main.rs`:

- add `--from-run <hash>` to `morph commit`
- wire it into the core provenance helper
- add the `morph show <hash>` read path

### 5. MCP exposure

Extend `morph-mcp/src/main.rs`:

- add optional `from_run` to `morph_commit`
- wire it to the same core flow as the CLI

Do not implement a separate MCP-only provenance path.

### 6. Documentation

If command syntax changes, update user-facing documentation. At minimum, update whichever docs now describe commit creation and inspection behavior most directly.

Likely candidates:

- `docs/v0-spec.md`
- `docs/TESTING.md`
- `README.md` if needed

---

## Tests You Must Add

Every code change in this repo must include tests. Follow `docs/TESTING.md`.

### A. Unit tests in `morph-core`

Add or extend tests in the touched modules.

Minimum unit coverage:

- creating a tree commit without `from_run` leaves provenance absent
- creating a tree commit from a recorded session-backed Run persists the expected run hash and trace hash
- `env_constraints` exactly mirror the stored Run environment
- contributors are derived deterministically and deduped
- commit creation fails when `from_run` points to a missing object
- commit creation fails when `from_run` points to a non-Run object
- commit creation fails when the Run points to a missing trace

Recommended files:

- `morph-core/src/commit.rs`
- `morph-core/src/record.rs` if helper coverage fits better there

### B. YAML CLI integration specs

Create a new YAML spec file:

- `morph-cli/tests/specs/provenance.yaml`

Follow the existing YAML format used in the repo. Use the real `morph` binary.

Add at least these cases:

#### `commit_from_recorded_session_persists_evidence`

- init repo
- run `morph run record-session ...`
- stage a project file
- create pipeline and eval suite objects as needed
- run `morph commit ... --from-run <run_hash>`
- run `morph show <commit_hash>`
- assert stdout contains the run hash and trace hash

#### `commit_from_recorded_run_persists_env_and_contributors`

- construct a real Run JSON with:
  - non-empty environment fields
  - a trace hash
  - multiple contributors
- record the trace and run
- create a commit with `--from-run`
- inspect with `morph show <commit_hash>`
- assert stdout contains:
  - the expected environment keys/values
  - contributor ids
  - contributor role(s)

#### `commit_from_missing_run_fails`

- try `morph commit ... --from-run <missing_hash>`
- expect exit code 1
- assert stderr contains a clear error

#### `plain_commit_without_provenance_still_works`

- create a normal commit without `--from-run`
- assert success
- inspect with `morph show <commit_hash>`
- assert provenance fields are absent or null, whichever representation you choose consistently

### C. End-to-end Gherkin scenarios

Add a new feature file:

- `morph-e2e/features/evidence_backed_commit.feature`

Required scenarios:

#### Scenario: Session to evidence-backed commit

- Given a morph repo
- record a session through the CLI
- add a real file
- create a commit with `--from-run`
- inspect the commit with the CLI read path
- assert the run hash, trace hash, and environment are visible

#### Scenario: Multi-contributor run to commit

- Given a morph repo
- write trace/run JSON to disk
- record the run
- create a commit with `--from-run`
- inspect the commit
- assert contributors are visible after commit creation

Only add new step definitions to `morph-e2e/tests/cucumber.rs` if the existing generic command runner is insufficient.

---

## Execution Sequence

Follow this order.

### Step 1: Read current behavior

Read:

- `docs/v0-spec.md`
- `docs/TESTING.md`
- current commit, record, CLI, and MCP entrypoints
- current YAML specs
- current E2E features and step definitions

### Step 2: Implement the core provenance model

Add the provenance helper/abstraction and wire it into commit creation.

### Step 3: Expose the real user-facing path

Add:

- `--from-run` on CLI commit
- `from_run` on MCP commit
- `morph show <hash>` or an equivalent inspection path

### Step 4: Write CLI specs first

Create `morph-cli/tests/specs/provenance.yaml` and run:

```bash
cargo test -p morph-cli
```

Fix failures before moving on.

### Step 5: Write E2E scenarios

Create `morph-e2e/features/evidence_backed_commit.feature` and run:

```bash
cargo test -p morph-e2e --test cucumber
```

Fix failures before moving on.

### Step 6: Write unit tests

Add unit coverage in the touched `morph-core` modules and run:

```bash
cargo test -p morph-core
```

### Step 7: Run the full workspace

Run:

```bash
cargo test --workspace
```

All tests must pass.

### Step 8: Report exact results

In your final report, include the exact commands you ran and the pass/fail counts for each one.

---

## Acceptance Criteria

All of the following must be true:

1. Commits can persist `evidence_refs` when a session or run exists.
2. Commits can persist `env_constraints` in a defined, testable shape derived from the Run environment.
3. Commits can persist contributor metadata in a defined, testable way derived from the Run.
4. The provenance flow is exposed through a real user-facing path: `morph commit --from-run ...`.
5. The same flow is available through the MCP `morph_commit` tool.
6. Provenance survives commit creation and can be inspected afterward through a real CLI read path.
7. New unit tests exist in the touched `morph-core` modules.
8. A new CLI YAML spec file exists for provenance-backed commits.
9. A new E2E feature exists for a realistic session-to-commit flow.
10. The relevant `cargo test` commands are run and reported with pass/fail counts.

---

## Anti-Patterns To Avoid

- Do not add a provenance helper that is never exposed through the real CLI or MCP path.
- Do not thread `evidence_refs`, `env_constraints`, and `contributors` separately through every layer if a single provenance abstraction is cleaner.
- Do not invent a second provenance flow for MCP that differs from CLI.
- Do not require tests to read `.morph/objects/*.json` directly to verify provenance.
- Do not change the v0-spec field contract.
- Do not break plain commits that omit provenance.
- Do not skip tests.
- Do not add new dependencies unless there is no reasonable alternative.

---

## Test Commands

```bash
cargo test -p morph-core
cargo test -p morph-cli
cargo test -p morph-e2e --test cucumber
cargo test --workspace
```

---

## Final Report Template

When done, report:

```text
## Test Results

### morph-core
- Command: cargo test -p morph-core
- Result: X passed, Y failed

### morph-cli
- Command: cargo test -p morph-cli
- Result: X passed, Y failed

### morph-e2e
- Command: cargo test -p morph-e2e --test cucumber
- Result: X passed, Y failed

### full workspace
- Command: cargo test --workspace
- Result: X passed, Y failed

### Provenance flow implemented
- [short summary of the chosen flow]

### New files created
- morph-cli/tests/specs/provenance.yaml
- morph-e2e/features/evidence_backed_commit.feature
- [any docs files added or updated]

### Code changes
- [implementation files modified with one-line summary]

### Bugs found and fixed
- [each bug and the fix]
```
