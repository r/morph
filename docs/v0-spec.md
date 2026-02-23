# Morph v0 Specification
## Concrete System Design

This document defines the first implementation target for Morph.

It translates the ideas in:

- [README.md](README.md) — engineering overview: what Morph is and the Git analogy
- [THEORY.md](THEORY.md) — formal algebraic foundations

into a minimal, buildable system.

This is **not** the final architecture.
It is the smallest coherent system that satisfies the Morph axioms (THEORY.md §18).

---

# 1. Scope of v0

Morph v0 will provide:

- A local content-addressed object store
- A commit graph (Merkle DAG)
- First-class prompt objects
- Program definitions (pipeline graphs)
- Evaluation suites as versioned objects
- Run manifests (execution receipts)
- Annotation objects (extensible metadata on any object)
- Basic evaluation gating
- Branching and merging
- CLI tooling
- IDE adapter interface (minimal contract)

Morph v0 will NOT include:

- Cryptographic signing
- Distributed trace transparency logs
- Advanced policy pack enforcement
- Distributed run deduplication
- Advanced statistical merge proofs
- Remote protocol (content-addressed store is designed for future distribution)

Those can come later.

### Morph is pure VCS (v0)

Morph does **not** execute programs or run evaluations. All LLM calls, tool execution, and test runs happen in external tools (IDEs, agents, CI). Morph's role is to:

- **Store** immutable content-addressed objects
- **Record** execution evidence that external tools report (runs, traces, metrics)
- **Version** programs with behavioral contracts (commits)
- **Gate** merges on metric dominance

Commands like `morph run record` and `morph eval record` **ingest** results; they do not run anything. The primary write path from the IDE is the **Cursor MCP server**, which reports session data into Morph. Reading and inspection happen via the CLI.

---

# 2. Relationship to Theory

This section maps THEORY.md concepts to v0 constructs.

| Theory Concept | THEORY.md | v0 Construct |
|---|---|---|
| State S = (D, C, M) | §5.1 | Tree (D), execution context in Run (C), Commit/Run metadata (M) |
| Environment E | §5.2 | Run `environment` field (model, version, parameters, toolchain) |
| Program P : S → F(S) | §6.1 | Program object (operator graph over Prompt blobs) |
| Effect functor F | §6.1 | Run execution: running a Program produces probabilistic outputs |
| Identity program | §7.2 | Built-in no-op Program (identity hash, passes state through unchanged) |
| Sequential composition Q ∘ P | §7.1 | Graph edges (data flow between operator nodes) |
| Parallel composition P ⊗ Q | §8.3 | Independent subgraphs within a Program graph |
| Evaluation suite T | §10 | EvalSuite object |
| Contract satisfaction | §11.1 | Eval pass: aggregated metrics meet declared thresholds |
| Behavioral preorder P ⪯ Q | §11.3 | Metric dominance: Q meets or exceeds P's certified scores |
| Certificate vector v ∈ V_T | §11.2 | `observed_metrics` in Commit's eval contract |
| Commit = behavioral certificate | §12 | Commit object (program hash + eval contract + observed metrics) |
| Certificate lattice join | §13.2 | Componentwise max of parent metrics during merge |
| Merge as behavioral join | §13.3 | Merge commit must dominate both parents' observed metrics |
| Working space vs commit space | §14 | Working directories vs `.morph/objects/` |
| Run = execution receipt | §9.1 | Run object (environment, metrics, trace, artifacts) |
| Trace as DAG of events | §9.2 | Trace events with IDs, types, and sequential ordering (v0 simplifies to linear) |
| Annotations | §17.1 | Annotation object (feedback, bookmarks, tags on any object) |
| Program provenance | §17.2 | Program `provenance` field (derived_from_run, method, etc.) |

---

# 3. Repository Structure

A Morph repository contains:

```
.morph/
  objects/        # canonical content-addressed store: one <hash>.json per object
  refs/
    HEAD          # symbolic ref (e.g. "ref: heads/main\n")
    heads/        # branch refs: heads/<branch> contains commit hash
  runs/           # type index: copy of each Run as <hash>.json (also in objects/)
  traces/         # type index: copy of each Trace as <hash>.json (also in objects/)
  prompts/        # optional: user prompt files; type index: <hash>.json for prompt Blobs
  evals/          # optional: user eval JSON; type index: <hash>.json for EvalSuites
  config.json     # repo config (empty object by default)
  index.json      # staging index: maps working-dir paths to blob hashes (cleared after commit)
```

`morph init` creates only `.morph/` (like `git init`): `objects/`, `refs/heads/`, `runs/`, `traces/`, `prompts/`, `evals/`, `config.json`, and `refs/HEAD` (pointing at `heads/main`). The working directory — the user's project directory — is the working space. There are no top-level `prompts/`, `programs/`, or `evals/` directories.

### `.morph/objects/`
The single source of truth. Every object is stored here as `<sha256>.json` (content-addressed). Object types:

- Blobs
- Trees
- Programs
- EvalSuites
- Commits
- Runs
- Artifacts
- Traces
- TraceRollups
- Annotations

### `.morph/refs/`
- **HEAD** — symbolic ref to the current branch (e.g. `ref: heads/main\n`).
- **heads/<branch>** — each file contains the commit hash for that branch.

### `.morph/runs/` and `.morph/traces/`
When a Run or Trace is stored via `put()`, it is written to `objects/<hash>.json`. A copy is also written to `runs/<hash>.json` or `traces/<hash>.json` so runs and traces can be listed by type without scanning all objects. Same content as in `objects/`.

### `.morph/prompts/` and `.morph/evals/`
User-facing: optional prompt files (e.g. `.prompt`) and eval JSON files. When a prompt Blob or EvalSuite is stored, a copy is also written to `prompts/<hash>.json` or `evals/<hash>.json` (type index). `morph status` and `morph add` include these directories; other `.morph/` internals (objects, refs, config) are excluded. Paths matching `.morphignore` (see §3 Working Space) are also excluded.

### Working Space

The **working directory** (the user's project directory) is the working space — analogous to Git's working tree. `morph add` works like `git add`: it stages any file from the working directory into the object store.

`.morph/prompts/` and `.morph/evals/` are optional metadata directories for prompt and eval suite definitions. The `programs/` directory is eliminated: program manifests are created via `morph program create <file>` and exist only in the object store. Morph can be used as a plain VCS without prompts or evals.

Changes in working space are not versioned until committed.

### `.morphignore`

A **`.morphignore`** file at the repository root (same directory as `.morph/`) specifies paths that Morph will not include in `morph status` or `morph add`. Same syntax and semantics as [`.gitignore`](https://git-scm.com/docs/gitignore): one pattern per line, `#` comments, blank lines ignored, and patterns are relative to the repo root. Examples:

- `target/` — ignore the `target` directory and everything under it
- `*.log` — ignore any file ending in `.log`
- `node_modules/` — ignore dependency trees you do not want to version as content

If `.morphignore` is missing, nothing is excluded beyond the built-in rules (e.g. `.morph/` internals). This keeps working-space scans and commits focused on the files that define your programs, prompts, and evals.

### Commit Space

The `.morph/objects/` store and the commit graph comprise the **commit space** — stabilized, evaluation-certified behavioral identities. Rollup collapses exploratory working-space history into stable commit-space identities.

### Storage Backend

Morph's core logic depends on an abstract storage interface, not on a specific backend. Any system that can implement the following operations qualifies:

- **put(object) → hash** — serialize, hash, and store an immutable object
- **get(hash) → object** — retrieve an object by its content hash
- **has(hash) → bool** — check existence
- **list(type) → [hash]** — enumerate objects of a given type
- **ref_read(name) → hash** — resolve a named reference (branch, tag)
- **ref_write(name, hash)** — update a named reference

The v0 default backend is **flat files on the local filesystem** — the `.morph/` directory layout described in §3. All objects (including Runs and Traces) are JSON files in `objects/<hash>.json`; refs are files (HEAD symbolic, heads/<branch> with hash); type-index copies for Runs, Traces, etc. live under `runs/`, `traces/`, `prompts/`, `evals/`.

Other backends are explicitly anticipated:

- **SQLite** — better query performance, transactional writes, FTS for search
- **S3 / object storage** — for remote/distributed backends
- **Database-backed** — for tools that want to index annotations, runs, or traces with richer query semantics

The filesystem layout is a projection of the storage backend, not the backend itself. CLI and IDE integrations talk to the backend interface. This means a tool like a session-capture daemon could use SQLite locally while the same Morph objects remain content-addressed and portable across backends.

---

# 4. Core Object Types

All objects are immutable and content-addressed by SHA-256.

Each object is serialized in canonical JSON before hashing.

---

## 4.1 Blob

Represents an atomic content unit:

- Prompt template
- Policy definition
- Tool schema
- Static configuration
- Any domain-specific content

```json
{
  "type": "blob",
  "kind": "<string>",
  "content": { }
}
```

The `kind` field is an open string. Well-known values include `prompt`, `policy`, `schema`, and `config`. Downstream tools may define additional kinds (e.g., `skill_template`, `resource`, `example`) without requiring changes to Morph itself.

---

## 4.2 Tree

Represents a structured grouping of objects (maps to the Document tree D in the state model).

```json
{
  "type": "tree",
  "entries": [
    { "name": "file_or_node", "hash": "<object_hash>", "entry_type": "blob | tree" }
  ]
}
```

`entry_type` distinguishes file entries (`blob`) from subdirectory entries (`tree`). Defaults to `blob` for backward compatibility.

---

## 4.3 Program

A Program encodes a prompt program — the core versioned unit in Morph. It corresponds to THEORY.md §6.1's transformation P : S → F(S), represented as a directed acyclic graph of operators.

```json
{
  "type": "program",
  "graph": {
    "nodes": [
      {
        "id": "node_id",
        "kind": "prompt_call | tool_call | retrieval | transform | identity",
        "ref": "<blob_hash or null>",
        "params": { }
      }
    ],
    "edges": [
      { "from": "node_id", "to": "node_id", "kind": "data | control" }
    ]
  },
  "prompts": ["<blob_hash>"],
  "eval_suite": "<eval_suite_hash>",
  "provenance": {
    "derived_from_run": "<run_hash or null>",
    "derived_from_trace": "<trace_hash or null>",
    "derived_from_event": "<event_id or null>",
    "method": "manual | extracted | composed"
  }
}
```

**Node kinds:**
- `prompt_call` — invokes an LLM with a referenced Prompt blob
- `tool_call` — invokes an external tool
- `retrieval` — fetches context from a document store
- `transform` — deterministic data transformation
- `identity` — no-op pass-through (corresponds to the theory's identity program I)

**Edge kinds:**
- `data` — output of source flows as input to target (sequential composition)
- `control` — execution ordering without data dependency

Nodes with no edges between them execute in parallel (parallel composition P ⊗ Q from the theory).

**Provenance** records how this Program was created. The `provenance` field is optional — null fields indicate unknown or not-applicable. When a Program is extracted from a successful Run (e.g., distilling an agent session into a reusable workflow), provenance records the source Run, Trace, and optionally the specific event within that Trace. The `method` field indicates whether the Program was written by hand (`manual`), extracted from a session (`extracted`), or built by composing existing Programs (`composed`).

---

## 4.4 EvalSuite

Evaluation suite — a first-class versioned object. This is the concrete realization of the theory's evaluation suite T (THEORY.md §10) that defines behavioral contracts.

### Evaluation interface

Morph's core logic depends on an abstract evaluation interface, not on a specific evaluation implementation. The interface has one operation:

- **evaluate(program, eval_suite, environment) → metrics**

Given a Program, an EvalSuite, and an Environment, produce a dictionary of metric name → score. Morph uses the returned scores for:

- Recording `observed_metrics` in Commits
- Checking dominance during merge
- Gating commit creation

How scores are produced is the evaluator's concern, not Morph's. An evaluator might:

- Run the program against test cases and compute automated scores
- Aggregate human feedback ratings collected via Annotations
- Call an external evaluation service
- Combine automated and human signals

The behavioral preorder is agnostic to metric source. What matters is that the evaluator returns comparable scores against the declared thresholds.

### EvalSuite object

```json
{
  "type": "eval_suite",
  "cases": [
    {
      "id": "case_id",
      "input": { },
      "expected": { },
      "metric": "metric_name"
    }
  ],
  "metrics": [
    {
      "name": "metric_name",
      "aggregation": "<string>",
      "threshold": 0.0
    }
  ]
}
```

The `aggregation` field is an open string. It tells the evaluator how to reduce per-case scores into a single metric value. Well-known values are listed below. Custom evaluators may define their own.

### v0 evaluator (built-in)

The v0 default evaluator is minimal:

- Runs the Program against each case in the EvalSuite
- Computes per-case scores by comparing outputs to expected values
- Aggregates using the declared aggregation function
- Returns the metric dictionary

**Built-in aggregation functions:**
- `mean` — arithmetic mean of per-case scores
- `min` — worst-case score
- `p95` — 95th percentile
- `lower_ci_bound` — lower bound of a 95% confidence interval (a minimal nod to distributional stability)

**Pass criteria:** for each metric, aggregated score ≥ declared threshold.

Statistical sophistication is minimal in v0. Future versions or custom evaluators may implement two-sample equivalence tests, Bayesian comparison, or human-in-the-loop evaluation pipelines (see THEORY.md §10.4 for the general certification framework).

---

## 4.5 Commit

```json
{
  "type": "commit",
  "tree": "<tree_hash>",
  "program": "<program_hash>",
  "parents": ["<commit_hash>"],
  "message": "string",
  "timestamp": "...",
  "author": "...",
  "eval_contract": {
    "suite": "<eval_suite_hash>",
    "observed_metrics": { "metric_name": "value" }
  },
  "morph_version": "0.3"
}
```

The `tree` field records the root hash of the file tree at commit time — the same role as Git's tree in a commit. `morph_version` records the store version that created this commit. Commits from before version 0.3 have `tree: null` and `morph_version: null`.

A Commit now stores both the behavioral contract AND the file tree snapshot. `program` and `eval_contract` default to the identity program and empty eval suite when not specified, making `morph commit -m 'message'` work as a plain VCS commit.

The `observed_metrics` field records the metrics achieved at commit time. This is critical for merge: the merge commit must dominate these observed values, not merely pass the suite's base thresholds.

Commits are claims. Runs are receipts.

---

## 4.6 Run

A Run is an execution receipt.

```json
{
  "type": "run",
  "program": "<program_hash>",
  "commit": "<commit_hash or null>",
  "environment": {
    "model": "...",
    "version": "...",
    "parameters": { },
    "toolchain": { }
  },
  "input_state_hash": "...",
  "output_artifacts": ["<artifact_hash>"],
  "metrics": { },
  "trace": "<trace_hash>",
  "agent": {
    "id": "...",
    "version": "...",
    "policy": "<policy_hash or null>"
  }
}
```

Runs do not modify commit history.

The `commit` field is null for exploratory runs in working space. It references a commit hash when the run serves as evidence for a committed program.

Environment recording is mandatory (THEORY.md §18, Axiom 9).

---

## 4.7 Artifact

```json
{
  "type": "artifact",
  "kind": "patch | file | bundle",
  "content": "...",
  "metadata": { }
}
```

---

## 4.8 Trace

A Trace is the full execution record of a Run. Each event within a Trace has a unique ID, enabling fine-grained annotation of individual steps.

```json
{
  "type": "trace",
  "events": [
    {
      "id": "<string, unique within trace>",
      "seq": 0,
      "ts": "...",
      "kind": "<string>",
      "payload": { }
    }
  ]
}
```

**Event fields:**
- `id` — unique identifier within this Trace (e.g., `evt_001`). Enables Annotations to target specific events.
- `seq` — monotonically increasing sequence number for ordering.
- `ts` — timestamp.
- `kind` — open string describing the event type. Well-known values include `prompt`, `response`, `tool_call`, `file_read`, `file_edit`, `error`. Downstream tools define their own event vocabularies.
- `payload` — event-specific data (prompt text, tool arguments, diff content, etc.).

Traces may be large. They are stored separately but referenced by Run.

---

## 4.9 TraceRollup

Summarized trace:

```json
{
  "type": "trace_rollup",
  "trace": "<trace_hash>",
  "summary": "...",
  "key_events": ["<event_id>"]
}
```

TraceRollup never replaces Trace. The `key_events` field references event IDs from the source Trace, enabling higher-level tools to highlight notable moments.

---

## 4.10 Annotation

An Annotation attaches metadata to any content-addressed object — or to a specific event within a Trace — without altering that object's identity (hash). Annotations are themselves immutable, content-addressed objects.

```json
{
  "type": "annotation",
  "target": "<object_hash>",
  "target_sub": "<event_id or null>",
  "kind": "<string>",
  "data": { },
  "author": "...",
  "timestamp": "..."
}
```

**Fields:**
- `target` — hash of the object being annotated (Run, Trace, Commit, Program, Blob, etc.).
- `target_sub` — optional sub-addressing within the target. For Traces, this is an event ID. For Trees, this could be an entry name. Null when annotating the object as a whole.
- `kind` — open string. Well-known values include:
  - `feedback` — human rating (good / partial / bad)
  - `bookmark` — marks a notable point (a "Moment")
  - `tag` — categorical label
  - `note` — freeform text
  - `link` — cross-reference to another object
- `data` — kind-specific payload. Examples:
  - Feedback: `{ "rating": "good", "note": "this prompt pattern works well" }`
  - Bookmark: `{ "title": "The fix that worked", "note": "..." }`
  - Tag: `{ "tags": ["auth", "jwt", "migration"] }`
  - Note: `{ "text": "..." }`
  - Link: `{ "rel": "derived_from", "target": "<object_hash>" }`
- `author` — who created the annotation (user ID, agent ID).
- `timestamp` — when the annotation was created.

**Design rationale:** Annotations are the extensibility primitive of Morph. Because they reference targets by hash and are content-addressed themselves, they compose with the immutable object model: adding metadata never rewrites history. Higher-level tools (session capture, curation, workflow extraction) can layer rich metadata onto the Morph object graph without requiring changes to Morph's core object types.

---

# 5. The Identity Program

THEORY.md §7.2 requires an identity program I such that I ∘ P = P ∘ I = P.

In v0, this is a well-known Program object:

```json
{
  "type": "program",
  "graph": {
    "nodes": [{ "id": "passthrough", "kind": "identity", "ref": null, "params": {} }],
    "edges": []
  },
  "prompts": [],
  "eval_suite": null,
  "provenance": null
}
```

Its hash is deterministic and serves as the identity element for program composition.

---

# 6. CLI Commands (v0)

Morph CLI mirrors Git where possible.

## 6.1 Repository Management

```
morph init
morph status
morph log
```

## 6.2 Prompt Object Creation

```
morph prompt create
morph prompt materialize <hash>
```

Prompts are canonical objects. Materialization writes them to the working directory (or `.morph/prompts/`) for review.

## 6.3 Program Management

```
morph program create <file>
morph program edit
morph program show
```

Program manifests are created via `morph program create <file>` and exist only in the object store. There is no `programs/` directory.

## 6.4 Commit Workflow

```
morph add .
morph commit -m "message"
```

`morph add .` stages files and updates the staging index. `morph commit -m "message"` builds the tree from the index, creates the commit, and clears the index.

`--program` and `--eval-suite` are optional flags. When omitted, the commit behaves as a plain VCS commit (identity program, empty eval suite). When specified, `morph commit` validates:

- Program graph integrity (DAG, valid node/edge kinds)
- Eval suite presence and hash integrity
- Uses **recorded** observed metrics (from external evaluation or prior `morph eval record`) to form the eval contract

Morph does not run the eval suite; external tools do. Morph applies its **metrics validation layer** (aggregation, threshold checks) to reported scores.

## 6.5 Branching

```
morph branch <name>
morph checkout <name>
```

Branches are pointers to commits. `morph checkout` restores the working tree from the commit's tree hash.

## 6.6 Run ingestion

```
morph run record <file>
```

**Ingests** a Run object (JSON). Does not execute any program. External tools (IDE, agent, CI) produce the run and report it. Morph stores the Run, its Trace, and Artifacts. Used to record execution receipts.

## 6.7 Eval ingestion

```
morph eval record <file>
```

**Ingests** evaluation results (metrics against an EvalSuite). Does not run tests. External tools run the eval and report scores. Morph validates aggregation and thresholds and records the metrics for use in commits and merge dominance checks.

## 6.8 Merge

```
morph merge <branch>
```

Merge procedure:

1. Structural merge of program graphs
2. Determine the merge eval contract: **union** of both parents' eval suites (all metrics from both must be satisfied)
3. Merged program's **recorded** observed metrics (from external evaluation) must be supplied or already present
4. Validate **dominance**: merged program's observed metrics must meet or exceed both parents' `observed_metrics` (not merely the base thresholds)
5. Create merge commit if satisfied

If dominance is not achieved, merge aborts. Morph does not run evaluations; external tools do and report results.

This realizes THEORY.md §13.3: merge candidate R must satisfy R ⪰ P and R ⪰ Q under the behavioral preorder.

## 6.9 Rollup (Squash)

```
morph rollup <range>
```

Collapses multiple working-space commits into one commit-space identity.
Attaches evaluation summary.

Does not delete traces. Traces may be summarized via TraceRollup objects.

## 6.10 Annotations

```
morph annotate <object_hash> --kind feedback --data '{"rating": "good"}'
morph annotate <object_hash> --sub <event_id> --kind bookmark --data '{"title": "..."}'
morph annotations <object_hash>
morph annotations <object_hash> --sub <event_id>
```

`morph annotate` creates an Annotation object targeting any content-addressed object (or a sub-element within it).

`morph annotations` lists all annotations on a given object, optionally filtered by sub-target.

---

# 7. Reproducibility Model (v0)

Morph v0 defines reproducibility as:

- **Evaluation contract preservation**: re-running a committed program should satisfy its declared eval contract.
- **Explicit environment recording**: all runs record environment E (model, version, parameters, toolchain).
- **Deterministic replay is optional**: some environments support it; Morph does not require it.

This aligns with THEORY.md §18, Axiom 11: reproducibility is behavioral, not byte-level.

---

# 8. IDE Adapter Contract (v0)

IDE must emit:

- Prompt object definitions
- Program graph updates
- Run execution metadata (runs, traces, metrics)
- Filesystem diffs (working-space changes)
- Environment descriptor

**Primary write path:** Cursor MCP server. Cursor (or other IDEs) write to Morph via an MCP server that exposes morph-core operations as tools: record run, record eval, stage, commit, annotate. This is how the development environment reports execution evidence into Morph.

**Read path:** CLI. Users inspect history, status, annotations, and object contents via `morph log`, `morph status`, `morph annotations`, etc.

Morph is the source of truth. The filesystem is a projection.

---

# 9. Axiom Satisfaction

How v0 satisfies each Morph axiom:

| # | Axiom (THEORY.md §18) | v0 Mechanism |
|---|---|---|
| **A. Identity and Immutability** | | |
| 1 | Immutable Content-Addressed Objects | All objects (including Annotations) content-addressed by SHA-256, stored in `.morph/objects/` |
| 2 | Evidence Does Not Rewrite History | Run and Trace objects are separate from commits; evidence never mutates prior objects |
| **B. Program Algebra** | | |
| 3 | Effect Monad for Sequencing | Program execution produces `F(S)` — probabilistic outputs with traces; sequential composition via graph edges and bind semantics |
| 4 | Product State Spaces | State modeled as (Tree, execution context, metadata); composition via Trees |
| 5 | Zip for Parallelism | Independent subgraphs within a Program DAG execute in parallel; results combined |
| **C. Behavioral Semantics** | | |
| 6 | Evaluation Suites are Explicit Contracts | EvalSuite objects define T with metrics, ordering, and thresholds |
| 7 | Certificates are Comparable | Observed metrics in commits form certificate vectors; dominance is componentwise |
| 8 | Merge is Dominance of Joined Requirements | Merge requires metric dominance (componentwise max) over both parents (v0 §6.8) |
| **D. Environment and Decentralization** | | |
| 9 | Explicit Environment Recording | Run object records full environment (model, version, params, toolchain) |
| 10 | Decentralization | Content-addressed store requires no central authority; v0 is local-only but the design extends to distributed remotes |
| 11 | Behavioral Reproducibility | Reproducibility = eval contract preservation, not byte equality |

---

# 10. Out of Scope (v0)

- Distributed run deduplication
- Cryptographic signatures
- Policy enforcement at network level
- Advanced statistical testing (two-sample equivalence, Bayesian comparison)
- Federated merge verification
- Remote push/pull protocol

---

# 11. v0 Success Criteria

Morph v0 is successful if:

- Prompt programs can be versioned as first-class objects
- Runs are recorded as execution receipts with full environment
- Traces capture typed, addressable events
- Annotations can attach feedback, bookmarks, tags, and notes to any object
- Merge is behaviorally gated (dominance, not just structural)
- IDE integration works via the adapter contract
- Git users feel comfortable with the CLI
- Agent-generated patches are accountable (agent identity in runs)
- Higher-level tools (session capture, curation, workflow extraction) can be built on the object model without changes to Morph core

---

# 12. Guiding Constraint

If something feels too clever, remove it.

v0 must:

- Be coherent
- Be minimal
- Preserve Git mental models
- Satisfy the Morph axioms

Complexity can grow later.

Correct foundations cannot.

---

# 13. Implementation: Rust (v0)

The reference v0 implementation is in Rust.

- **Storage default:** Flat JSON files on the local filesystem (`.morph/objects/<sha256>.json`). Trait-based store interface allows future backends (e.g. SQLite).
- **Metrics validation:** Built-in aggregation (mean, min, p95, lower_ci_bound) and threshold/dominance checks. Morph does not execute tests; it validates and compares reported metric vectors.
- **Crates:** `morph-core` (library), `morph-cli` (read path + manual writes), `morph-mcp` (Cursor MCP server, primary write path from the IDE).
