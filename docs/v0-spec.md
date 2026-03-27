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
- Remote protocol (content-addressed store is designed for future distribution)

Those can come later.

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

The v0 implementation provides two backends, selected by `repo_version` in `.morph/config.json`:

| Store version | Backend | Hash function | Notes |
|---|---|---|---|
| `"0.0"` | **FsStore** — flat `objects/<hash>.json` | SHA-256 of canonical JSON | Created by `morph init`. Legacy. |
| `"0.2"` | **FsStore (Git-format)** — same directory layout | SHA-256 of `"blob " + len + "\0" + canonical_json` (Git object format) | Created by `morph upgrade` from 0.0. |
| `"0.3"` | **FsStore (Git-format)** + tree commits | Same as 0.2 | Adds file tree storage in commits. Current. |

Both backends use the same directory layout: `objects/<hash>.json` for objects, `refs/` for references, and type-index directories (`runs/`, `traces/`, `prompts/`, `evals/`) for fast type-filtered listing. The `list(type)` operation uses type-index directories when available, falling back to full object scan for types without indexes.

Migration between store versions is handled by `morph upgrade` (CLI only). Migration from 0.0 to 0.2 rewrites all objects with new hashes and updates all internal references.

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
morph init
morph status
morph log
```

## 6.2 Prompt Operations

```
morph prompt create <file>
morph prompt materialize <hash> [--output <path>]
morph prompt latest [<ref>]
```

Prompts are canonical objects. Materialization writes them to the working directory (or `.morph/prompts/`) for review.

`morph prompt latest` prints the prompt text from a Run. Ref follows a Git-like syntax: `latest` (default, most recent run), `latest~N` or `latest-N` (Nth run back), or a 64-char run hash.

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
```

`morph add .` stages files and updates the staging index. `morph commit -m "message"` builds the tree from the index, creates the commit, and clears the index.

`--pipeline` and `--eval-suite` are optional flags. When omitted, the commit behaves as a plain VCS commit (identity pipeline, empty eval suite). When specified, `morph commit` validates:

- Pipeline graph integrity (DAG, valid node/edge kinds)
- Eval suite presence and hash integrity
- Uses **recorded** observed metrics (from external evaluation or prior `morph eval record`) to form the eval contract

`--from-run <run_hash>` derives commit provenance from a recorded Run:

- `evidence_refs`: the run hash and its trace hash
- `env_constraints`: the Run's environment (model, version, parameters, toolchain)
- `contributors`: the run's agent (with role "primary") and any additional contributors

If the run hash points to a missing object, a non-Run object, or a Run whose trace cannot be resolved, commit creation fails with a clear error. When `--from-run` is omitted, provenance fields are absent (plain VCS commit).

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

```
morph eval record <file>
```

**Ingests** evaluation results (metrics against an EvalSuite). Does not run tests. External tools run the eval and report scores. Morph validates aggregation and thresholds and records the metrics for use in commits and merge dominance checks.

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

```
morph merge <branch> -m <message> --pipeline <hash> --metrics '<json>'
morph merge <branch> -m <message> --pipeline <hash> --eval-suite <hash> --metrics '<json>'
morph merge <branch> -m <message> --pipeline <hash> --metrics '<json>' --retire 'old_metric1,old_metric2'
```

`--pipeline` and `--metrics` are required. `--eval-suite` is optional: when omitted, the union of both parents' evaluation suites is computed automatically. When provided, the explicit suite is used.

Merge procedure:

1. Resolve both parent commits (current HEAD and target branch)
2. Combine evaluation suites: if `--eval-suite` is provided, use it; otherwise compute T = T1 ⊎ T2 (union by metric ID)
3. Apply metric retirement (if `--retire` is specified): remove retired metrics from the union suite. Retirement is explicit in the merge plan; per paper §5.3 the merged pipeline should include a `review` node for attribution (not enforced by the CLI in v0—see §6.8 notes below)
4. Record the bar: embed each parent's scores into V_T and record the best from either parent on every surviving metric
5. Validate **dominance**: merged pipeline's observed metrics must meet or exceed both parents' `observed_metrics` on every surviving metric (direction-aware). Only metrics in the (post-retirement) union suite are checked.
6. Create merge commit if satisfied

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
```

`morph hash-object` reads a Morph object from a JSON file, stores it, and prints its content hash. Used by hook scripts that need to construct and store objects outside the normal CLI workflow.

`morph upgrade` migrates the repository store to the latest version (e.g. 0.0 → 0.2 → 0.3). Required before using MCP on older repos.

`morph visualize` starts a local web server for browsing the repo in a browser: commit strip, detail panel (message, author, pipeline, eval contract, prompts), and an object browser. The web UI is embedded in the binary.

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
    "required_metrics": ["tests_passed", "pass_rate"],
    "thresholds": { "pass_rate": 0.95 },
    "directions": { "latency": "minimize" },
    "default_eval_suite": "<eval_suite_hash or null>",
    "merge_policy": "dominance",
    "ci_defaults": { "runner": "github-actions" }
  }
}
```

Fields:
- **required_metrics**: Metric names that must be present for certification.
- **thresholds**: Minimum values per metric (direction-aware).
- **directions**: Override direction per metric ("maximize" default, or "minimize").
- **default_eval_suite**: Hash of the default eval suite for certification.
- **merge_policy**: "dominance" (default) requires behavioral dominance at merge.
- **ci_defaults**: Default CI runner metadata.

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
morph policy show                       # display current policy
morph policy set <file>                 # set policy from JSON file
morph policy set-default-eval <hash>    # set default eval suite
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

# 12. v0 Success Criteria

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

# 13. Guiding Constraint

If something feels too clever, remove it.

v0 must:

- Be coherent
- Be minimal
- Preserve Git mental models
- Satisfy the Morph axioms

Complexity can grow later.

Correct foundations cannot.

---

# 14. Implementation: Rust (v0)

The reference v0 implementation is in Rust.

- **Storage:** Trait-based `Store` interface with a single filesystem backend (`FsStore`) supporting two hash modes: `FsStore::new` for legacy store version 0.0, `FsStore::new_git` for 0.2+. Both use flat JSON files in `.morph/objects/<hash>.json` with type-index directories for fast listing. `morph upgrade` migrates between versions. Future backends (SQLite, S3) plug into the same trait.
- **Metrics validation:** Built-in aggregation (`mean`, `min`, `p95`, `lower_ci_bound`), direction-aware threshold checks (`maximize`/`minimize`), and componentwise dominance checks. Morph does not execute tests; it validates and compares reported metric vectors.
- **Crates:**

| Crate | Role |
|---|---|
| `morph-core` | Library: object model, storage, hashing, commits, metrics, trees, migration |
| `morph-cli` | CLI: read path + manual writes (`morph init`, `add`, `commit`, `log`, ...) |
| `morph-mcp` | Cursor MCP server: primary write path from the IDE |
| `morph-serve` | Hosted service: shared inspection and policy layer (`morph serve` and `morph visualize`) with stable JSON API, multi-repo support, behavioral status derivation, org-level policy |

---

# 15. Hosted Service (Phase 7)

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
