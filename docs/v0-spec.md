# Morph v0 Specification
## Concrete System Design

This document defines the first implementation target for Morph.

It translates the ideas in:

- [README.md](README.md) — project overview: motivation, what Morph does, and how it relates to Git
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
- Pipeline definitions (pipeline graphs)
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
- Cryptographic content-blob replication / signed transparency log

Those can come later. (A line-oriented JSON-RPC remote protocol over
SSH **is** in v0 — see §10 — but the cross-fleet content-blob
replication + auditing layer is out of scope.)

### Morph is pure VCS (v0)

Morph does **not** execute pipelines or run evaluations. All LLM calls, tool execution, and test runs happen in external tools (IDEs, agents, CI). Morph's role is to:

- **Store** immutable content-addressed objects
- **Record** execution evidence that external tools report (runs, traces, metrics)
- **Version** pipelines with behavioral contracts (commits)
- **Gate** merges on metric dominance

Commands like `morph run record` and `morph eval record` **ingest** results; they do not run anything. The primary write path from the IDE is the **Cursor MCP server**, which reports session data into Morph. Reading and inspection happen via the CLI.

---

# 2. Relationship to Theory

This section maps THEORY.md concepts to v0 constructs.

| Theory Concept | Paper / THEORY.md | v0 Construct |
|---|---|---|
| State S = (D, C, M) | paper §3.1 | Tree (D), execution context in Run (C), Commit/Run metadata (M) |
| Actor = (id, type, env_config) | paper §3.2 | `ActorRef` struct (`id`, `actor_type`, `env_config`) |
| Environment E | §5.2 | Run `environment` field; Commit `env_constraints` field |
| Pipeline P : S → F(S) | paper §3.3 | Pipeline object (operator graph over Prompt blobs) |
| Pipeline node types κ | paper §3.3 | Node `kind`: prompt_call, tool_call, retrieval, transform, identity, **review** |
| Per-node env ε : V → EnvConfig | paper §3.3 | PipelineNode `env` field |
| Attribution α : V → 2^A | paper §3.3 | Pipeline `attribution` field with `actors` array (set of ActorRefs) |
| Effect functor F | §6.1 | Run execution: running a Pipeline produces probabilistic outputs |
| Identity pipeline | §7.2 | Built-in no-op Pipeline (identity hash, passes state through unchanged) |
| Sequential composition Q ∘ P | §7.1 | Graph edges (data flow between operator nodes) |
| Parallel composition P ⊗ Q | §8.3 | Independent subgraphs within a Pipeline graph |
| Multi-agent contributions | paper §3.4 | Run `contributors` field, Commit `contributors` field |
| Evaluation suite T | §10 | EvalSuite object (cases, metrics with direction) |
| Fixture source | paper §4.1 | EvalCase `fixture_source` field |
| Contract satisfaction | §11.1 | Eval pass: aggregated metrics meet declared thresholds |
| Behavioral preorder P ⪯ Q | §11.3 | Metric dominance: Q meets or exceeds P's certified scores |
| Certificate vector v ∈ V_T | §11.2 | `observed_metrics` in Commit's eval contract |
| Commit = behavioral certificate | paper §5.1 | Commit object (tree + pipeline + eval + env_constraints + evidence_refs) |
| Evidence refs | paper §5.1 | Commit `evidence_refs` field (hashes of Runs and Traces) |
| Certificate lattice join | §13.2 | Componentwise max of parent metrics during merge |
| Merge as behavioral join | paper §5.2 | Merge commit must dominate both parents' observed metrics |
| Metric retirement | paper §5.3 | `retire_metrics()` + `--retire` CLI flag on merge |
| Working space vs commit space | §14 | Working directories vs `.morph/objects/` |
| Run = execution receipt | §9.1 | Run object (environment, metrics, trace, artifacts) |
| Trace as DAG of events | §9.2 | Trace events with IDs, types, and sequential ordering (v0 simplifies to linear) |
| Annotations | §17.1 | Annotation object (feedback, bookmarks, tags on any object) |
| Pipeline provenance | §17.2 | Pipeline `provenance` field (derived_from_run, method, etc.) |

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

`morph init` creates only `.morph/` (like `git init`): `objects/`, `refs/heads/`, `runs/`, `traces/`, `prompts/`, `evals/`, `config.json`, and `refs/HEAD` (pointing at `heads/main`). The working directory — the user's project directory — is the working space. There are no top-level `prompts/`, `pipelines/`, or `evals/` directories.

### `.morph/objects/`
The single source of truth. Every object is stored here as `<sha256>.json` (content-addressed). Object types:

- Blobs
- Trees
- Pipelines
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

`.morph/prompts/` and `.morph/evals/` are optional metadata directories for prompt and eval suite definitions. The `pipelines/` directory is eliminated: pipeline manifests are created via `morph pipeline create <file>` and exist only in the object store. Morph can be used as a plain VCS without prompts or evals.

Changes in working space are not versioned until committed.

### `.morphignore`

A **`.morphignore`** file at the repository root (same directory as `.morph/`) specifies paths that Morph will not include in `morph status` or `morph add`. Same syntax and semantics as [`.gitignore`](https://git-scm.com/docs/gitignore): one pattern per line, `#` comments, blank lines ignored, and patterns are relative to the repo root. Examples:

- `target/` — ignore the `target` directory and everything under it
- `*.log` — ignore any file ending in `.log`
- `node_modules/` — ignore dependency trees you do not want to version as content

If `.morphignore` is missing, nothing is excluded beyond the built-in rules (e.g. `.morph/` internals). This keeps working-space scans and commits focused on the files that define your pipelines, prompts, and evals.

### Commit Space

The `.morph/objects/` store and the commit graph comprise the **commit space** — stabilized, evaluation-certified behavioral identities. Rollup collapses exploratory working-space history into stable commit-space identities.

### Storage Backend

Morph's core logic depends on an abstract storage interface, not on a specific backend. Any system that can implement the following operations qualifies:

- **put(object) → hash** — serialize, hash, and store an immutable object
- **get(hash) → object** — retrieve an object by its content hash
- **has(hash) → bool** — check existence
- **list(type) → [hash]** — enumerate objects of a given type
- **hash_object(object) → hash** — compute the content hash without storing (same algorithm as put, no side effects)
- **ref_read(name) → hash** — resolve a named reference (branch, tag)
- **ref_write(name, hash)** — update a named reference
- **ref_read_raw(name) → string** — raw ref content (e.g. `"ref: heads/main"` for symbolic HEAD)
- **ref_write_raw(name, value)** — write raw ref content (symbolic or hash)

The v0 implementation provides a single `FsStore` backend with several on-disk variants, selected by `repo_version` in `.morph/config.json`:

| Store version | Backend | Hash function | Object layout | Notes |
|---|---|---|---|---|
| `"0.0"` | **FsStore** | SHA-256 of canonical JSON | flat `objects/<hash>.json` | Created by `morph init`. Legacy. |
| `"0.2"` | **FsStore (Git-format)** | SHA-256 of `"blob " + len + "\0" + canonical_json` | flat `objects/<hash>.json` | First Git-format hashing version. Reached by `morph upgrade` from 0.0. |
| `"0.3"` | **FsStore (Git-format)** + tree commits | Same as 0.2 | flat `objects/<hash>.json` | Adds file tree storage in commits. |
| `"0.4"` | **FsStore (Git-format, fan-out)** | Same as 0.2 | `objects/<xx>/<hash[2..]>.json` (Git-style fan-out) | Scales to large object stores. |
| `"0.5"` | **FsStore (Git-format, fan-out)** | Same as 0.2 | Same as 0.4 | Config-only bump that locks in the merge state machine (`.morph/MERGE_*` files, `index.unmerged_entries`). Old binaries opening a 0.5 repo see a clear `RepoTooNew` error instead of silently mishandling an in-progress merge. Current. |

All variants share the same directory layout outside of `objects/`: `refs/` for references and type-index directories (`runs/`, `traces/`, `prompts/`, `evals/`) for fast type-filtered listing. The `list(type)` operation uses type-index directories when available, falling back to a full object scan for types without indexes.

Migration between store versions is handled by `morph upgrade` (CLI only). 0.0 → 0.2 rewrites every object under the new hash and updates all internal references. 0.3 → 0.4 moves objects into the fan-out layout; hashes are preserved. 0.4 → 0.5 is a config-only bump (no object rewrites).

Other backends are explicitly anticipated (SQLite, S3, database-backed). The filesystem layout is a projection of the storage backend, not the backend itself. CLI and IDE integrations talk to the backend interface.

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

## 4.3 Pipeline

A Pipeline encodes a prompt pipeline — the core versioned unit in Morph. It corresponds to THEORY.md §6.1's transformation P : S → F(S), represented as a directed acyclic graph of operators.

```json
{
  "type": "pipeline",
  "graph": {
    "nodes": [
      {
        "id": "node_id",
        "kind": "prompt_call | tool_call | retrieval | transform | identity | review",
        "ref": "<blob_hash or null>",
        "params": { },
        "env": { "model": "gpt-4o", "temperature": 0.7 }
      }
    ],
    "edges": [
      { "from": "node_id", "to": "node_id", "kind": "data | control" }
    ]
  },
  "prompts": ["<blob_hash>"],
  "eval_suite": "<eval_suite_hash>",
  "attribution": {
    "node_id": {
      "agent_id": "<string>",
      "agent_version": "<string or null>",
      "actors": [
        { "id": "<string>", "actor_type": "human | agent", "env_config": { } }
      ]
    }
  },
  "provenance": {
    "derived_from_run": "<run_hash or null>",
    "derived_from_trace": "<trace_hash or null>",
    "derived_from_event": "<event_id or null>",
    "method": "manual | extracted | composed"
  }
}
```

**Node kinds** (paper §3.3, κ):
- `prompt_call` — invokes an LLM with a referenced Prompt blob
- `tool_call` — invokes an external tool
- `retrieval` — fetches context from a document store
- `transform` — deterministic data transformation
- `identity` — no-op pass-through (corresponds to the theory's identity pipeline I)
- `review` — explicit acceptance or modification decision (human approving a diff, agent evaluating a candidate)

**Per-node environment** (paper ε : V → EnvConfig ∪ {⊥}): The `env` field on each node records which model and toolchain ran that node. It is null for human-only nodes or when environment is unspecified. This lets the record distinguish "agent A used gpt-4o for generation" from "agent B used claude-4 for review" within the same pipeline.

**Edge kinds:**
- `data` — output of source flows as input to target (sequential composition)
- `control` — execution ordering without data dependency

Nodes with no edges between them execute in parallel (parallel composition P ⊗ Q from the theory).

**Attribution** records which actors contributed to each node in the pipeline DAG. This is the v0 realization of the paper's attribution function α : V → 2^A (set of Actor IDs per node). The `attribution` field is optional — null or empty when all nodes share a single author. Each entry maps a node ID to an attribution record. The `actors` array is the set-valued attribution from the paper: a review node where a human accepts and edits an agent's diff gets `actors: [{id: "agent-1", actor_type: "agent"}, {id: "human-1", actor_type: "human"}]`. The legacy `agent_id` field is kept for backward compatibility; when `actors` is present it takes precedence. Certification remains holistic — the certificate vector applies to the composed pipeline, not to individual actors' contributions.

**Provenance** records how this Pipeline was created. The `provenance` field is optional — null fields indicate unknown or not-applicable. When a Pipeline is extracted from a successful Run (e.g., distilling an agent session into a reusable workflow), provenance records the source Run, Trace, and optionally the specific event within that Trace. The `method` field indicates whether the Pipeline was written by hand (`manual`), extracted from a session (`extracted`), or built by composing existing Pipelines (`composed`).

---

## 4.4 EvalSuite

Evaluation suite — a first-class versioned object. This is the concrete realization of the theory's evaluation suite T (THEORY.md §10) that defines behavioral contracts.

### Evaluation interface

Morph's core logic depends on an abstract evaluation interface, not on a specific evaluation implementation. The interface has one operation:

- **evaluate(pipeline, eval_suite, environment) → metrics**

Given a Pipeline, an EvalSuite, and an Environment, produce a dictionary of metric name → score. Morph uses the returned scores for:

- Recording `observed_metrics` in Commits
- Checking dominance during merge
- Gating commit creation

How scores are produced is the evaluator's concern, not Morph's. An evaluator might:

- Run the pipeline against test cases and compute automated scores
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
      "threshold": 0.0,
      "direction": "maximize"
    }
  ]
}
```

The `aggregation` field is an open string. It tells the evaluator how to reduce per-case scores into a single metric value. Well-known values are listed below. Custom evaluators may define their own.

The `direction` field specifies the ordering for this metric: `"maximize"` (default, higher is better) or `"minimize"` (lower is better). This corresponds to THEORY.md §10.4's ordering direction. Threshold checks and dominance checks respect direction: for "maximize" metrics, the score must be ≥ threshold; for "minimize" metrics, the score must be ≤ threshold.

### v0 evaluator (built-in)

The v0 default evaluator is minimal:

- Runs the Pipeline against each case in the EvalSuite
- Computes per-case scores by comparing outputs to expected values
- Aggregates using the declared aggregation function
- Returns the metric dictionary

**Built-in aggregation functions:**
- `mean` — arithmetic mean of per-case scores
- `min` — worst-case score
- `p95` — 95th percentile
- `lower_ci_bound` — lower bound of a 95% confidence interval (a minimal nod to distributional stability)

**Pass criteria:** for each metric, the aggregated score must satisfy the threshold in the metric's direction. For "maximize" metrics (default): score ≥ threshold. For "minimize" metrics: score ≤ threshold.

Statistical sophistication is minimal in v0. Future versions or custom evaluators may implement two-sample equivalence tests, Bayesian comparison, or human-in-the-loop evaluation pipelines (see THEORY.md §10.4 for the general certification framework).

---

## 4.5 Commit

```json
{
  "type": "commit",
  "tree": "<tree_hash>",
  "pipeline": "<pipeline_hash>",
  "parents": ["<commit_hash>"],
  "message": "string",
  "timestamp": "...",
  "author": "...",
  "contributors": [
    { "id": "...", "role": "<string or null>" }
  ],
  "eval_contract": {
    "suite": "<eval_suite_hash>",
    "observed_metrics": { "metric_name": "value" }
  },
  "env_constraints": { "model": "gpt-4o", "runner": "ci-v2" },
  "evidence_refs": ["<run_hash>", "<trace_hash>"],
  "morph_version": "0.3"
}
```

The `tree` field records the root hash of the file tree at commit time — the same role as Git's tree in a commit. `morph_version` records the store version that created this commit. Commits from before version 0.3 have `tree: null` and `morph_version: null`.

A Commit stores the behavioral contract AND the file tree snapshot (paper Definition 5.1). The full tuple is:

```
c = (tree_hash, pipeline_id, T, v, parents, env_constraints, evidence_refs)
```

`pipeline` and `eval_contract` default to the identity pipeline and empty eval suite when not specified, making `morph commit -m 'message'` work as a plain VCS commit. (The JSON field accepts both `pipeline` and `program` for backward compatibility.)

The `observed_metrics` field records the metrics achieved at commit time. This is critical for merge: the merge commit must dominate these observed values, not merely pass the suite's base thresholds.

The `env_constraints` field records the environment in which the scores were captured (paper Definition 5.1). Without this, scores from different environments are not comparable.

The `evidence_refs` field lists hashes of supporting Run and Trace objects that back up the commit's behavioral claims (paper Definition 5.1). Runs are receipts; evidence_refs link a commit to the receipts that justify it.

The `contributors` field is optional and lists all agents and humans that contributed to this commit. The `author` field remains the primary committer (analogous to Git's author). `contributors` captures the broader set — agents that authored pipeline nodes, ran evaluations, or produced artifacts that inform this commit.

Commits are claims. Runs are receipts.

---

## 4.6 Run

A Run is an execution receipt.

```json
{
  "type": "run",
  "pipeline": "<pipeline_hash>",
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
  },
  "contributors": [
    {
      "id": "...",
      "version": "...",
      "policy": "<policy_hash or null>",
      "role": "<string or null>"
    }
  ]
}
```

Runs do not modify commit history.

The `commit` field is null for exploratory runs in working space. It references a commit hash when the run serves as evidence for a committed pipeline. (The JSON field accepts both `pipeline` and `program` for backward compatibility.)

The `agent` field records the primary or orchestrating agent. The `contributors` field is optional and lists all agents that participated in producing the run's outputs. Each contributor has the same identity fields as `agent`, plus an optional `role` string (e.g., `"retrieval"`, `"generation"`, `"review"`). For single-agent runs, `contributors` is null or empty. This supports the theory's multi-agent attribution model (paper §3.5) — recording who contributed while acknowledging that credit assignment from the certificate vector to individual agents requires additional analysis.

Environment recording is mandatory (THEORY.md §18, Axiom 7).

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
- `target` — hash of the object being annotated (Run, Trace, Commit, Pipeline, Blob, etc.).
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

# 5. The Identity Pipeline

THEORY.md §7.2 requires an identity pipeline I such that I ∘ P = P ∘ I = P.

In v0, this is a well-known Pipeline object:

```json
{
  "type": "pipeline",
  "graph": {
    "nodes": [{ "id": "passthrough", "kind": "identity", "ref": null, "params": {}, "env": null }],
    "edges": []
  },
  "prompts": [],
  "eval_suite": null,
  "provenance": null
}
```

Its hash is deterministic and serves as the identity element for pipeline composition. (The `"type"` field accepts both `"pipeline"` and `"program"` for backward compatibility.)

---

# 6. CLI Commands (v0)

Morph CLI mirrors Git where possible.

## 6.1 Repository Management

```
morph init [--bare] [--no-default-policy]
morph status
morph log
```

`morph init` creates a working repo in `.morph/`. Fresh repos receive an opinionated default `RepoPolicy` that requires `tests_total` and `tests_passed` on every commit (see §11.1). The hidden `--no-default-policy` flag exists for the test harness; production use should rely on `morph policy require-metrics` to relax the gate when needed.

`morph status` prints the working tree, accumulated runs/traces, and an **Evidence** summary block: the most recent metric-bearing run, the registered default eval suite (or a hint to register one), and any unaddressed gaps. The same data is exposed structurally over MCP via `morph_status` and `morph_eval_gaps`.

## 6.2 Prompt Operations

```
morph prompt create <file>
morph prompt materialize <hash> [--output <path>]
morph prompt show [<ref>] [--run-upgrade]
```

Prompts are canonical objects. Materialization writes them to the working directory (or `.morph/prompts/`) for review.

`morph prompt show` prints the prompt text from a Run. Ref follows a Git-like syntax: `latest` (default, most recent run), `latest~N` or `latest-N` (Nth run back), or a 64-char run hash. Pass `--run-upgrade` to attempt one store upgrade and retry when the trace referenced by the run is not found.

## 6.3 Pipeline Management

```
morph pipeline create <file>
morph pipeline show <hash>
morph pipeline identity-hash
morph pipeline extract --from-run <run_hash>
```

Pipeline manifests are created via `morph pipeline create <file>` and exist only in the object store. There is no `pipelines/` directory.

`morph pipeline identity-hash` prints the hash of the identity pipeline (creating it in the store if needed). Useful for hook scripts and automation.

`morph pipeline extract --from-run <run_hash>` extracts a Pipeline from a recorded Run. For session-backed Runs (created by `morph run record-session`), this produces a deterministic minimal graph: a `generate` (prompt_call) node flowing into a `review` node. The extracted Pipeline includes provenance (`derived_from_run`, `derived_from_trace`, `derived_from_event`, `method: "extracted"`), attribution derived from the Run's agent and contributors, and a prompt blob reference. The extracted Pipeline is a first-class object reusable anywhere a Pipeline hash is accepted (e.g., `morph commit --pipeline <hash>`).

## 6.4 Commit Workflow

```
morph add .
morph commit -m "message"
morph commit -m "message" --from-run <run_hash>
morph commit -m "message" --metrics '{"tests_passed":42,"tests_total":42}'
morph commit -m "message" --allow-empty-metrics              # bypass policy.required_metrics
morph commit -m "message" --new-cases id1,id2                # record introduced acceptance cases
```

`morph add .` stages files and updates the staging index. `morph commit -m "message"` builds the tree from the index, creates the commit, and clears the index.

`--pipeline` and `--eval-suite` are optional flags. When omitted, the commit behaves as a plain VCS commit (identity pipeline, empty eval suite). When specified, `morph commit` validates:

- Pipeline graph integrity (DAG, valid node/edge kinds)
- Eval suite presence and hash integrity
- Uses **recorded** observed metrics (from external evaluation or prior `morph eval record`) to form the eval contract

`--eval-suite <hash>` is optional: when omitted, the commit picks up `policy.default_eval_suite` so the suite registered by `morph eval add-case` flows into every commit automatically. Pass an explicit hash to override.

`--from-run <run_hash>` derives commit provenance from a recorded Run:

- `evidence_refs`: the run hash and its trace hash
- `env_constraints`: the Run's environment (model, version, parameters, toolchain)
- `contributors`: the run's agent (with role "primary") and any additional contributors
- `observed_metrics`: copied from the Run when no explicit `--metrics` is given

If the run hash points to a missing object, a non-Run object, or a Run whose trace cannot be resolved, commit creation fails with a clear error. When `--from-run` is omitted, provenance fields are absent (plain VCS commit).

`--allow-empty-metrics` bypasses the commit-time policy gate. The default policy on a fresh repo requires `tests_total` and `tests_passed` (see §11.1); this flag is the documented escape hatch for genuinely metric-less commits (rebases, backports). It is recorded in the trace so reviewers can audit when the gate was skipped.

`--new-cases id1,id2` writes an `introduces_cases` annotation against the new commit. The case ids are caller-defined (typically `<spec_basename>:<case_name>` matching the suite). `morph merge-plan` reads these annotations and prints per-branch case provenance so a reviewer can see which acceptance cases each branch contributed.

Morph does not run the eval suite; external tools do. Morph applies its **metrics validation layer** (aggregation, threshold checks) to reported scores.

## 6.5 Branching

```
morph branch [<name>]
morph checkout <name|hash>
```

`morph branch` without arguments lists all branches. With a name, it creates a new branch at HEAD. Branches are pointers to commits. `morph checkout` accepts a branch name or a 64-char commit hash (detached HEAD). If the commit has a tree, the working directory is restored from it.

## 6.6 Run ingestion

```
morph run record <file> [--trace <file>] [--artifact <file>...]
morph run record-session --prompt <text> --response <text> [--model-name <name>] [--agent-id <id>]
```

`morph run record` **ingests** a Run object (JSON). Does not execute any pipeline. External tools (IDE, agent, CI) produce the run and report it. Morph stores the Run, its Trace, and Artifacts.

`morph run record-session` is a convenience command that creates a Run + Trace from a single prompt/response pair. Used by hook scripts and automation to record IDE sessions without constructing the full Run JSON.

## 6.7 Eval ingestion

Morph treats acceptance cases and metric-bearing runs as first-class
objects. The `morph eval` family covers both directions: ingesting
human-authored specs into an `EvalSuite`, and ingesting test-runner
output into a metric-bearing `Run`. Both feed the merge-dominance gate.

```
morph eval record <file>                       # ingest precomputed metrics JSON
morph eval from-output [--runner R] [--record] <file>  # parse captured stdout (use - for stdin)
morph eval run -- <cmd...>                     # exec command, parse stdout, write Run linked to HEAD
morph eval add-case <file_or_dir>...           # YAML / Cucumber → EvalCase, append to default suite
morph eval suite-from-specs <dir>              # rebuild the default suite from a directory tree
morph eval suite-show [--suite <hash>] [--json]   # print the cases in a suite
morph eval gaps [--json] [--fail-on-gap]       # report missing behavioral evidence
```

- `record` — Ingests a `{"metrics": {...}}` JSON file. Used when an
  external CI run already produced canonical scores. Does not run any
  test command.
- `from-output` / `run` — Wrap the supported runners (cargo / pytest /
  vitest / jest / go) plus an `auto` mode that sniffs the output. They
  produce the same canonical metric map (`tests_passed`,
  `tests_failed`, `tests_total`, `pass_rate`, `wall_time_secs`, …) and
  can optionally write a `Run` object linked to HEAD so a subsequent
  `morph commit --from-run <hash>` inherits the metrics as
  `observed_metrics`.
- `add-case` / `suite-from-specs` — Convert YAML specs and Cucumber
  `.feature` files into `EvalCase` objects. The first ingestion writes
  a fresh `EvalSuite` and stores its hash under
  `policy.default_eval_suite`; subsequent ingestions extend the same
  suite (deduping by case id).
- `suite-show` — Inspect the registered default suite (or any suite by
  hash). `--json` is provided for tooling.
- `gaps` — Returns a structured list of unaddressed evidence gaps:
  `empty_head_metrics`, `empty_default_suite`, `no_recent_run`, and
  (in reference mode) `git_morph_drift` when the latest git HEAD
  has no mirrored Morph commit. The Cursor stop-hook
  (`morph-record-checks.sh`, installed by `morph setup cursor`)
  shells out to this command.

The same surface is exposed over MCP as `morph_record_eval`,
`morph_eval_from_output`, `morph_eval_run`, `morph_add_eval_case`,
`morph_eval_suite_from_specs`, `morph_eval_suite_show`, and
`morph_eval_gaps`.

## 6.8 Merge

### Merge planning (read-only inspection)

```
morph merge-plan <branch>
morph merge-plan <branch> --retire 'old_metric1,old_metric2'
```

Inspects the current HEAD and the target branch, resolves both parent commits, computes the merge eval context, and prints a human-readable summary including:

- Current and other branch parent hashes
- Parent metrics from each branch
- Resolved union eval suite (auto-computed from both parents' suites)
- Merged reference bar the candidate must dominate
- Retired metrics (if `--retire` is specified)

This command does not create a commit. It is the inspection/planning step.

### Merge execution

The `morph merge` command runs in two modes. The **stateful** form is the default and matches Git's three-step lifecycle; the **single-shot** form is a backwards-compatible shortcut for scripts that already have the merged metrics in hand.

#### Stateful flow

```
morph merge <branch>                                # start
morph merge resolve-node <node-id> --pick ours|theirs|base   # for pipeline conflicts
morph merge --continue [-m <message>] [--author <name>]      # finalize
morph merge --abort                                  # back out
```

`morph merge <branch>`:

1. Resolves both parents and the LCA (`merge_base`).
2. If the result is `AlreadyMerged` / `AlreadyAhead` / `FastForward`, the CLI updates the local ref (and working tree) and exits.
3. Otherwise runs the structural 3-way merge (`treemerge`, `pipemerge`, `objmerge`), applies workdir writes, and writes the merge state files (`MERGE_HEAD`, `ORIG_HEAD`, `MERGE_MSG`, plus `MERGE_PIPELINE` / `MERGE_SUITE` when relevant). If the merge has no conflicts the CLI immediately auto-finalizes by calling `continue_merge`.
4. When there *are* conflicts the CLI exits non-zero with a list of the unmerged paths and pipeline-node conflicts, so the user can resolve them and run `--continue`.

`morph merge resolve-node <id> --pick ours|theirs|base` writes the chosen side of a single pipeline-node conflict into `MERGE_PIPELINE.json` and removes it from the in-progress conflict list. `--continue` is unblocked once every conflict is resolved.

`morph merge --continue` reads the staged tree and merge state, builds the merge commit with `parents = [HEAD, MERGE_HEAD]`, runs the dominance gate (when `merge_policy != "none"`), unions `evidence_refs` from both parents, advances the active branch ref, and clears all merge state. It refuses (without making partial commits) if no merge is in progress, the staging index still has unmerged entries, or the working tree is dirty.

`morph merge --abort` rewinds the working tree to `ORIG_HEAD` and clears the merge state. It refuses when no merge is in progress so users get a clear signal rather than a silent no-op.

#### Single-shot flow

```
morph merge <branch> -m <message> --pipeline <hash> --metrics '<json>'
morph merge <branch> -m <message> --pipeline <hash> --eval-suite <hash> --metrics '<json>'
morph merge <branch> -m <message> --pipeline <hash> --metrics '<json>' --retire 'old_metric1,old_metric2'
```

When the user supplies `--pipeline`, `--metrics`, and `-m` together, `morph merge` skips the stateful flow and calls `prepare_merge` + `execute_merge` directly. `--eval-suite` is optional: when omitted, the union of both parents' evaluation suites is computed automatically. `--retire` (comma-separated) drops the named metrics from the union suite before the dominance check (paper §5.3).

In both flows the merge procedure is:

1. Resolve both parent commits (current HEAD and target branch).
2. Combine evaluation suites: if `--eval-suite` is provided, use it; otherwise compute T = T1 ⊎ T2 (union by metric id).
3. Apply metric retirement (if `--retire` is specified): remove retired metrics from the union suite.
4. Record the bar: embed each parent's scores into V_T and record the best from either parent on every surviving metric.
5. Validate **dominance**: the merged pipeline's observed metrics must meet or exceed both parents' `observed_metrics` on every surviving metric (direction-aware). Only metrics in the post-retirement union suite are checked. The check is skipped when `RepoPolicy.merge_policy = "none"` (see §11.1).
6. Create merge commit if satisfied. The commit's `evidence_refs` is the deduped sorted union of both parents' `evidence_refs` (paper §5.1).

If dominance is not achieved, merge aborts with a detailed explanation identifying which metric failed, the merged and parent values, and which parent was violated. Morph does not run evaluations; external tools do and report results.

This realizes the paper's merge semantics (§5) and metric retirement (§5.3): merge candidate R must dominate both parents on every non-retired metric.

## 6.9 Rollup (Squash)

```
morph rollup <base_ref> <tip_ref> [-m <message>]
```

Collapses multiple working-space commits into one commit-space identity. The new commit has `base_ref` as its parent and uses the pipeline and eval contract from the tip commit.

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

## 6.11 Object Inspection

```
morph show <hash>
```

`morph show` accepts any Morph object hash and prints its stored content as pretty JSON. Works for commits, runs, traces, pipelines, blobs, eval suites, annotations, and all other object types. Use it to inspect commit provenance (`evidence_refs`, `env_constraints`, `contributors`) after creation.

## 6.12 Utilities

```
morph hash-object <file>
morph upgrade
morph visualize [<path>] [--port <port>] [--interface <addr>]
morph serve [--repo <name>=<path>] [--port <port>] [--interface <addr>] [--org-policy <file>]
```

`morph hash-object` reads a Morph object from a JSON file, stores it, and prints its content hash. Used by hook scripts that need to construct and store objects outside the normal CLI workflow.

`morph upgrade` migrates the repository store to the latest version (e.g. 0.0 → 0.2 → 0.3). Required before using MCP on older repos.

`morph visualize` starts a local web server for browsing the repo in a browser: commit strip, detail panel (message, author, pipeline, eval contract, prompts), and an object browser. The web UI is embedded in the binary. `morph serve` is the full hosted service with multi-repo support, a stable JSON API, behavioral status derivation, and org-level policy (see §15).

## 6.13 Tags

```
morph tag <name>            # tag the current HEAD commit
morph tag                   # list all tags (`<name> <hash>` per line)
morph tag --json            # list all tags as a JSON envelope
morph tag --delete <name>   # delete a tag
```

Tags are named pointers to commits, stored under `.morph/refs/tags/`. Creating a tag on a non-existent branch or empty repo fails with a clear error.

## 6.14 Stash

```
morph stash save [-m <message>]  # save staged changes
morph stash pop                  # restore most recent stash (LIFO)
morph stash list                 # list stashed entries
```

Stash saves the current staging index as a stash entry and clears the index. Pop restores the most recent entry. Stash entries are LIFO-ordered.

## 6.15 Revert

```
morph revert <commit_hash>
```

Creates a new commit that restores the parent's file tree, effectively undoing the specified commit. Reverts the root commit to an empty tree. Fails if the hash does not reference a commit.

## 6.16 Diff

```
morph diff <ref1> <ref2>   # compare two commits/branches
morph diff HEAD <ref>      # compare HEAD against another ref
```

Compares the file trees of two commits and reports added, deleted, and modified files.

## 6.17 Tap (Trace Analysis)

```
morph tap summary                          # overview of all runs in the repo
morph tap inspect <run_hash>               # grouped steps for a single run (pass "all" for every run)
morph tap diagnose [<run_hash>]            # recording quality report (default: all runs)
morph tap export --mode <mode>             # export eval cases (prompt-only, with-context, agentic)
morph tap trace-stats <trace_hash>         # detailed event-level statistics for a trace
morph tap preview <run_hash> [--mode M]    # labeled prompt/context/response preview
```

Tap reads traces and runs from the store, groups events into logical steps (prompt, response, tool calls, file operations), and produces structured output for evaluation frameworks. `export` supports filtering by model (`--model`), agent (`--agent`), and minimum step count (`--min-steps`), and writes to a file with `--output`.

### 6.17a Structured trace views

```
morph traces summary [--limit N] [--json]
morph traces task-structure <run_or_trace_hash>
morph traces target-context <run_or_trace_hash>
morph traces final-artifact <run_or_trace_hash>
morph traces semantics <run_or_trace_hash>
morph traces verification <run_or_trace_hash>
```

Higher-level views over the same traces, for replay and eval-case construction: task classification (phase, scope, target files/symbols, goal), target file/function context, final artifact (function text / file snippet / patch summary), change semantics, and verification commands. These are also exposed as MCP tools (`morph_get_recent_trace_summaries`, `morph_get_trace_task_structure`, etc.).

## 6.18 IDE Setup

```
morph setup cursor     # install Cursor MCP config, hooks, and rules
morph setup opencode   # install OpenCode MCP config, AGENTS.md, and plugin
```

Writes (or merges into) the IDE-specific configuration files in the project directory. Idempotent — safe to re-run.

---

# 7. Reproducibility Model (v0)

Morph v0 defines reproducibility as:

- **Evaluation contract preservation**: re-running a committed pipeline should satisfy its declared eval contract.
- **Explicit environment recording**: all runs record environment E (model, version, parameters, toolchain).
- **Deterministic replay is optional**: some environments support it; Morph does not require it.

This aligns with THEORY.md §18, Axiom 8: reproducibility is behavioral, not byte-level.

---

# 8. IDE Adapter Contract (v0)

IDE must emit:

- Prompt object definitions
- Pipeline graph updates
- Run execution metadata (runs, traces, metrics)
- Filesystem diffs (working-space changes)
- Environment descriptor

**Primary write path:** Cursor MCP server. Cursor (or other IDEs) write to Morph via an MCP server that exposes morph-core operations as tools: record run, record eval, stage, commit, annotate. This is how the development environment reports execution evidence into Morph.

**Read path:** CLI. Users inspect history, status, annotations, and object contents via `morph log`, `morph status`, `morph annotations`, etc.

Morph is the source of truth. The filesystem is a projection.

---

# 9. Axiom Satisfaction

How v0 satisfies each Morph axiom:

| # | Axiom (Paper §7 / THEORY.md §18) | v0 Mechanism |
|---|---|---|
| 1 | Content-Addressed, Immutable Objects | All objects content-addressed by SHA-256, stored in `.morph/objects/` |
| 2 | Evidence Does Not Rewrite History | Run and Trace objects are separate from commits; evidence never mutates prior objects |
| 3 | Pipeline Steps Compose Cleanly | Pipeline DAG with data/control edges; sequential via bind, parallel via independent subgraphs; identity pipeline as no-op |
| 4 | Evaluation Suites are Explicit Contracts | EvalSuite objects define T with metrics (name, aggregation, threshold, direction) and fixture sources |
| 5 | Scores are Partially Ordered | Observed metrics in commits form certificate vectors; dominance is componentwise with per-metric direction |
| 6 | Merge Records Scores From Both Parents | Parent commits retain their `observed_metrics`; merge stores the merged candidate's metrics; merge flow computes union suite (with optional retirement), reference bar from parents, and requires dominance (see §6.8 notes) |
| 7 | Environment is Part of the Record | Run `environment` + Commit `env_constraints` record full environment (model, version, params, toolchain) |
| 8 | Reproducibility Means Re-Running the Checks | Reproducibility = eval contract preservation under declared environment, not byte equality |
| — | Actor Model (paper §3.2) | `ActorRef` struct with id/type/env_config; attribution as sets of actors per pipeline node |
| — | Review Nodes (paper §3.3) | `review` node kind for explicit acceptance/modification decisions with attribution |
| — | Metric Retirement (paper §5.3) | `retire_metrics()` removes obsolete metrics from union suite during merge; `--retire` CLI flag |

---

# 10. Remote Sync (Phase 5)

Morph supports distributing history across multiple repositories via named remotes.

## 10.1 Remote Model

A remote is another Morph repository reachable via a filesystem path. Both local and remote repos use the same object format and ref layout — sync copies missing objects and updates refs safely.

Phase 5 uses local-path transport only. The architecture is designed to grow into network transport later.

## 10.2 Ref Layout

- **Local branches:** `refs/heads/<branch>`
- **Remote-tracking refs:** `refs/remotes/<remote>/<branch>`
- **Remote repos:** standard `refs/heads/<branch>` layout

Fetch creates remote-tracking refs. It never overwrites local branches.

## 10.3 Object Transfer

Sync is content-addressed and minimal:
- Determines which objects are reachable from the branch tip
- Transfers only objects the destination lacks
- Preserves object hashes exactly (no rewriting)

The reachable closure from a commit includes: the commit, its tree and all entries recursively, the pipeline and its prompts/eval_suite/provenance refs, the eval contract suite, evidence_refs (runs, traces, and their transitive references), and parent commits recursively.

## 10.4 Fast-Forward Policy

- `push` updates a remote branch only if it is absent or a fast-forward
- `pull` updates a local branch only if it is a fast-forward from the fetched remote-tracking ref
- Non-fast-forward operations fail with a clear explanation

## 10.5 CLI Commands

```
morph remote add <name> <path>    # configure a named remote
morph remote list                  # list configured remotes
morph push <remote> <branch>       # push branch to remote
morph fetch <remote>               # fetch remote branches into tracking refs
morph pull <remote> <branch>       # fetch + fast-forward local branch
morph refs                         # list all refs (local + remote-tracking)
```

## 10.6 Configuration

Remotes are stored in `.morph/config.json` under the `"remotes"` key:

```json
{
  "repo_version": "0.0",
  "remotes": {
    "origin": { "path": "/path/to/remote/repo" }
  }
}
```

---

# 11. Repository Policy and CI Gating (Phase 6)

Morph supports repository-level behavioral policy for CI integration and team workflows.

## 11.1 Repository Policy

A repository-level policy is stored in `.morph/config.json` under the `"policy"` key:

```json
{
  "repo_version": "0.0",
  "policy": {
    "required_metrics": ["tests_total", "tests_passed"],
    "thresholds": { "pass_rate": 0.95 },
    "directions": { "latency": "minimize" },
    "default_eval_suite": "<eval_suite_hash or null>",
    "merge_policy": "dominance",
    "ci_defaults": { "runner": "github-actions" },
    "push_gated_branches": []
  }
}
```

Fields:
- **required_metrics**: Metric names that must be present on every commit (commit-time gate). `morph init` writes `["tests_total", "tests_passed"]` by default so commits without test results fail loudly. `morph commit --allow-empty-metrics` (or `morph_commit { allow_empty_metrics: true }`) bypasses the gate. `morph policy require-metrics <name>...` sets or clears the list (pass no names to disable).
- **thresholds**: Minimum values per metric (direction-aware).
- **directions**: Override direction per metric ("maximize" default, or "minimize").
- **default_eval_suite**: Hash of the default eval suite for certification. Set automatically by `morph eval add-case` / `morph eval suite-from-specs` unless `--no-set-default` is passed.
- **merge_policy**: `"dominance"` (default) requires behavioral dominance at merge. Setting it to `"none"` opts out of the dominance gate — useful during rapid prototyping when behavioral evidence has not caught up to the structural change. Both `morph-core::merge::execute_merge` and `morph-core::merge_flow::continue_merge` honor this setting.
- **ci_defaults**: Default CI runner metadata.
- **push_gated_branches**: Branch-name globs (`*` / `?` / literal) that must pass `gate_check` before a bare server accepts a `RefWrite` over SSH. Empty list (default) gates nothing.

## 11.2 Certification

`morph certify --metrics-file <file>` validates externally produced metrics against the configured policy:
1. Checks that all required metrics are present.
2. Checks that all thresholds are satisfied (direction-aware).
3. Records the result as a certification Annotation on the commit.

Certification does not execute evaluations. External tools (CI, test runners, human reviewers) produce the metrics; Morph validates and records them.

## 11.3 Gate

`morph gate` checks whether a commit satisfies the project's behavioral policy:
1. Verifies required metrics exist (in commit or certification).
2. Verifies thresholds are met.
3. Verifies the commit has a passing certification annotation.

Gate is read-only and suitable for CI blocking steps. Exit code 0 = pass, 1 = fail.

## 11.4 CLI Commands

```
morph policy init [--force]             # write default policy if absent (idempotent)
morph policy show                       # display current policy
morph policy set <file>                 # set policy from JSON file
morph policy set-default-eval <hash>    # set default eval suite
morph policy require-metrics <name>...  # replace required_metrics (empty list disables)
morph certify --metrics-file <file>     # certify HEAD or --commit <hash>
morph certify --metrics-file <file> --json  # JSON output for CI
morph gate                              # gate check on HEAD
morph gate --commit <hash> --json       # gate check with JSON output
```

## 11.5 CI Integration Pattern

1. Developer works locally, records Morph history.
2. Git branch is pushed for review.
3. CI runs evaluations externally.
4. CI calls `morph certify --metrics-file results.json --runner ci-v2`.
5. CI calls `morph gate` as a blocking step.
6. Team inspects `morph log` and `morph show` to review behavioral evidence.

---

# 12. Out of Scope (v0)

- Distributed run deduplication
- Cryptographic signatures
- Policy enforcement at network level
- Advanced statistical testing (two-sample equivalence, Bayesian comparison)
- Federated merge verification
- Network-based remote transport (HTTP, SSH)

---

# 13. v0 Success Criteria

Morph v0 is successful if:

- Pipelines can be versioned as first-class objects
- Runs are recorded as execution receipts with full environment
- Traces capture typed, addressable events
- Annotations can attach feedback, bookmarks, tags, and notes to any object
- Merge is behaviorally gated (dominance, not just structural)
- IDE integration works via the adapter contract
- Git users feel comfortable with the CLI
- Agent-generated patches are accountable (agent identity in runs)
- Higher-level tools (session capture, curation, workflow extraction) can be built on the object model without changes to Morph core

---

# 14. Guiding Constraint

If something feels too clever, remove it.

v0 must:

- Be coherent
- Be minimal
- Preserve Git mental models
- Satisfy the Morph axioms

Complexity can grow later.

Correct foundations cannot.

---

# 15. Implementation: Rust (v0)

The reference v0 implementation is in Rust.

- **Storage:** Trait-based `Store` interface with a single filesystem backend (`FsStore`). Three on-disk variants selected by `repo_version`: `FsStore::new` for legacy store version 0.0 (SHA-256 of canonical JSON), `FsStore::new_git` for 0.2/0.3 (Git-format hashing, flat `objects/`), and `FsStore::new_git_fanout` for 0.4 (Git-format hashing, fan-out `objects/<xx>/<rest>.json`). All variants share the same `refs/` and type-index directories for fast listing. `morph upgrade` migrates between versions. Future backends (SQLite, S3) plug into the same trait.
- **Metrics validation:** Built-in aggregation (`mean`, `min`, `p95`, `lower_ci_bound`), direction-aware threshold checks (`maximize`/`minimize`), and componentwise dominance checks. Morph does not execute tests; it validates and compares reported metric vectors.
- **Crates:**

| Crate | Role |
|---|---|
| `morph-core` | Library: object model, storage, hashing, commits, metrics, trees, migration |
| `morph-cli` | CLI: read path + manual writes (`morph init`, `add`, `commit`, `log`, ...) |
| `morph-mcp` | Cursor MCP server: primary write path from the IDE |
| `morph-serve` | Hosted service: `morph serve` (multi-repo JSON API + browser UI) and `morph visualize` (single-repo alias), with behavioral status derivation and org-level policy |
| `morph-e2e` | End-to-end Cucumber scenarios that drive the real `morph` CLI against a temp filesystem |

---

# 16. Hosted Service (Phase 7)

The hosted service (`morph serve`) exposes the Morph object graph through a stable HTTP/JSON API for collaborative team inspection.

## Starting the service

```bash
morph serve                              # serve current repo on :8765
morph serve --port 9000                  # custom port
morph serve --repo alpha=/path/to/repo   # multi-repo mode
morph serve --org-policy org-policy.json # apply org-level policy
```

## API surface

All repo-scoped endpoints live under `/api/repos/{name}/...`.

| Endpoint | Method | Returns |
|---|---|---|
| `/api/repos` | GET | List of configured repos with summary stats |
| `/api/repos/{repo}/summary` | GET | Repo summary: head, branches, commit/run counts |
| `/api/repos/{repo}/branches` | GET | Branch listing with current branch |
| `/api/repos/{repo}/commits` | GET | Commit history from HEAD with behavioral badges |
| `/api/repos/{repo}/commits/{hash}` | GET | Full commit detail with behavioral status |
| `/api/repos/{repo}/runs` | GET | Run listing |
| `/api/repos/{repo}/runs/{hash}` | GET | Run detail with agent, environment, metrics |
| `/api/repos/{repo}/traces/{hash}` | GET | Trace events (prompt/response text) |
| `/api/repos/{repo}/pipelines/{hash}` | GET | Pipeline graph, provenance, attribution |
| `/api/repos/{repo}/objects/{hash}` | GET | Raw object JSON |
| `/api/repos/{repo}/annotations/{hash}` | GET | Annotations on a target |
| `/api/repos/{repo}/policy` | GET | Effective policy (repo + org merged) |
| `/api/repos/{repo}/gate/{hash}` | GET | Gate check result for a commit |
| `/api/org/policy` | GET/POST | Organization-level policy |

Backward-compatible endpoints (`/api/log`, `/api/runs`, `/api/object/{hash}`, `/api/graph`) route to the default repo.

## Behavioral status

The commit detail endpoint returns a `behavioral_status` object:

- `certified`: whether the commit has a passing certification annotation
- `certification`: details (runner, eval_suite, metrics, failures)
- `gate_passed`: whether the commit satisfies the repo policy
- `gate_reasons`: list of reasons for gate failure
- `is_merge`: true if the commit has 2+ parents
- `merge_status`: parent metrics and dominance results for merge commits

## Organization-level policy

An optional org-level policy file can set default required_metrics, thresholds, and named presets. The effective policy for each repo is the union of org and repo policies (repo overrides win for thresholds).

---

# 17. Behavioral Merge & Server-Readiness (Phase 8)

Phase 5 introduced local-path remotes. Phase 8 extends Morph from "syncs to a directory" to "ready to host shared bare repositories driven over SSH, with server-enforced policy." It is the union of PRs 3, 4, 5, and 6 of the multi-machine roadmap and is the smallest set of changes that lets two engineers on two laptops collaborate the way Git users expect.

For user-facing walkthroughs see [MULTI-MACHINE.md](MULTI-MACHINE.md) and [SERVER-SETUP.md](SERVER-SETUP.md). For the merge engine internals see [MERGE.md](MERGE.md). This section is the schema-level reference.

## 17.1 Behavioral 3-way Merge (PRs 3–4)

Merge is structural and behavioral, not textual. Given two divergent commits and their LCA from `merge_base`, Morph runs:

- `objmerge::merge_eval_suites(base, ours, theirs)` — case + metric reconciliation.
- `pipemerge::merge_pipelines(base, ours, theirs)` — node + edge reconciliation, prompt set union, provenance.
- `treemerge::merge_trees(base, ours, theirs)` — file tree reconciliation. Disjoint changes compose. Same-path edits fall back to `git merge-file` for textual diff3, write conflict markers into the workdir, and record an `UnmergedEntry { base_blob, ours_blob, theirs_blob }` in the staging index.
- `check_dominance(parent_a, parent_b, candidate, retired)` — the merged commit must be at least as good as both parents on every non-retired metric.

The orchestrator lives in `morph-core/src/merge_flow.rs` and exposes `start_merge`, `continue_merge`, `abort_merge`, `resolve_node`, plus a `MergeProgress` view used by `morph status`. In-progress state is recorded under `.morph/MERGE_HEAD` (the "their" tip), `.morph/ORIG_HEAD` (used by `--abort` to rewind), `.morph/MERGE_MSG` (proposed message), and — when the merge produced one — `.morph/MERGE_PIPELINE.json` and `.morph/MERGE_SUITE`. See [`MERGE.md`](MERGE.md) for the full state-machine walkthrough.

## 17.2 Identity Fields on Commits (PR 6 stage A–B)

Every commit now carries two identity fields:

| Field | Type | Resolved from |
|---|---|---|
| `author` | `String` | `--author` flag → `MORPH_AUTHOR_NAME` / `MORPH_AUTHOR_EMAIL` env → `morph config user.name` / `user.email` → `"morph"` default |
| `morph_instance` | `Option<String>` | `agent.instance_id` in `.morph/config.json`, generated at `morph init` time as `morph-<6-hex>` |

`author` is the human; `morph_instance` is the machine. Two laptops belonging to the same person produce different commit hashes for identical content because their `morph_instance` differs. Both fields are `serde(default)` and round-trip through legacy commits unchanged.

CLI:

```
morph config user.name <value>
morph config user.email <value>
morph config <key>           # get
morph config --get <key>     # explicit get
```

## 17.3 Evidence Union on Merge (PR 6 stage C)

The merge commit's `evidence_refs` is the deduped, sorted union of `parent_a.evidence_refs` and `parent_b.evidence_refs` (`merge::union_evidence_refs`). Both parents stay reachable through the merge — `morph log` and remote fetches that traverse `evidence_refs` continue to work after a merge.

If both parents have no evidence the field stays `None` (rather than `Some(vec![])`) so empty cases serialize identically pre- and post-PR6.

## 17.4 Bare Repositories (PR 6 stage D)

A **bare** repo is the server-side layout: no working tree, no `.morph/` wrapper. `objects/`, `refs/`, `config.json`, etc. live directly at the repo root.

```
morph init --bare /srv/repos/myproject.morph
```

`config.json` carries `"bare": true` and the same `agent.instance_id` a working repo gets. Helpers:

- `morph_core::init_bare(root)` — create a bare layout (no `.gitignore`).
- `morph_core::is_bare(morph_dir)` — read the bare flag.
- `morph_core::resolve_morph_dir(path)` — auto-detect `path/.morph` (working) vs `path/` (bare); used by `morph remote-helper` so it accepts both.

Bare repos are the only kind that should accept pushes from multiple clients; pushing into a working repo would race with whatever is editing its working tree.

## 17.5 SSH Transport (PR 5)

Network transport is JSON-RPC over an SSH session driven by a hidden subcommand. There is no daemon, no port beyond SSH.

- `morph remote-helper --repo-root <abs-path>` — server side. Reads one JSON request per line on stdin, writes one JSON response per line on stdout. Exits 0 on EOF.
- `morph_core::ssh_proto` — wire types (`Request`, `Response`, `OkResponse`, `ErrResponse`, `ErrorKind`).
- `morph_core::ssh_store::SshStore` — client-side `Store` implementation.
- `morph_core::ssh_store::SshUrl` — accepts `ssh://user@host[:port]/path` and `user@host:path`.
- `morph_core::ssh_store::RemoteSpawn` — spawns SSH; honors `MORPH_SSH` (override binary) and `MORPH_REMOTE_BIN` (server-side `morph` path).

`morph remote add <name> <path-or-url>` stores SSH URLs verbatim and resolves plain paths to absolute. `morph push|fetch|pull` route through `open_remote_store` which dispatches to `SshStore` or `FsStore` based on URL shape.

## 17.6 Branch Upstreams and `morph sync` (PR 5 stage G)

Per-branch upstream tracking lives in `.morph/config.json` under `"branches"`:

```json
{
  "branches": {
    "main":    { "upstream": { "remote": "origin", "branch": "main" } },
    "feature": { "upstream": { "remote": "origin", "branch": "feature" } }
  }
}
```

CLI:

```
morph branch --set-upstream <remote>/<branch>   # configure
morph sync [branch]                              # fetch + pull --merge against upstream
```

`morph sync` defaults to the current branch when no argument is supplied. It is fetch + fast-forward when possible, fetch + 3-way merge when the branch has diverged.

## 17.7 Schema Handshake (PR 6 stage E)

Every SSH session begins with a `Hello` exchange. The server's `Hello` response now carries:

```json
{
  "version": "X.Y.Z",
  "protocol_version": 1,
  "repo_version": "0.5"
}
```

- `MORPH_PROTOCOL_VERSION` is a single integer constant in `morph_core::ssh_proto`. Bump it whenever the wire format changes incompatibly.
- The client validates the field via `validate_hello`. On mismatch it raises `MorphError::IncompatibleRemote { remote, local, reason }`.
- Legacy helpers that don't include `protocol_version` are accepted silently — exactly one release of overlap, then mismatch becomes a hard error.

## 17.8 Server-Side Closure Validation (PR 6 stage F)

`morph_core::verify_closure(store, tip)` walks the reachable graph from a tip hash and verifies every object is present. Wired into the `RefWrite` handler of `morph remote-helper`: a push that doesn't carry its full closure is rejected before the ref moves. This makes partial pushes impossible — either every dependency lands and the ref updates, or nothing changes.

## 17.9 Server-Side Push Gating (PR 6 stage F)

`RepoPolicy.push_gated_branches: Vec<String>` lists branch-name patterns the server will run `gate_check` against on every `RefWrite`. The flow:

1. Receive `RefWrite { name, hash }`.
2. `verify_closure(store, &hash)` — reject if incomplete.
3. If `branch_from_ref(name)` matches any pattern in `policy.push_gated_branches` (via `branch_matches_any`), call `enforce_push_gate(store, morph_dir, name, &hash)`.
4. `enforce_push_gate` runs `gate_check` against the policy; on failure returns `MorphError::Serialization("push gate failed for branch '<name>': <reasons>")`.
5. Only on success is the ref written.

Empty `push_gated_branches` (the default) reproduces pre-PR6 behavior — no server-side enforcement.

### Pattern grammar (PR 9)

Each entry is a glob:

- `*` — zero or more non-`/` characters
- `?` — exactly one non-`/` character
- anything else literal

`release/*` matches `release/v1.0` but **not** `release/v1/hotfix` — `*` is bounded by the next `/`, which mirrors Git refspec semantics. Patterns without metacharacters keep the pre-PR9 exact-match meaning, so existing configs upgrade unchanged.

The matcher is `morph_core::branch_matches_pattern(branch, pattern)` and the membership wrapper is `branch_matches_any(branch, &patterns)`. Both are pure, allocation-free, and tested in `morph-core/src/policy.rs`.

## 17.10 `morph clone` (PR 8)

`morph_core::clone_repo(remote_url, destination, opts) -> CloneOutcome` packages the multi-machine onboarding flow into a single command. Behavior:

1. Refuse to clone into a non-empty `destination` (preserves any user-authored files).
2. `init_repo` (or `init_bare` when `opts.bare`).
3. Configure `origin = remote_url`.
4. `fetch_remote(local, remote, "origin")` — pulls every branch into `refs/remotes/origin/*`.
5. Choose a default branch:
   - explicit `opts.branch`, then
   - the remote's `HEAD` (filesystem remotes via `ref_read_raw("HEAD")`; SSH remotes don't expose HEAD on the v0 wire), then
   - `"main"`.
6. Write `refs/heads/<branch>` and set HEAD to point at it.
7. `set_branch_upstream("<branch>", { remote: "origin", branch: "<branch>" })` so `morph sync` works immediately.
8. For working clones, `restore_tree` into the destination; bare clones skip this.

CLI:

```
morph clone <url-or-path> [destination] [--branch <name>] [--bare]
```

Default destination is the basename of `url` with any trailing `.morph` stripped (matches `git clone`'s heuristic).

## 17.11 Error Variants

| Variant | Where it comes from |
|---|---|
| `Diverged { local, remote }` | `pull` without `--merge` against a divergent branch |
| `IncompatibleRemote { remote, local, reason }` | `Hello` handshake mismatch |
| `NotFound(String)` | `verify_closure` saw a missing object |
| `Serialization("push gate failed for branch '...': ...")` | `enforce_push_gate` rejected a push |
| `RepoTooOld(...)` / `RepoTooNew(...)` / `UpgradeRequired(...)` | Store-version compatibility checks (Phase 5+) |

## 17.12 Module Map

| File | Phase 8 role |
|---|---|
| `morph-core/src/merge.rs` | LCA, prepare/execute_merge, dominance, evidence union |
| `morph-core/src/merge_flow.rs` | start/continue/abort orchestrator, `MergeProgress` |
| `morph-core/src/treemerge.rs` | 3-way tree merge, `WorkdirOp`, textual fallback |
| `morph-core/src/pipemerge.rs` | Pipeline DAG merge |
| `morph-core/src/objmerge.rs` | EvalSuite case/metric merge |
| `morph-core/src/index.rs` | `unmerged_entries`, `UnmergedEntry` |
| `morph-core/src/workdir.rs` | `working_tree_clean`, restore |
| `morph-core/src/repo.rs` | `init_repo`, `init_bare`, `is_bare`, `resolve_morph_dir` |
| `morph-core/src/agent.rs` | `agent.instance_id` generation/read/write |
| `morph-core/src/author.rs` | `user.name` / `user.email` resolution |
| `morph-core/src/sync.rs` | `RemoteSpec`, `read_remotes`, `open_remote_store`, `verify_closure`, branch upstream config, `clone_repo` |
| `morph-core/src/ssh_proto.rs` | wire types, `MORPH_PROTOCOL_VERSION`, error mapping |
| `morph-core/src/ssh_store.rs` | `SshStore`, `SshUrl`, `RemoteSpawn`/`LocalSpawn`, `validate_hello` |
| `morph-core/src/policy.rs` | `RepoPolicy.push_gated_branches`, `enforce_push_gate`, `branch_from_ref`, `branch_matches_pattern`, `branch_matches_any` |
| `morph-cli/src/remote_helper.rs` | server-side JSON-RPC dispatch wired to closure + push-gate checks |

