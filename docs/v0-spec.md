# Morph v0 Specification
## Concrete System Design

This document defines the first implementation target for Morph.

It translates the ideas in:

- `README.md` (why Morph exists)
- `THEORY.md` (formal foundations)

into a minimal, buildable system.

This is **not** the final architecture.
It is the smallest coherent system that satisfies the Morph axioms.

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

---

# 2. Relationship to Theory

This section maps THEORY.md concepts to v0 constructs.

| Theory Concept | v0 Construct |
|---|---|
| State S = (D, C, M) | Tree (D), execution context in Run (C), Commit/Run metadata (M) |
| Prompt program P : S → F(S) | Program object (operator graph over Prompt blobs) |
| Effect functor F | Run execution: the act of running a Program produces probabilistic outputs |
| Identity program I | Built-in no-op Program (identity hash, passes state through unchanged) |
| Sequential composition Q ∘ P | Graph edges (data flow between operator nodes) |
| Parallel composition P ⊗ Q | Independent subgraphs within a Program graph |
| Evaluation suite T | EvalSuite object |
| Behavioral equivalence ≈ₜ | Eval pass: metrics within thresholds relative to a suite |
| Behavioral preorder P ⪯ Q | Metric dominance: Q meets or exceeds P's observed scores |
| Merge dominance | Merge commit must dominate both parents' observed metrics |
| Commit = behavioral identity | Commit object (program hash + eval contract) |
| Run = execution receipt | Run object (environment, metrics, trace, artifacts) |
| Annotations (metadata on objects) | Annotation object (feedback, bookmarks, tags on any object) |
| Trace events (addressable steps) | Trace events with IDs, types, and sequential ordering |

---

# 3. Repository Structure

A Morph repository contains:

```
.morph/
  objects/        # content-addressed objects (by hash)
  refs/           # branch pointers and tags
  runs/           # local run cache
  traces/         # trace payload storage
  config.json

prompts/          # working-space prompt files
programs/         # working-space program definitions
evals/            # working-space evaluation suites
```

### `.morph/objects/`
Content-addressed objects (by hash):

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
Branch pointers and tags.

### `.morph/runs/`
Local run cache (may mirror object store).

### `.morph/traces/`
Trace payload storage (content-addressed).

### Working Space

The top-level `prompts/`, `programs/`, and `evals/` directories are the **working space** — the filesystem projection of objects under active development. These are analogous to Git's working tree.

Changes in working space are not versioned until committed.

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

The v0 default backend is **flat files on the local filesystem** — the `.morph/` directory layout described above. This is the simplest possible implementation: objects are individual JSON files named by their SHA-256 hash, refs are files containing a hash, and traces are stored as separate files due to size.

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
    { "name": "file_or_node", "hash": "<object_hash>" }
  ]
}
```

---

## 4.3 Program

A Program encodes a prompt program — the core versioned unit in Morph. It corresponds to the theory's transformation P : S → F(S), represented as a directed acyclic graph of operators.

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

Evaluation suite — a first-class versioned object. This is the concrete realization of the theory's test suite T that defines behavioral equivalence.

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

Statistical sophistication is minimal in v0. Future versions or custom evaluators may implement two-sample equivalence tests (directly realizing ≈ₜ from the theory), Bayesian comparison, or human-in-the-loop evaluation pipelines.

---

## 4.5 Commit

```json
{
  "type": "commit",
  "program": "<program_hash>",
  "parents": ["<commit_hash>"],
  "message": "string",
  "timestamp": "...",
  "author": "...",
  "eval_contract": {
    "suite": "<eval_suite_hash>",
    "observed_metrics": { "metric_name": "value" }
  }
}
```

A Commit does **not** store run evidence. It stores the behavioral contract.

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

Environment recording is mandatory (Theory Axiom 8).

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

The theory requires an identity program I such that I ∘ P = P ∘ I = P.

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

Prompts are canonical objects. Materialization writes them to the filesystem working space for review.

## 6.3 Program Management

```
morph program create
morph program edit
morph program show
```

## 6.4 Commit Workflow

```
morph add .
morph commit -m "message"
```

`morph add` stages working-space changes into the object store.

`morph commit` validates:

- Program graph integrity (DAG, valid node/edge kinds)
- Eval suite presence and hash integrity
- Runs eval suite to record observed metrics
- Creates commit with eval contract

## 6.5 Branching

```
morph branch <name>
morph checkout <name>
```

Branches are pointers to commits.

## 6.6 Run Execution

```
morph run <program>
```

Produces:

- Run object
- Artifact(s)
- Trace
- Metrics

## 6.7 Evaluation

```
morph eval <program>
```

Runs eval suite and produces metrics summary.

## 6.8 Merge

```
morph merge <branch>
```

Merge procedure:

1. Structural merge of program graphs
2. Determine the merge eval contract: **union** of both parents' eval suites (all metrics from both must be satisfied)
3. Run the merged eval suite
4. Validate **dominance**: merged program's observed metrics must meet or exceed both parents' `observed_metrics` (not merely the base thresholds)
5. Create merge commit if satisfied

If evaluation fails or dominance is not achieved, merge aborts.

This realizes Theory §11: merge candidate R must satisfy R ⪰ P and R ⪰ Q under the behavioral preorder.

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

This aligns with Theory Axiom 10: reproducibility is behavioral, not byte-level.

---

# 8. IDE Adapter Contract (v0)

IDE must emit:

- Prompt object definitions
- Program graph updates
- Run execution metadata
- Filesystem diffs (working-space changes)
- Environment descriptor

IDE writes to Morph via local CLI or API.

Morph is the source of truth. The filesystem is a projection.

---

# 9. Axiom Satisfaction

How v0 satisfies each Morph axiom:

| # | Axiom | v0 Mechanism |
|---|---|---|
| 1 | Immutable Objects | All objects (including Annotations) content-addressed by SHA-256, stored in `.morph/objects/` |
| 2 | Associative Composition | Program graph edges define sequential composition; DAG structure ensures associativity |
| 3 | Identity Program | Well-known identity Program object (§5) |
| 4 | Evaluation-Relative Equivalence | EvalSuite objects define T; pass/fail is relative to suite |
| 5 | Behavioral Preorder | Observed metrics define ordering; dominance checked at merge |
| 6 | Merge Dominance | Merge requires metric dominance over both parents (§6.8) |
| 7 | Runs Don't Rewrite Commits | Run objects are separate; commit history is immutable |
| 8 | Explicit Environment Recording | Run object records full environment |
| 9 | Decentralization | Content-addressed store requires no central authority; v0 is local-only but the design extends to distributed remotes |
| 10 | Behavioral Reproducibility | Reproducibility = eval contract preservation, not byte equality |

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
