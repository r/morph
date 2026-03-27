# Phase 3 Agent Prompt: Trace-Backed Pipeline Extraction

## Objective

Upgrade Morph from evidence-backed commits to reusable agent workflows.

Phase 2 established a real provenance path from recorded evidence into commits: a user can record a Run or session, create a commit with `--from-run`, and inspect the stored commit provenance afterward.

Phase 3 must build directly on that path. When you are done, a user must be able to:

1. Record a session or Run into Morph.
2. Extract a first-class Pipeline object from that recorded evidence through a real CLI path.
3. Inspect the extracted Pipeline and see meaningful structure, provenance, and attribution.
4. Reuse the extracted Pipeline anywhere a normal Pipeline hash is accepted.

This is not a metadata-only exercise. Implement one coherent run-to-pipeline extraction workflow end to end.

---

## Product Contract

Use `docs/v0-spec.md` as the source of truth for Pipeline extraction behavior.

The Phase 3 contract to honor is:

- `Pipeline.provenance`
- `Pipeline.attribution`
- the existing pipeline node kinds, especially `review`
- trace-backed workflow extraction from recorded Runs

Relevant v0-spec sections:

- Pipeline object: `docs/v0-spec.md`
- Run object: `docs/v0-spec.md`
- Trace object: `docs/v0-spec.md`
- CLI pipeline management: `docs/v0-spec.md`
- Object inspection: `docs/v0-spec.md`

Do not invent incompatible field names, node kinds, or schema shapes. If you add helper structs in Rust, they must serialize back into the existing Pipeline schema.

---

## Target Files

Read these before coding:

- `docs/v0-spec.md`
- `docs/TESTING.md`
- `morph-core/src/objects.rs`
- `morph-core/src/record.rs`
- `morph-core/src/commit.rs`
- `morph-core/src/working.rs`
- `morph-cli/src/main.rs`
- existing YAML specs in `morph-cli/tests/specs/`
- existing Gherkin features in `morph-e2e/features/`
- `morph-e2e/tests/cucumber.rs`

Phase 2 already introduced:

- `morph commit --from-run <run_hash>`
- `morph show <hash>`
- unit tests for commit provenance from Run
- CLI specs for provenance-backed commits
- E2E coverage for session-to-commit provenance

Build on that implementation and test surface. Do not create a disconnected Phase 3 feature.

---

## What Exists Today

### Pipeline schema support already exists

`morph-core/src/objects.rs` already defines:

- `Pipeline.provenance`
- `Pipeline.attribution`
- node kinds including `prompt_call`, `tool_call`, `retrieval`, `transform`, `identity`, and `review`

Hash/serialization tests already cover:

- review node round-tripping
- attribution actor sets
- provenance field serialization

### Evidence ingestion already exists

`morph-core/src/record.rs` already records:

- session-backed Runs and Traces via `record_session`
- arbitrary Run objects via `record_run`

For `record_session`, the trace shape is canonical and deterministic:

- event 0: prompt
- event 1: response

### Provenance resolution from Run already exists

Phase 2 added a commit provenance abstraction in `morph-core/src/commit.rs` that already knows how to:

- resolve a stored Run
- validate its Trace
- derive deterministic evidence refs
- map environment into `env_constraints`
- derive contributors

Phase 3 should reuse that knowledge where it fits instead of re-solving the same problem in a separate way.

### The missing piece is extraction

Today there is no real user-facing path that turns recorded evidence into a Pipeline object:

- `morph pipeline create <file>` exists
- `morph pipeline show <hash>` exists
- there is no `morph pipeline extract ...`
- there is no deterministic graph synthesis from a recorded Run/Trace
- there are no CLI or E2E tests proving a session can become a reusable Pipeline

---

## The Extraction Flow To Implement

Implement this exact user-facing flow:

```bash
morph pipeline extract --from-run <run_hash>
```

Behavior:

- accepts a stored Run hash
- resolves the Run and its Trace from the object store
- extracts a new Pipeline object from that evidence
- stores the Pipeline in Morph
- prints the new Pipeline hash

This is the single extraction flow for Phase 3.

### Scope: one coherent supported source shape

Phase 3 must fully support extraction from session-backed Runs created by:

```bash
morph run record-session --prompt ... --response ...
```

That is the minimum supported real user path.

You may support additional recorded Run shapes if they map cleanly onto the existing Pipeline schema, but do not broaden the scope into a speculative general trace compiler. If a source Run cannot be extracted without guesswork, fail clearly with an unsupported-shape error.

### Session-backed extraction contract

For a canonical `record_session` Run, extraction must produce a deterministic minimal Pipeline graph with this exact shape:

1. A `prompt_call` node named `generate`
2. A `review` node named `review`
3. One `data` edge from `generate` to `review`

The extracted Pipeline must be a real first-class Pipeline object, not a special case outside the object model.

### `generate` node requirements

The `generate` node must:

- have `kind: "prompt_call"`
- reference a prompt blob derived from the source Trace prompt text
- use deterministic empty/default params unless the source evidence supplies more
- persist per-node `env` derived from the Run environment

The extracted Pipeline's `prompts` array must contain the prompt blob hash referenced by the `generate` node.

### `review` node requirements

The `review` node must:

- have `kind: "review"`
- use `ref: null`
- represent the acceptance of the generated session workflow into a reusable Pipeline
- be inspectable as an explicit review step in the graph rather than an implicit convention

Use the existing schema only. If you need lightweight metadata on the review node, put it in `params`; do not invent new top-level Pipeline fields.

### Attribution contract

Use `Pipeline.attribution` to record who contributed to the extracted nodes.

For the session-backed minimum path:

- `generate` must be attributed to the primary Run agent
- `review` must be attributed deterministically

If the source Run has contributors with role `"review"`, attribute the `review` node to those contributors.

If the session-backed Run has no explicit reviewer, attribute the `review` node to the primary Run agent so the extracted graph still has a stable, inspectable review step.

Use `actors` arrays with `ActorRef` values where possible. Preserve backward-compatible fields if the existing code requires them, but the extracted Pipeline should visibly honor the set-valued attribution model from the spec.

### Provenance contract

Persist `Pipeline.provenance` using the source evidence:

- `derived_from_run`: the source Run hash
- `derived_from_trace`: the source Trace hash
- `derived_from_event`: the response event id for session-backed runs
- `method`: `"extracted"`

This is the Phase 3 equivalent of Phase 2's commit provenance path. Do not leave extraction provenance absent.

### Reuse contract

The extracted Pipeline must be reusable through existing user-facing flows with no special case handling.

At minimum, it must work as input to the existing commit path:

```bash
morph commit -m "..." --pipeline <extracted_pipeline_hash>
```

You do not need to make Morph execute the Pipeline. Reusability in v0 means it is stored, inspectable, and accepted anywhere a normal Pipeline object is accepted.

### Failure behavior

Extraction must fail clearly when `--from-run` points to:

- a missing hash
- a non-Run object
- a Run whose Trace cannot be resolved
- a Run/Trace shape that Phase 3 does not support extracting

---

## Required Read Path

The extracted Pipeline must be inspectable after creation through a real CLI path.

Preferred path:

```bash
morph pipeline show <hash>
```

That read path must make it easy for tests and users to inspect:

- graph nodes and edges
- provenance fields
- attribution fields
- prompt references

`morph show <hash>` may also work, but tests should not need to read raw `.morph/objects/*.json` files directly.

If `morph pipeline show` is currently too weak or too terse to inspect extraction output clearly, improve it.

---

## Implementation Requirements

Complete all of the following.

### 1. Core extraction abstraction

Introduce a small extraction abstraction in `morph-core`, such as:

- `extract_pipeline_from_run`
- `PipelineExtraction`
- or an equivalent helper

The goal is to model extraction as one coherent core flow, not as a collection of disconnected per-field helpers.

### 2. Reuse Phase 2 provenance knowledge

Build on the Phase 2 Run-resolution path where it helps:

- Run loading and validation
- Trace validation
- deterministic evidence ordering
- environment mapping
- contributor interpretation

Do not fork the provenance logic into unrelated commit and pipeline code paths unless there is a strong reason.

### 3. Trace-backed graph synthesis

Add helper logic that turns a supported Run/Trace shape into a Pipeline graph:

- create or reuse a prompt blob derived from the source prompt event
- create the deterministic `generate -> review` graph for session-backed Runs
- attach per-node env
- attach attribution
- attach provenance

Keep this logic testable and deterministic.

### 4. CLI exposure

Extend `morph-cli/src/main.rs`:

- add `pipeline extract --from-run <hash>`
- wire it into the new core extraction flow
- keep the output consistent with existing commands: print the extracted Pipeline hash on success

### 5. Read-path quality

Ensure the extracted Pipeline can be inspected cleanly through:

- `morph pipeline show <hash>`
- or an improved equivalent under the existing CLI surface

### 6. Documentation

If CLI behavior changes, update user-facing docs.

Likely candidates:

- `docs/v0-spec.md`
- `docs/TESTING.md`
- `README.md` if command discovery would otherwise be unclear

If you add or change MCP behavior, document that too. But do not create a separate extraction semantics for MCP.

---

## Tests You Must Add

Every code change in this repo must include tests. Follow `docs/TESTING.md`.

### A. Unit tests in `morph-core`

Add or extend tests in the touched modules.

Minimum unit coverage:

- extracting from a canonical session-backed Run produces the exact deterministic graph shape:
  - node ids `generate` and `review`
  - node kinds `prompt_call` and `review`
  - one edge `generate -> review`
- the extracted Pipeline provenance persists:
  - `derived_from_run`
  - `derived_from_trace`
  - `derived_from_event`
  - `method = "extracted"`
- the `generate` node env mirrors the source Run environment
- the extracted Pipeline includes a prompt blob ref and a non-empty `prompts` array
- attribution includes the primary Run agent on `generate`
- explicit review contributors, when present, are mapped onto the `review` node attribution
- extraction fails when `from_run` points to a missing object
- extraction fails when `from_run` points to a non-Run object
- extraction fails when the Run points to a missing Trace
- extraction fails clearly on unsupported trace shape

Recommended files:

- `morph-core/src/record.rs`
- `morph-core/src/objects.rs`
- `morph-core/src/commit.rs` if reuse lives there
- whichever module owns the extraction helper

### B. YAML CLI integration specs

Create a new YAML spec file:

- `morph-cli/tests/specs/pipeline_extract.yaml`

Follow the existing YAML format used in the repo. Use the real `morph` binary.

Add at least these cases:

#### `extract_pipeline_from_recorded_session`

- init repo
- run `morph run record-session ...`
- run `morph pipeline extract --from-run <run_hash>`
- run `morph pipeline show <pipeline_hash>`
- assert stdout contains:
  - `prompt_call`
  - `review`
  - `derived_from_run`
  - `derived_from_trace`
  - `derived_from_event`
  - `extracted`
  - the source Run hash

#### `extract_pipeline_from_recorded_run_with_reviewer`

- construct a real Trace JSON and Run JSON
- include a contributor with role `review`
- record the Run
- extract the Pipeline
- inspect it through the CLI
- assert stdout contains:
  - `review`
  - reviewer id
  - primary agent id
  - provenance fields

#### `extract_pipeline_from_missing_run_fails`

- run `morph pipeline extract --from-run <missing_hash>`
- expect exit code 1
- assert stderr contains a clear error

#### `extracted_pipeline_is_reusable_in_commit`

- record a session
- extract a Pipeline
- stage a real file
- create a commit with `--pipeline <extracted_pipeline_hash>`
- inspect the resulting commit
- assert the commit references the extracted Pipeline hash

### C. End-to-end Gherkin scenarios

Add a new feature file:

- `morph-e2e/features/run_to_pipeline_extraction.feature`

Required scenarios:

#### Scenario: Session to inspectable reusable pipeline

- Given a morph repo
- record a session through the CLI
- extract a Pipeline from the recorded Run
- inspect the Pipeline through the CLI
- assert the extracted graph shape and provenance are visible
- stage a real file
- commit using the extracted Pipeline hash
- assert the commit succeeds

#### Scenario: Extraction fails clearly for missing Run

- Given a morph repo
- run extraction with a missing Run hash
- assert non-zero exit
- assert stderr contains a clear error

Only add new step definitions to `morph-e2e/tests/cucumber.rs` if the existing generic command runner is insufficient.

---

## Execution Sequence

Follow this order.

### Step 1: Read current behavior

Read:

- `docs/v0-spec.md`
- `docs/TESTING.md`
- current object model, record path, commit provenance path, and CLI pipeline commands
- current YAML specs
- current E2E features and step definitions

### Step 2: Implement the core extraction model

Add the extraction helper/abstraction in `morph-core` and make the session-backed graph synthesis deterministic.

### Step 3: Expose the real user-facing path

Add:

- `morph pipeline extract --from-run <hash>`
- any necessary improvements to `morph pipeline show`

### Step 4: Write CLI specs first

Create `morph-cli/tests/specs/pipeline_extract.yaml` and run:

```bash
cargo test -p morph-cli
```

Fix failures before moving on.

### Step 5: Write E2E scenarios

Create `morph-e2e/features/run_to_pipeline_extraction.feature` and run:

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

1. A user can extract a Pipeline from a recorded Run through a real CLI path.
2. The minimum supported user flow works from a canonical `record_session` Run.
3. The extracted Pipeline persists `Pipeline.provenance` derived from the source Run and Trace.
4. The extracted Pipeline has a deterministic minimal graph shape for session-backed Runs:
   - `generate` prompt node
   - `review` review node
   - one `generate -> review` data edge
5. The extracted Pipeline records inspectable attribution using the existing Pipeline attribution schema.
6. The extracted Pipeline is inspectable afterward through `morph pipeline show` or an improved equivalent CLI read path.
7. The extracted Pipeline is reusable through the existing commit path without special cases.
8. New unit tests exist in the touched `morph-core` modules.
9. A new CLI YAML spec file exists for pipeline extraction.
10. A new E2E feature exists for a realistic session-to-pipeline workflow.
11. The relevant `cargo test` commands are run and reported with pass/fail counts.

---

## Anti-Patterns To Avoid

- Do not add extraction helpers that are never exposed through the real CLI path.
- Do not just return the existing `Run.pipeline` hash or identity pipeline hash and call that extraction.
- Do not invent new Pipeline node kinds or provenance fields.
- Do not make tests read `.morph/objects/*.json` directly to verify extraction.
- Do not build partial per-field helpers instead of one coherent extraction flow.
- Do not guess at arbitrary trace semantics. If a trace shape is unsupported, fail clearly.
- Do not skip review nodes if your extracted graph is meant to encode an explicit acceptance step.
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

### Extraction flow implemented
- [short summary of the chosen run-to-pipeline flow]

### New files created
- morph-cli/tests/specs/pipeline_extract.yaml
- morph-e2e/features/run_to_pipeline_extraction.feature
- [any docs files added or updated]

### Code changes
- [implementation files modified with one-line summary]

### Bugs found and fixed
- [each bug and the fix]
```
