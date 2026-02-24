# Morph Theory

**The Algebraic Foundations of a Distributed Version Control System for Transformation Programs**

---

## 1. Purpose of This Document

This document defines the mathematical model of Morph.

It explains:

- What Morph versions
- How Morph generalizes Git
- How effectful, probabilistic transformations can be version-controlled
- The algebra underlying programs, commits, runs, evaluation, and merges
- The axioms required for the system to be coherent

This is not an implementation document.
See `v0-spec.md` for concrete system design.

---

## 2. The Problem Morph Solves

Git versions deterministic source code by tracking file trees.

Modern AI-centric development workflows do not behave like deterministic text editing. They often:

- Transform structured document state (codebases, datasets, trees, artifacts)
- Use effectful operators (LLM calls, retrieval, tools, compilers, test runners)
- Produce probabilistic outputs
- Depend on model versions and runtime environments
- Are authored by humans and agents together

Git assumes:

- Identity is byte equality
- Reproducibility means identical output bytes
- Merge is syntactic reconciliation

In transformation-heavy systems, those assumptions break.

Morph generalizes version control to:

> Effectful, probabilistic transformations over structured state, with explicit evaluation-defined behavioral claims.

---

## 3. Core Intuition

In Git:

- Files are versioned.
- A commit freezes a file tree snapshot.

In Morph:

- Programs are versioned.
- A commit freezes a program plus a behavioral contract it is certified to satisfy.

A Morph commit represents:

> A stabilized transformation program together with a declared evaluation contract and a certificate of achieved behavior, backed by immutable execution evidence.

This is the key shift:

- **Git** tracks *what the files are*.
- **Morph** tracks *what the transformation does*, under declared evaluation and environment constraints.

---

## 4. Objects and Identity

Morph is built on content-addressed immutable objects, like Git, but extended to include execution evidence.

### 4.1 Immutable Object Graph

Morph's core objects are content-addressed and immutable:

| Object | Description |
|---|---|
| **Blob** | Raw content (prompts, patches, config, binaries, datasets, etc.) |
| **Tree** | Structured document tree (like a filesystem tree) |
| **Program** | A transformation graph (DAG) of operators |
| **Run** | One execution instance of a Program |
| **Trace** | Detailed execution record (often a DAG of events) |
| **EvalSuite** | An evaluation contract (metrics + certification rule + fixtures) |
| **Commit** | A certified behavioral identity (Program + contract + certificate + evidence refs) |
| **Annotation** | Immutable metadata attached to any object by hash reference |

Everything can be distributed and verified by hash. No central authority is required.

---

## 5. State and Environment

Morph is about programs transforming state under an environment.

### 5.1 State

We model workspace state as a structured product:

\[
S = D \times C \times M
\]

Where:

- **D** — Document tree (code, inputs, datasets, retrieved docs, produced artifacts)
- **C** — Execution context (scratchpad, intermediate results, caches)
- **M** — Metadata (provenance refs, run IDs, trace refs, etc.)

Engineer mental model: `S` is a typed "workspace" struct:

```
S = { docs: Tree, ctx: Context, meta: Metadata }
```

### 5.2 Environment

Programs depend on an environment:

\[
E = (\text{runner/toolchain, model id/version, decoding params, policies, \ldots})
\]

Examples of what must be representable in `E`:

- A container image digest / VM image / Nix derivation
- Toolchain versions (compiler, interpreter, package lockfiles)
- Model identifier and version (when available)
- Decoding parameters, safety/policy refs
- Credentials references (never raw secrets), endpoints, and tool adapters

Environment is recorded because reproducibility is defined relative to it.

---

## 6. Programs as Effectful Transformations

A Morph program is a workflow that transforms state.

Crucially:

> A Morph program is not "prompt-only."
> It is a transformation DAG whose nodes may include prompts, tool calls, deterministic transforms, and human edits (modeled as patches).

### 6.1 Program Type

For a fixed environment `E`, a program has type:

\[
P_E : A \to F(B)
\]

Most commonly `A = B = S`, i.e. state-to-state:

\[
P_E : S \to F(S)
\]

Where `F` represents effects such as:

- Randomness / sampling
- IO / tool calls
- Logging and traces
- Failures and retries
- Human-in-the-loop steps (as recorded inputs)

Engineer mental model: `F[T]` is a "box" that carries side effects and receipts along with a value of type `T`.

### 6.2 Operator Families (Prompt + Non-Prompt + Human)

A program DAG is built from operators. Examples:

| Operator | Description |
|---|---|
| **Prompt operator** | Calls an LLM (probabilistic) |
| **Retrieval operator** | Queries an index / web / DB (effectful) |
| **Tool operator** | Runs external commands, compilers, test runners (effectful) |
| **Pure transform** | Deterministic rewrite, formatting, AST transform (pure) |
| **Patch apply** | Deterministic application of a recorded diff/patch (pure) |
| **Selection operator** | Choose among candidates (may be stochastic or policy-driven) |

Human edits fit naturally:

1. The human produces a patch blob.
2. The program includes a node `apply_patch(patch_blob)`.

That makes "manual coding" first-class, reproducible, and reviewable.

---

## 7. Sequential Composition Requires a Monad

If programs return results "in a box" \( F(\cdot) \), ordinary function composition doesn't type-check.

To compose effectful computations, we require `F` to be a monad.

### 7.1 Monad Interface

A monad provides:

```
pure : A -> F[A]
bind : F[A] -> (A -> F[B]) -> F[B]    // aka flatMap
```

Sequential composition of programs is defined by `bind`:

Given:

- \( P : A \to F(B) \)
- \( Q : B \to F(C) \)

Define:

\[
(Q \circ P)(a) = \mathrm{bind}(P(a),\; Q)
\]

### 7.2 Why We Need Monad Laws

The standard monad laws imply:

- **Associativity**: pipelines compose without parentheses changing meaning
- **Identity**: there is a no-op program (do nothing)

This is what makes "program chaining" algebraically stable, and it is the mathematical reason DAG scheduling and refactoring preserve meaning.

---

## 8. Parallel Composition Requires Product State + Zip for Effects

Morph must support:

- Parallel prompt branches
- Agent swarms
- Independent experiments
- Concurrent evaluation

Parallelism is not "two threads racing on the same memory." Morph models parallelism as:

> Fork state into independent components, run computations, then explicitly join.

To define this correctly we need two things:

1. Product of state spaces
2. A lawful way to combine effects

### 8.1 Product of State Spaces

Assume state spaces support products:

\[
A \times B
\]

and a unit object \( 1 \). This is "tuple state."

### 8.2 Zipping Effects

To run two effectful computations independently and combine results, require a natural operation:

\[
\mathrm{zip}_{A,B} : F(A) \times F(B) \to F(A \times B)
\]

Engineer equivalents:

- `Promise.all`
- "Product distribution" (independent sampling)
- "Pair outputs and concatenate traces"

This structure is satisfied by many practical effect models, including:

- Distributions (with independence)
- Traced IO (with trace combination)
- Applicative effects

It is the mathematical hook that turns parallel DAG execution into a lawful algebra rather than an implementation accident.

### 8.3 Parallel Composition of Programs

Given:

- \( P : A \to F(B) \)
- \( Q : C \to F(D) \)

Define:

\[
P \otimes Q : (A \times C) \to F(B \times D)
\]

by:

\[
(P \otimes Q)(a, c) = \mathrm{zip}(P(a),\; Q(c))
\]

**Same-input branching** (common case):

If \( P, Q : S \to F(S) \) both consume the same input state, use the diagonal:

\[
\Delta(s) = (s, s)
\]

Then:

\[
\mathrm{branch}(P, Q) = (P \otimes Q) \circ \Delta : S \to F(S \times S)
\]

A later explicit join step can reconcile the two results:

\[
J : (S \times S) \to F(S)
\]

This mirrors real systems: branches generate candidates; a join step selects/merges them.

### 8.4 Multi-Agent Programs and Attribution

When multiple agents contribute to a single program, the program DAG naturally records what each agent did. We formalize this with an attribution function.

**Attributed Program.** An attributed program extends a program with an attribution function:

\[
\alpha : V \to \mathcal{A} \cup \{\bot\}
\]

where \( \mathcal{A} \) is a set of agent identifiers and \( \bot \) denotes unattributed nodes. For a single-agent or human-authored program, \( \alpha \) is constant.

Attribution composes naturally:

- **Sequential:** For \( Q \circ P \), the attribution of the composite is the union of both attribution functions over disjoint node sets.
- **Parallel:** For \( P \otimes Q \), similarly. The join step \( J \) may be attributed to a coordinating agent or left unattributed.

**Certification is holistic.** The certificate vector \( \mathrm{cert}_T(P, E, s_0) \in V_T \) is a property of the composed program as a whole. There is no natural decomposition:

\[
\mathrm{cert}_T(P, E, s_0) = \bigoplus_{a \in \mathcal{A}} \mathrm{cert}_T(P|_a, E, s_0)
\]

because the behavioral contribution of agent \( a \)'s nodes generally depends on the context provided by other agents' nodes. The evaluation suite tests the joint result.

This means Morph can:

- Certify that a multi-agent program meets its behavioral contract
- Record which agent contributed which operators (via \( \alpha \))

But it cannot attribute credit or blame to individual agents from the certificate alone. Decomposing joint performance into per-agent contributions requires additional machinery (counterfactual evaluation, Shapley values, modular evaluation suites).

**Distributed vs. cooperative multi-agent work.** The framework distinguishes two patterns:

| Pattern | Description | Theory treatment |
|---|---|---|
| **Distributed** | Agents on separate branches, each independently certified | Merge-as-dominance (§13) ensures no regression. Identical to two humans on two branches. |
| **Cooperative** | Multiple agents contribute to a single program | \( P \otimes Q \) with join models orchestrated parallelism. Attribution records provenance; certification evaluates the composed result. |

Real-time concurrent edits to shared state (CRDT-style coordination) live below the VCS layer. Agents coordinate however they coordinate, produce a result, and that result is committed and certified by Morph.

---

## 9. Runs, Traces, and Artifacts

A Run is one execution instance of a program under a specific environment and initial state.

### 9.1 Run Definition

Given a program `P`, environment `E`, and initial state \( s_0 \):

\[
\mathrm{Run}(P, E, s_0)
\]

Conceptually, if \( P_E : S \to F(S) \), a run is a realized outcome (a "sample") including receipts.

### 9.2 Trace

A run produces a **Trace**:

- Tool calls
- Model calls
- Intermediate states
- Timing/costs
- Branching structure (often a DAG)

A trace is immutable and addressable by event IDs, enabling fine-grained annotation.

### 9.3 Artifacts

A run's resulting state includes a document tree `D`. That tree is the produced artifact set:

- Generated code
- Modified tests
- Binaries
- Reports
- Datasets
- Evaluation logs

Artifacts are content-addressed trees/blobs, so you can diff, store, and distribute them — just like Git trees — but they come from executing a program.

This cleanly separates:

- **Program identity** (the transformer)
- **Artifact identity** (the produced outputs)

---

## 10. Evaluation: Programs Are Certified by Contracts

### 10.1 Evaluation is an Effectful Computation Too

Running tests, compiling code, judging outputs with another model, or collecting human ratings are all effectful processes.

So we model evaluation as its own effectful program.

An evaluation suite `T` defines an evaluator:

\[
\mathrm{Eval}_{T,E} : S \to F(\mathrm{Obs}_T)
\]

Where \( \mathrm{Obs}_T \) includes:

- Raw metric samples
- Logs from test execution
- Failures, stack traces
- Timing/cost data
- Anything needed for certification and audit

Engineer mental model: `EvalSuite` is a reproducible test harness definition, not just a list of assertions.

### 10.2 Two Kinds of Evaluation (Both First-Class)

Morph supports both:

**A) Artifact Evaluation** (most common)

Evaluate the produced code/artifacts by building/running/tests:

- Unit tests
- Integration tests
- E2E tests
- Static analysis
- Fuzzing
- Performance benchmarks

This is simply evaluation programs that operate on the produced document tree `D` and runner environment `E`.

**B) Program/Process Evaluation** (optional but powerful)

Evaluate the behavior of the transformation process itself:

- Cost
- Latency
- Tool usage constraints
- Trace properties ("must not call external internet", "must cite sources")
- Policy compliance of prompts and tool calls
- Determinism / variance bounds

Both are meaningful for merges. For example, you might require:

- Artifact correctness (tests pass)
- Plus process bounds (cost under $X, no forbidden tools)

### 10.3 Where Tests Come From (Prevents "Cheating")

A key subtlety:

> If the program is allowed to modify the tests that are used to evaluate it, it can make evaluation meaningless.

Morph makes test/fixture provenance explicit in the suite definition.

An `EvalSuite` must declare a **fixture source** for each component:

| Source | Meaning |
|---|---|
| **candidate-sourced** | Use tests/data from the produced tree `D` |
| **base-sourced** | Use tests/data from a specified base tree |
| **pinned** | Use tests/data referenced immutably by hash |
| **external** | Use tests/data from an external source referenced by immutable descriptor |

This supports real workflows:

- "Run the repo's own tests" → candidate-sourced
- "Run a central conformance suite the PR cannot edit" → pinned/external
- "Run e2e tests maintained by a separate team" → pinned/external

E2E tests fit naturally: they're just evaluators that execute a system-level harness under the runner environment.

### 10.4 Metrics, Direction, and Certification

Each metric `m` in suite `T` defines:

- A scoring function from observations: \( \mathrm{score}_m(\mathrm{Obs}_T) \)
- An **ordering direction** (`maximize` or `minimize`), formalized as an order \( \leq_m \). For `maximize` metrics (e.g. accuracy), higher is better: \( x \leq_m y \iff x \leq y \). For `minimize` metrics (e.g. latency), lower is better: \( x \leq_m y \iff x \geq y \).
- An **aggregation method** (how to reduce per-case scores into a single value)
- A **threshold** (the minimum certified claim for the metric under its direction)

The v0 implementation supports four built-in aggregation methods: `mean`, `min`, `p95`, and `lower_ci_bound`. The direction field defaults to `maximize` when omitted (backward-compatible with pre-direction suites).

Examples of certification rules (future versions may add more):

- Lower confidence bounds
- Quantile guarantees
- Expectation with variance bounds
- Deterministic pass/fail for unit tests

Morph does not force one statistical method, but the suite must specify it so certificates are meaningful.

---

## 11. Contracts, Certificates, and Behavioral Order

### 11.1 Contract Satisfaction

Write:

\[
P \models_{E, s_0} T
\]

to mean: running `P` (under stated environment constraints and initial state distribution) produces observations that satisfy `T`'s certification rules.

In practice:

1. Execute evaluation runs (possibly multiple samples)
2. Compute metric samples
3. Apply certification rule
4. Store evidence and certificate

### 11.2 Certificate Vector

For a suite `T`, define a certificate space:

\[
V_T = \prod_{m \in T} V_m
\]

with componentwise ordering:

\[
x \leq_T y \iff \forall m,\; x_m \leq_m y_m
\]

A certification procedure yields:

\[
\mathrm{cert}_T(P, E, s_0) \in V_T
\]

Certificates are what commits store.

### 11.3 Behavioral Preorder (Dominance)

Define dominance:

\[
P \preceq_{E, s_0, T} Q \quad\text{iff}\quad \mathrm{cert}_T(P, E, s_0) \leq_T \mathrm{cert}_T(Q, E, s_0)
\]

This is a preorder (reflexive, transitive). It matches engineering reality: you compare certified claims, not unknowable "true" behaviors.

---

## 12. Commits as Behavioral Identities

A Morph commit freezes:

- A **File tree hash** — the root hash of the working directory tree at commit time (same role as Git's tree object in a commit)
- A **Program ID** (hash of the program DAG + referenced blobs/trees)
- An **EvalSuite** (contract ID)
- A **certificate vector** \( v \in V_T \) (the `observed_metrics`)
- **Parent commit hashes** (forming the Merkle DAG)
- **Author and timestamp**

Conceptually:

\[
\mathrm{Commit} = (\mathrm{tree\_hash},\; \mathrm{program\_id},\; T,\; v,\; \mathrm{parents},\; \mathrm{metadata})
\]

A Morph commit is both a file snapshot (like Git) AND a behavioral certificate. The program and eval contract default to the identity program and empty suite when unspecified, making Morph usable as a plain VCS.

Commits are claims. Runs are receipts. History is immutable and verifiable.

---

## 13. Merge as Join of Behavioral Requirements

Git merge reconciles text. Morph merge reconciles behavioral requirements.

### 13.1 Union of Contracts

If parents have suites \( T_1 \) and \( T_2 \), the merge suite is:

\[
T = T_1 \uplus T_2
\]

(disjoint union by metric ID, not by name)

This ensures metric definitions cannot silently collide.

### 13.2 Join of Certificates

Embed each parent certificate into \( V_T \), then define:

\[
v_{\mathrm{req}} = \mathrm{embed}(v_1) \sqcup \mathrm{embed}(v_2)
\]

where \( \sqcup \) is the componentwise least upper bound (e.g., max under each metric's order).

This is where the word *join* is mathematically correct: it lives in certificate space.

### 13.3 Merge Validity

A merge candidate program `R` is valid if it can be certified to dominate the joined requirement:

\[
\mathrm{cert}_T(R, E, s_0) \geq_T v_{\mathrm{req}}
\]

If no such `R` exists (or can't be found/certified), merge fails.

---

## 14. Working Space vs Commit Space

Morph supports exploration without rewriting receipts.

### 14.1 Working Space

Working space contains evolving material:

- Prompts as files/blobs
- Patches
- Partial program DAGs ("work graphs")
- Intermediate traces and runs
- Experimental branches

A working space can be:

- A set of prompts (no edges)
- A DAG of prompts/tools/transforms
- A hybrid of manual edits + agentic steps

### 14.2 Commit Space

Commit space contains stabilized objects:

- Content-addressed programs
- Evaluation suites
- Certified commits
- Immutable evidence graphs

Work can be "rolled up" by creating new commits that reference prior objects; nothing is rewritten.

---

## 15. `morph run` and `morph eval` (Theory-Level Semantics)

This section is intentionally multi-language and binary-friendly.

### 15.1 Runner Abstraction

Define a **Runner** as part of environment `E`. A runner provides the operational ability to execute program operators, such as:

- Run a prompt call via a model adapter
- Run a compiler
- Run a test harness
- Run arbitrary commands
- Load binaries and execute them
- Access configured tools/services

The runner is recorded by immutable identifiers as much as possible (container digest, Nix store path hash, Bazel target + lockfiles, etc.).

### 15.2 `morph run`

`morph run` executes a Program `P` under:

- Environment `E` (including runner + toolchain + model config)
- Initial state \( s_0 \)

It produces a **Run** object:

- Output state \( s_1 \) (including produced artifact tree \( D_1 \))
- Trace DAG
- Raw tool/model receipts
- Optional intermediate artifacts

In math terms, it realizes an element of \( P_E(s_0) \in F(S) \).

### 15.3 `morph eval`

`morph eval` executes one or more EvalSuites `T` against:

- A chosen program `P` (and/or an artifact tree `D`)
- Environment `E` (runner/toolchain)
- Specified fixture sources (candidate/base/pinned/external)

It produces evaluation evidence:

- Observations \( \mathrm{Obs}_T \)
- Metric samples
- Certification result (certificate vector)
- Trace of the evaluation process itself (compiles, test runs, logs)

In math terms, it realizes an element of \( \mathrm{Eval}_{T,E}(s) \in F(\mathrm{Obs}_T) \), then applies `T`'s certification rule.

**Important**: because evaluation is effectful, evaluation runs are first-class Runs too — same immutability, same trace, same auditability.

---

## 16. Do Agents Have to Write the Tests?

No.

Morph requires that merges be justified by *some* evaluation contract(s). Those contracts can be:

- The project's existing CI test suite
- Manually written acceptance tests
- Conformance suites from another team
- Human review scores (as an evaluation pipeline)
- Performance benchmarks
- Safety checks
- Any combination

Agents may add or improve tests, but Morph does not assume they do.

Morph's key requirement is:

> If you claim "this is a safe merge," you must point to a contract and evidence that certifies it.

---

## 17. Annotations and Provenance

### 17.1 Annotations

Annotations attach metadata to immutable objects without changing their hash.

They enable:

- Human ratings and notes on runs/trace events
- Tagging commits ("good", "regression", "candidate for release")
- Linking artifacts to decisions and reviews
- Provenance and audit overlays

### 17.2 Program Provenance

Programs may be derived from:

- Human authoring
- Distilled traces
- Composition of sub-programs

Provenance records references to the source run/trace/event that produced the program.

---

## 18. Minimal Axioms of Morph

Morph requires the following axioms.

### A. Identity and Immutability

1. **Immutable Content-Addressed Objects** — All primary objects (blobs, trees, programs, runs, traces, eval suites, commits, annotations) are immutable and content-addressed.
2. **Evidence Does Not Rewrite History** — Runs and evaluation evidence never mutate prior commits; new claims are new commits.

### B. Program Algebra

3. **Effect Monad for Sequencing** — There exists an endofunctor `F` that is a monad, enabling associative sequential composition and identity programs.
4. **Product State Spaces** — State spaces support products \( A \times B \) and a unit object \( 1 \).
5. **Zip for Parallelism** — `F` provides a lawful zip: \( \mathrm{zip}_{A,B} : F(A) \times F(B) \to F(A \times B) \), enabling principled parallel composition.

### C. Behavioral Semantics

6. **Evaluation Suites are Explicit Contracts** — An `EvalSuite` defines metrics (with ordering direction), fixture sources, and a certification rule.
7. **Certificates are Comparable** — Certified metric vectors live in a product preorder with componentwise dominance.
8. **Merge is Dominance of Joined Requirements** — A merge commit must certify dominance over the join of parent certificates under the union of their contracts.

### D. Environment and Decentralization

9. **Explicit Environment Recording** — Every run records the environment `E` (runner/toolchain/model config) needed to interpret evidence.
10. **Decentralization** — No global authority is required; verification is by hashes + receipts.
11. **Behavioral Reproducibility** — Reproducibility means the behavioral claim can be re-certified under declared environment constraints, not byte-identical outputs.

---

## 19. What Morph Is

Morph is:

- A semantic DVCS for transformation programs
- A system of certified behavioral identities backed by immutable receipts
- Merge gating defined by behavioral contracts
- Compatible with human edits, agent edits, and mixed workflows
- Usable across multi-language and binary ecosystems via runner-recorded environments

---

## 20. What Morph Is Not

Morph is not:

- A prompt registry
- A logging dashboard
- A centralized audit database
- A replacement for Git

It complements Git: Git remains ideal for deterministic source snapshots; Morph versions transformation behavior and its certified guarantees.

---

## 21. Closing Statement

Git versioned deterministic source code. Morph versions effectful transformations.

Git tracked files. Morph tracks certified behavior, with receipts.

---

## Appendix: Cheat Sheet

| Concept | Summary |
|---|---|
| **Program** | `A -> F[B]` — a transformer returning results in an effects-and-receipts box |
| **Sequential composition** | `bind` / `flatMap` — lawful pipelines because monad laws |
| **Parallel composition** | `zip` / `Promise.all` — lawful fork-join because state products + zipped effects |
| **Attribution** | \( \alpha : V \to \mathcal{A} \cup \{\bot\} \) — maps operators to agents; composes over sequential and parallel |
| **Run** | A realized execution outcome with a trace (immutable receipt) |
| **EvalSuite** | Metric definitions (name, aggregation, threshold, direction) + optional test cases |
| **Commit** | Tree hash + program hash + eval contract (suite + observed metrics) + parents |
| **Merge** | Candidate must dominate the join of parent certificates under unified contract |
| **Direction** | Each metric is `maximize` (higher is better) or `minimize` (lower is better) |
