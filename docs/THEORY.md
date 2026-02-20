# Morph Theory
## The Algebraic Foundations of a Distributed Version Control System for Prompt Programs

---

# 1. Purpose of This Document

This document defines the theoretical foundation of Morph.

It explains:

- What Morph versions
- How Morph generalizes Git
- How probabilistic prompt programs can be version-controlled
- The algebra underlying commits, merges, and evaluation
- The minimal axioms that define the Morph system

This is not an implementation document.
See `v0-spec.md` for concrete system design.

This document defines the model.

---

# 2. The Problem Morph Solves

Git versions deterministic source code.

Prompt-driven systems are not deterministic.

Modern AI systems:

- Transform document trees (codebases, datasets, files)
- Use effectful operators (LLM calls, retrieval, tools)
- Produce probabilistic outputs
- Depend on model versions and runtime environments
- Are increasingly authored by agents

Git assumes:

- Byte equality defines identity
- Reproducibility means identical outputs
- Merges are syntactic

Prompt systems violate all three.

Morph generalizes version control to:

> Effectful, probabilistic transformations over structured document state.

---

# 3. Core Intuition

In Git:

- Files are versioned.
- Commits freeze file trees.

In Morph:

- Programs are versioned.
- Commits freeze behavioral identities.

A Morph commit represents:

> A stabilized prompt program together with its declared behavioral contract.

Commits are not merely text snapshots.
They are semantic identities.

---

# 4. State Model

Morph operates over structured state.

Define state:

S = (D, C, M)

Where:

- **D** — Document tree (code, inputs, retrieved documents)
- **C** — Execution context (scratchpad, intermediate memory)
- **M** — Metadata (environment, provenance, trace references)

This state generalizes Git's file tree.

In the v0 spec, D is realized as Tree objects, C is captured in Run execution context, and M is distributed across Commit and Run metadata fields.

---

# 5. Prompt Programs as Transformations

A prompt program transforms state.

If everything were deterministic:


P : S → S


But prompt programs:

- Call models
- Sample tokens
- Use tools
- Depend on external services

Thus the true form is:


P : S → F(S)


Where F is an endofunctor representing computational effects — nondeterminism, randomness, logging, external calls, or distributions over outputs.

This is the foundational abstraction of Morph.

---

# 6. Effect Structure

Programs of type S → F(S) do not compose by ordinary function composition.

To compose P : S → F(S) and Q : S → F(S) into Q ∘ P, we need structure on F:

- A **unit** η : S → F(S) that embeds pure values into the effect (this is the identity program)
- A **join** μ : F(F(S)) → F(S) that flattens nested effects

Together, (F, η, μ) form a **monad**.

Composition of effectful programs is then defined by:

Q ∘ P = μ ∘ F(Q) ∘ P

This is the Kleisli composition. Programs P : S → F(S) are morphisms in the **Kleisli category** of the monad F.

We require F to be a monad, but we do not fix which monad. Different deployment environments may instantiate F differently:

- Pure distributions (for testing)
- IO with randomness (for production)
- Traced computations (for auditing)

The theory is parametric in F. The v0 spec realizes F operationally: running a Program produces a Run, and the Run records the effectful outcome.

---

# 7. Minimal Category Theory (Required for Morph)

We introduce only what is necessary.

## 7.1 The Kleisli Category

A category consists of:

- Objects
- Morphisms (arrows between objects)
- Composition
- Identity morphisms

In Morph's Kleisli category:

- **Object**: State S
- **Morphism**: Prompt program P : S → F(S)
- **Composition**: Kleisli composition (§6)
- **Identity**: The unit η : S → F(S) (the no-op program)

Composition is associative:

(R ∘ Q) ∘ P = R ∘ (Q ∘ P)

And the identity program satisfies:

I ∘ P = P ∘ I = P

These are the only structural requirements for sequential program chaining.

---

# 8. Parallel Composition

Morph must support:

- Multiple agents
- Parallel prompt branches
- Independent experimental variants

We define a parallel operator:


P ⊗ Q


Which runs P and Q independently and combines results.

Sequential and parallel composition together form a **monoidal category**:

- The monoidal product is ⊗
- The unit object is the trivial state (empty document tree, no context)
- Associativity: (P ⊗ Q) ⊗ R ≅ P ⊗ (Q ⊗ R)

This enables:

- Agent swarms
- Ensemble prompting
- Concurrent evaluation

In the v0 spec, parallel composition is realized by independent subgraphs within a Program's operator DAG: nodes with no connecting edges execute in parallel.

---

# 9. Behavioral Semantics

Git equality = byte equality.

Morph equality = behavioral equivalence.

---

## 9.1 Evaluation-Relative Equivalence

Let T be a test suite (realized as an EvalSuite object in v0).

Define:

P ≈ₜ Q

If:

Under T, the observable outputs of P and Q are statistically indistinguishable within required thresholds.

More precisely, for each metric m defined by T with threshold τ:

|score(P, m) − score(Q, m)| is small enough that both satisfy τ, and neither dominates the other in a practically significant way.

Equivalence is relative to T.

It is not absolute.

**v0 simplification**: In v0, ≈ₜ is approximated by both programs passing the same EvalSuite thresholds. Future versions may implement proper two-sample equivalence testing (e.g., permutation tests or equivalence bounds).

**Metric source agnosticism**: Evaluation metrics may originate from automated scoring (model outputs against expected results), human feedback (ratings collected via Annotations), or any combination. The behavioral preorder is agnostic to metric source. What matters is that scores are comparable and thresholds are meaningful. This allows the same algebraic framework to support both automated evaluation pipelines and human-in-the-loop curation workflows.

---

## 9.2 Congruence Requirement

For ≈ₜ to interact correctly with composition, it should be a **congruence**:

If P ≈ₜ P', then Q ∘ P ≈ₜ Q ∘ P' (for suitable Q and T).

This is not guaranteed for arbitrary stochastic programs. In practice, Morph enforces this by requiring evaluation at each commit boundary rather than relying on compositional equivalence propagation. Each commit independently certifies its behavioral contract.

---

## 9.3 Behavioral Preorder

We define improvement:

P ⪯ Q

If Q meets or exceeds P's observed metric scores across all metrics in the evaluation suite.

This defines a preorder:

- Reflexive: P ⪯ P
- Transitive: if P ⪯ Q and Q ⪯ R then P ⪯ R

Not necessarily symmetric: P ⪯ Q does not imply Q ⪯ P.

**Critical distinction**: dominance is measured against observed metrics, not base thresholds. If P scores 0.95 on a metric with threshold 0.8, then Q must score ≥ 0.95 to satisfy Q ⪰ P. Merely passing the 0.8 threshold is not sufficient.

The v0 spec records `observed_metrics` in each Commit for exactly this purpose.

---

# 10. Commits as Behavioral Identities

In Git:

A commit freezes a file tree.

In Morph:

A commit freezes:

- A prompt program definition (by hash)
- An evaluation contract (suite + observed metrics)
- Optional environment constraints

A commit represents a point in the behavioral preorder — a certified claim that the program achieves specific metric levels under the declared evaluation suite.

Commits are semantic stabilization events.

---

# 11. Merge as Behavioral Join

Given two commits with programs P and Q:

A merge candidate R must satisfy:

R ⪰ P
R ⪰ Q

Under the behavioral preorder.

That is: R must dominate both parents' observed metric scores across all evaluation dimensions.

When P and Q have different evaluation suites T₁ and T₂, the merge eval contract is the **union** T₁ ∪ T₂. The merged program must satisfy all metrics from both suites and dominate both parents' observed scores.

If such R does not exist (or cannot be found), merge fails.

Thus merge is not purely structural.
It is behavioral.

---

# 12. Runs and Traces

A run is an execution instance:


Run = P(E, S₀)


Where:

- E = environment (model, decoding parameters, toolchain)
- S₀ = initial state

A run records:

- Inputs
- Environment
- Outputs
- Metrics
- Trace

Runs are immutable.

Commits are claims.
Runs are receipts.

A Trace is the detailed execution record of a Run. In v0, Trace events are typed and individually addressable by ID, enabling fine-grained annotation of specific steps within an execution.

---

# 13. Annotations

An annotation attaches metadata to an immutable object without altering its hash.

Annotations are themselves immutable, content-addressed objects. They form a separate layer of metadata over the primary object graph (Programs, Commits, Runs, Traces, Blobs).

Annotations enable:

- Human feedback signals (ratings, bookmarks, notes) on runs and trace events
- Categorical tagging of objects
- Cross-references and links between objects
- Provenance chains (linking derived programs to their source runs)
- Any domain-specific metadata a higher-level tool needs to attach

Annotations do not participate in the behavioral preorder directly. However, feedback annotations may be aggregated into evaluation metrics by higher-level tools, closing the loop between human judgment and automated evaluation.

This design preserves Axiom 1 (immutable objects) while enabling rich, extensible metadata layering. Because annotations reference their targets by hash, they compose naturally with content-addressed storage and decentralized distribution.

In the v0 spec, Annotations are realized as a core object type (§4.10) with open `kind` and `data` fields.

---

# 14. Program Provenance

Programs may be created in multiple ways:

- Written by hand
- Extracted from a successful run or trace
- Composed from existing programs

When a program is derived from execution evidence — for example, distilling an agent session into a reusable workflow — the derivation chain should be recorded.

Provenance is tracked via an optional field on the Program object that references the source Run, Trace, and specific event (if applicable). This enables higher-level tools to answer: "Where did this program come from? Which session produced it? Which moment was it extracted from?"

Provenance complements Annotations: the Program records its origin, while Annotations on that Program (or its source Trace) provide human curation signals (ratings, notes, tags).

---

# 15. Environment Parameterization

Programs depend on environment E:


P : (E, S) → F(S)


Environment includes:

- Model identifier
- Model version (if available)
- Decoding parameters
- Toolchain versions
- Policy references (where applicable)

Reproducibility is defined relative to E.

The v0 spec mandates that every Run object records its full environment.

---

# 16. Prompts as Objects

Prompts are first-class Morph objects (Blob objects with kind "prompt" in v0).

However, they may be materialized as files for:

- Diffing
- Code review
- Pull requests

The object store is canonical.
Filesystem views are projections.

The Blob `kind` field is an open string, allowing downstream tools to introduce their own content types (templates, examples, resources) without modifying Morph's core schema.

---

# 17. Working Space vs Commit Space

Morph separates:

### Working Space
- Exploratory prompt evolution
- Agent experimentation
- Trace accumulation
- Non-stabilized programs

### Commit Space
- Stabilized program definitions
- Evaluation-certified behavioral identities

Rollup collapses exploratory commits into stable identities.

Traces are never rewritten.
They may be summarized via TraceRollup objects.

In the v0 spec, working space is the filesystem (`prompts/`, `programs/`, `evals/`). Commit space is the `.morph/objects/` store and the commit DAG.

---

# 18. Agent-Native Assumptions

Morph assumes:

- Agents generate code
- Agents modify prompt programs
- Agents produce extensive trace trees

Thus:

- Agent identity is recorded in Run manifests
- Policy references are versioned where applicable
- Evaluation gating is mandatory for merge
- Receipts accompany proposed commits

Morph is not merely auditing.
It is coordination and accountability infrastructure.

---

# 19. Minimal Axioms of Morph

Morph requires the following axioms:

1. **Immutable Objects**
   All blobs, trees, commits, runs, traces, and annotations are content-addressed and immutable.

2. **Associative Composition**
   Prompt programs compose associatively under Kleisli composition.

3. **Identity Program**
   There exists a no-op program I (the monadic unit η) such that I ∘ P = P ∘ I = P.

4. **Evaluation-Relative Equivalence**
   Equivalence between programs is defined relative to evaluation suites.

5. **Behavioral Preorder**
   There exists a preorder ⪯ defined by observed metric dominance.

6. **Merge Dominance Requirement**
   A merge commit must dominate both parents' observed metrics under ⪯.

7. **Runs Do Not Rewrite Commits**
   Execution evidence does not alter commit history.

8. **Explicit Environment Recording**
   All runs must record environment E.

9. **Decentralization**
   No global authority is required for commits or runs.

10. **Behavioral Reproducibility**
    Reproducibility is defined by preservation of evaluation contract, not byte equality.

---

# 20. What Morph Is

Morph is:

- A semantic DVCS
- A behavioral version control system
- A probabilistic generalization of Git
- A foundation for agentic software development

---

# 21. What Morph Is Not

Morph is not:

- A prompt registry
- A logging dashboard
- A centralized audit database
- A replacement for Git

It complements Git by extending version control into probabilistic systems.

Higher-level tools — session capture, curation, workflow extraction, registries — can be built on Morph's object model and annotation layer without requiring changes to the core system.

---

# 22. Closing Statement

Git versioned deterministic source code.

Morph versions stochastic transformations.

Git tracked files.

Morph tracks meaning.
