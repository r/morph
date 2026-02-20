# Morph Theory

**The Algebraic Foundations of a Distributed Version Control System for Prompt Programs**

*(Corrected, formalized, and engineer-readable edition)*

---

## 1. Purpose of This Document

This document defines the mathematical model of Morph.

It explains:

- What Morph versions
- How Morph generalizes Git
- How probabilistic prompt programs can be version-controlled
- The algebra underlying programs, commits, evaluation, and merges
- The minimal axioms that make the system coherent

This is not an implementation document.
See `v0-spec.md` for concrete system design.

This document is the "why the system makes sense" layer.

---

## 2. The Problem Morph Solves

Git versions deterministic text.

Prompt-driven systems are not deterministic.

Modern AI systems:

- Transform structured document state (codebases, datasets, trees of files, retrieved context)
- Use effectful operators (LLM calls, retrieval, tools)
- Produce probabilistic outputs
- Depend on model versions and runtime environments
- Are increasingly authored by agents and swarms

Git assumes:

- Byte equality defines identity
- Reproducibility means identical outputs
- Merges are syntactic (diff + patch)

Prompt systems violate all three.

Morph generalizes version control to:

> Effectful, probabilistic transformations over structured state, with evaluation-defined semantics.

---

## 3. Core Intuition

In Git:

- Files are versioned.
- Commits freeze file trees.

In Morph:

- Programs are versioned.
- Commits freeze a program plus a behavioral contract.

A Morph commit represents:

> A stabilized prompt program together with an explicit behavioral certificate:
> "Under this evaluation contract (and stated environment constraints), this program achieves at least these metric levels."

Commits are not only text snapshots.
They are semantic stabilization events backed by evidence.

---

## 4. State and Environment

Morph is about programs that transform state under an environment.

### 4.1 State

We model "everything the program can read/write" as structured state.

Let state be a product type:

$$S = D \times C \times M$$

Where:

- $D$ — Document tree (code, inputs, retrieved documents, datasets)
- $C$ — Execution context (scratchpad, intermediate memory, cached sub-results)
- $M$ — Metadata (provenance pointers, trace references, run IDs, etc.)

Engineer translation: think of $S$ as a strongly-typed "workspace object" with three fields: documents, working memory, metadata.

We will not require any particular internal representation — only that state can be composed and partitioned (more in §8).

### 4.2 Environment

Programs also depend on an environment:

$$E = \text{(model id/version, decoding params, toolchain versions, policy refs, \ldots)}$$

Crucially, the environment is not state in the version-control sense. It is an input parameter that must be recorded.

We will write:

- A program as a family of state transformers parameterized by environment:

$$P_E : S \to F(S)$$

Equivalently:

$$P : (E, S) \to F(S)$$

Both are the same idea; we'll use whichever is clearer.

---

## 5. Prompt Programs as Effectful Transformations

A prompt program is not "just a function," because it may:

- Sample tokens
- Call tools
- Branch
- Log traces
- Fail and retry
- Consult retrieval

So the correct type is:

$$P_E : S \to F(S)$$

Where $F$ is a type constructor describing computational effects.

Examples of what $F(X)$ might mean:

- "a probability distribution over $X$"
- "$X$ plus a trace"
- "$X$ plus logs and tool calls"
- "$X$ or an error"
- combinations of the above

A particularly useful mental model is:

$$F(X) \approx \text{Distribution}\big(\text{Trace} \times X\big)$$

Meaning: running the program produces a trace and a resulting state, but probabilistically.

Engineer translation: $F(S)$ is "$S$ inside a box" that also carries randomness and receipts.
If you know `Promise<T>`, `Result<T>`, `Generator<T>`, `Observable<T>`, `IO<T>`, etc. — this is the same pattern.

---

## 6. Sequential Composition Requires a Monad

If you have:

- $P : A \to F(B)$
- $Q : B \to F(C)$

you can't compose them with ordinary function composition, because the types don't match.

To compose effectful computations, we need the standard structure:

- **pure / return**: $\eta_A : A \to F(A)$
- **bind** (or equivalently join + map)

This is precisely a monad.

### 6.1 The Monad Interface (Engineer-Friendly)

A monad gives you:

```
pure : A -> F[A]
map  : (A -> B) -> F[A] -> F[B]
bind : F[A] -> (A -> F[B]) -> F[B]
```

Then sequential composition is:

$$(Q \circ P)(a) = \text{bind}(P(a), Q)$$

Engineer translation:
`bind` is `flatMap`.
Sequential composition is "run P, then feed its result into Q."

### 6.2 The Monad Laws (What Makes the Algebra Work)

We require the standard laws:

1. **Left identity**: $\text{bind}(\text{pure}(a), f) = f(a)$
2. **Right identity**: $\text{bind}(x, \text{pure}) = x$
3. **Associativity**: $\text{bind}(\text{bind}(x, f), g) = \text{bind}(x, (a \Rightarrow \text{bind}(f(a), g)))$

These imply:

- Sequential program chaining is associative
- There is an identity/no-op program

This is the minimum structure needed for "pipelines of prompt ops" to have reliable algebra.

---

## 7. The Category of Programs (Kleisli Category)

Now we can define the mathematical universe Morph lives in.

### 7.1 Objects: State Spaces (Not Just One State)

To speak correctly about parallelism, composition, and modularity, we allow many state types:

- Objects are types/schemas $A, B, C, \ldots$ (state spaces)

In practice, Morph often uses one "big state" $S$, but mathematically it is cleaner (and more powerful) to allow sub-states and product states.

### 7.2 Morphisms: Effectful Programs

Morphisms are:

$$A \to F(B)$$

i.e., programs that transform an input state-space $A$ into an output state-space $B$, inside effects.

### 7.3 Composition and Identity

Composition is Kleisli composition via bind:

- If $P : A \to F(B)$ and $Q : B \to F(C)$, then $Q \circ P : A \to F(C)$

Identity on object $A$ is:

$$\eta_A : A \to F(A)$$

This category is the Kleisli category of the monad $F$, written $\mathrm{Kl}(F)$.

Engineer translation:
The Kleisli category is "the world where functions returning `F[...]` are treated like normal arrows."

---

## 8. Parallel Composition Requires Product State + a "Zip" for Effects

Morph needs parallel execution:

- Agent swarms
- Branching experiments
- Ensembles
- Independent subgraphs in a DAG

To do this mathematically correctly, we need two ingredients:

1. A way to combine state spaces (product)
2. A way to combine effects (zip)

### 8.1 Product of State Spaces

Assume state spaces support a product:

$$A \times B$$

and a unit/empty state:

$$1$$

This is just the standard "tuple/product" idea:

- $A \times B$ holds both an $A$ and a $B$
- $1$ holds no information (unit type)

This makes the base category of state spaces cartesian monoidal.

Engineer translation:
$A \times B$ is a struct/tuple `(A, B)`.
The unit $1$ is like `()`.

### 8.2 Zipping Effects

To run two effectful computations "independently" and combine their results, we need a natural operation:

$$\text{zip}_{A,B} : F(A) \times F(B) \to F(A \times B)$$

This is the effectful version of "pair these results."

- For promises, this is like `Promise.all`.
- For distributions, it's the product distribution (independent sampling).
- For traced computations, it means "combine traces + pair outputs."

To make the algebra work, zip must satisfy coherence laws (associativity/unitality, plus symmetry if you want commutativity). This structure is commonly packaged as:

- an **applicative functor**, or
- a **strong monad**, and if symmetric:
- a **commutative strong monad** (also called a symmetric monoidal monad)

Morph's "parallel semantics" assumes at least this much structure when you claim parallel algebraic laws.

**Key point:**
Sequential composition needs a monad.
Parallel composition needs a monad plus a lawful zip.

### 8.3 Defining Parallel Composition

Given:

- $P : A \to F(B)$
- $Q : C \to F(D)$

Define:

$$P \otimes Q : (A \times C) \to F(B \times D)$$

by:

$$(P \otimes Q)(a, c) = \text{zip}(P(a),\, Q(c))$$

This is the mathematically clean form of "run both branches and pair outputs."

**Same-input branching** (common in agent systems)

Often you want $P, Q : S \to F(S)$ to both read the same input state.

Use the diagonal function:

$$\Delta : S \to S \times S, \quad \Delta(s) = (s, s)$$

Then:

$$\text{branch}(P, Q) = (P \otimes Q) \circ \Delta : S \to F(S \times S)$$

Now you have both branch outputs side-by-side.

If you want to reconcile them into one state, you add an explicit join program:

$$J : (S \times S) \to F(S)$$

and define:

$$\text{fork-join}(P, Q) = J \circ \text{branch}(P, Q)$$

This matches real systems: parallel branches produce separate artifacts, and a later step merges them.

Engineer translation:
Parallelism is not "two threads racing on the same memory."
It's "fork state into two sandboxes, run both, then explicitly join."

---

## 9. Runs and Traces

A run is one execution instance of a program under a concrete environment and initial state:

$$\text{Run}(P, E, s_0)$$

Conceptually, if $F$ is distribution-like, a run is a sample from:

$$P_E(s_0) \in F(S)$$

### 9.1 Trace as a First-Class Receipt

A run records:

- Inputs
- Environment
- Outputs
- Tool calls
- Intermediate events
- Metrics
- Timing, costs, etc.

A **Trace** is the detailed execution record.

To support parallelism cleanly, it is helpful to model Trace as a DAG of events (partial order), not necessarily a single linear log. Parallel branches then combine traces by DAG union (with coherence).

Runs and traces are immutable evidence.

---

## 10. Behavioral Observations and Evaluation Suites

Git observes "bytes."

Morph observes "behavior," but behavior must be made operational: you need a defined observation function.

### 10.1 Evaluation Suite

An evaluation suite $T$ defines:

- A set of test inputs / scenarios (if applicable)
- A set of metrics $m \in T$
- For each metric:
  - How to compute it from a run (scoring function)
  - Which direction is better (maximize/minimize)
  - Thresholds (contract targets)
  - Statistical method / confidence (how claims are certified)

Formally, each metric $m$ provides a scoring function:

$$\mathrm{score}_m : \text{(Run evidence)} \to V_m$$

where $V_m$ is an ordered value domain (often $\mathbb{R}$, but could be categories, booleans, etc.).

And each metric includes an order $\le_m$ meaning "no better than," so we can compare correctly even when "lower is better."

Engineer translation:
A metric is not just a number; it's a number with an ordering direction and a definition.

### 10.2 From Program to Metric Distributions

A program is probabilistic, so metric values are random variables.

Running $P$ under $(E, s_0)$ induces a distribution over runs, and pushing that through scoring induces a distribution over metric vectors:

$$\mathcal{D}_{P,E,s_0,T}$$

This is the semantic behavior of the program with respect to suite $T$.

---

## 11. Contracts, Satisfaction, and Equivalence (Separated Correctly)

This section fixes a common pitfall: "passing tests" is not the same thing as "being equivalent."

### 11.1 Contract Satisfaction

A contract is a predicate a program must satisfy under a suite $T$.

Because behavior is stochastic, satisfaction must specify how uncertainty is handled.

Morph does not force one method, but the contract must declare its rule, e.g.:

- **Expectation-based**: $\mathbb{E}[\mathrm{score}_m] \ge \tau_m$
- **Quantile-based**: $\Pr(\mathrm{score}_m \ge \tau_m) \ge 1 - \delta$
- **Confidence-bound-based**: lower confidence bound $\ge$ threshold

We write:

$$P \models_{E,s_0} T$$

to mean "$P$ satisfies contract $T$ under environment $E$ and initial state $s_0$" (using $T$'s declared certification method).

Engineer translation:
"Passes the suite" means the contract's statistical rule says "yes," not just "the point estimate looks good."

### 11.2 Observational Equivalence (Mathematically Real Equivalence)

Define the observable behavior of a program under $T$ as the induced metric distribution:

$$\mathcal{D}_{P,E,s_0,T}$$

Then define exact equivalence:

$$P \equiv_{E,s_0,T} Q \quad\text{iff}\quad \mathcal{D}_{P,E,s_0,T} = \mathcal{D}_{Q,E,s_0,T}$$

This is a true equivalence relation (reflexive, symmetric, transitive).

It is also very strict — rarely true in practice — but it is the correct mathematical anchor.

### 11.3 Approximate Equivalence (Useful, but Not a True Equivalence)

In practice you want "close enough."

Choose a distance $d$ on distributions (e.g., Wasserstein distance, total variation, MMD, etc.) and define:

$$P \approx^{\varepsilon}_{E,s_0,T} Q \quad\text{iff}\quad d(\mathcal{D}_{P,E,s_0,T},\, \mathcal{D}_{Q,E,s_0,T}) \le \varepsilon$$

This is a tolerance/closeness relation. It is not generally transitive for a fixed $\varepsilon$, but it is still very useful.

Engineer translation:
Exact equivalence is the "physics."
Approximate equivalence is the "engineering."

---

## 12. Certified Scores and the Behavioral Preorder

Morph needs an ordering notion for "better than / dominates."

Doing this directly on distributions is possible but can get heavy.

Instead, Morph uses certificates: conservative summaries produced by the evaluation contract's certification procedure.

### 12.1 The Certificate Vector

For a suite $T$, define the certificate space:

$$V_T = \prod_{m \in T} V_m$$

Each metric domain $V_m$ comes with its order $\le_m$.
The product order $\le_T$ is defined componentwise:

$$x \le_T y \quad\text{iff}\quad \forall m \in T,\; x_m \le_m y_m$$

A certification procedure produces a certificate:

$$\mathrm{cert}_T(P, E, s_0) \in V_T$$

Examples of cert outputs:

- Expected score vector
- Lower confidence bound vector (recommended)
- "Guaranteed pass margins" vector

### 12.2 Behavioral Preorder (Dominance)

Now define:

$$P \preceq_{E,s_0,T} Q \quad\text{iff}\quad \mathrm{cert}_T(P, E, s_0) \le_T \mathrm{cert}_T(Q, E, s_0)$$

This is a preorder (and becomes a partial order if certificates are compared by exact equality).

This definition has two key advantages:

1. It's mathematically clean (product order).
2. It matches engineering reality: commits store certified summaries, not true unknown distributions.

**Critical distinction** (kept from your original, but now formal):
Dominance is defined against the certificate values, not merely "both pass thresholds."

---

## 13. Contract Space is a Lattice (Where "Join" Actually Lives)

The phrase "merge is a join" is only mathematically correct if we say what is being joined.

The correct place is the **certificate lattice**.

Because $V_T$ is a product of ordered sets, it has natural pointwise operations.

If each metric domain supports joins (true for real numbers with max/min under the chosen order), then $V_T$ forms a lattice with:

- **Join** (least upper bound): componentwise max (under the metric's order)
- **Meet** (greatest lower bound): componentwise min

We write the join as:

$$x \sqcup y$$

This is always well-defined in certificate space.

**Important:** This join is about required performance claims, not about syntax, and not even necessarily about the existence of a program that achieves it.

---

## 14. Commits as Behavioral Certificates

In Git:

> A commit freezes a file tree.

In Morph:

A commit freezes:

- A program definition (by hash)
- A contract definition (suite(s), scoring, statistical method)
- A certificate vector produced under that contract
- Evidence references (runs/traces) supporting the certificate
- Environment constraints (what $E$ this claim is intended to hold under)

So a commit is:

$$\text{Commit} = (\text{program\_id},\; T,\; \text{env\_constraints},\; v,\; \text{evidence\_refs})$$

where:

- $v \in V_T$ is the certificate vector stored in the commit.

Commits are claims. Runs are receipts.
Everything is immutable and content-addressed.

---

## 15. Merge as Behavioral Join in Contract Space

Now we can define merge correctly, with real math behind it.

### 15.1 When Suites Differ: Union of Metrics

If two parent commits use suites $T_1$ and $T_2$, the merge contract uses:

$$T = T_1 \uplus T_2$$

A disjoint union keyed by metric IDs, not just names.

(If two metrics share a name but differ in definition, they are different IDs and both appear.)

### 15.2 The Required Certificate is a Join

Let the parent certificates be:

- $v_P \in V_{T_1}$
- $v_Q \in V_{T_2}$

Embed them into $V_T$ (by aligning metrics by ID), then define:

$$v_{\text{req}} = \mathrm{embed}(v_P) \sqcup \mathrm{embed}(v_Q)$$

This $v_{\text{req}}$ is the least certificate that dominates both parents' certified claims.

That is the correct sense in which "merge is a join."

### 15.3 Valid Merge Candidate

A merge candidate program $R$ is valid if:

$$\mathrm{cert}_T(R, E, s_0) \ge_T v_{\text{req}}$$

Equivalently: it dominates both parents on their metrics, under the unified suite.

If no such $R$ exists (or you can't certify one), merge fails.

Engineer translation:
Git merge is a text-level reconciliation.
Morph merge is: "find a program whose certified behavior is at least the componentwise max of the parents' certificates."

This also explains why merges can fail even when syntax merges cleanly: behavior constraints may be incompatible.

---

## 16. Runs Do Not Rewrite Commits

Axiomatically and operationally:

- Runs are immutable evidence.
- Commits are immutable claims.

If later evidence changes your belief about performance (models shift, tools drift), you do not mutate the old commit. You create a new commit with a new certificate under a stated environment.

This is the behavioral analogue of "history doesn't rewrite."

---

## 17. Annotations (Metadata Without Hash Mutation)

Annotations attach metadata to immutable objects without altering their hash.

Annotations are themselves immutable, content-addressed objects.

They can target:

- Programs
- Commits
- Runs
- Trace events
- Blobs/Trees

Annotations enable:

- Human feedback (ratings, notes, tags)
- Provenance links
- Curation
- Dataset labeling
- Policy/audit links

Annotations don't directly change certificates, but they can be used by evaluation suites as inputs.

---

## 18. Program Provenance

Programs can be created by:

- Manual authorship
- Extraction from a run/trace
- Composition of existing programs

Provenance is recorded by references to:

- Source Run
- Source Trace
- Possibly an event ID

This allows answering:

- "Where did this program come from?"
- "Which session produced it?"
- "What evidence led to this workflow?"

---

## 19. Prompts as Objects, Files as Projections

Prompts are first-class Morph objects (blobs of kind "prompt").

They can be materialized as files for:

- Diffing
- Review
- PR workflows

But the canonical identity is the object store (content-addressed).
Filesystem views are projections.

---

## 20. Working Space vs Commit Space, and Rollups Without Rewriting

Morph separates:

**Working Space**

- Exploratory program evolution
- Agent experimentation
- Many runs/traces
- Unstable variants

**Commit Space**

- Stabilized program objects
- Evaluation-certified commits
- Merge-gated coordination

A rollup is implemented by creating a new commit that supersedes (references) a set of earlier work-in-progress commits.

Nothing is deleted or rewritten. Older commits remain addressable by hash.

---

## 21. Agent-Native Assumptions

Morph assumes:

- Agents generate and modify prompt programs
- Agent sessions create large trace DAGs
- Evaluation gating is mandatory for merge
- Receipts (runs/traces) accompany proposed commits

Thus:

- Agent identity is recorded in runs
- Policy references are versioned (when applicable)
- Evidence is first-class

Morph is coordination infrastructure, not only auditing.

---

## 22. Minimal Axioms of Morph (Corrected)

Morph requires the following axioms.

### A. Immutability and Identity

1. **Immutable Objects** — All blobs, trees, programs, commits, runs, traces, and annotations are content-addressed and immutable.
2. **Evidence Immutability** — Runs and traces are immutable receipts. Evidence never mutates prior objects.

### B. State and Effects

3. **State Spaces Support Product** — State spaces form a cartesian monoidal structure with product $\times$ and unit object $1$.
4. **Effect Monad** — There exists an endofunctor $F$ over state spaces that is a monad (pure + bind), enabling sequential composition.
5. **Zip for Parallelism** — To support lawful parallel composition, $F$ provides a natural zip:
   $$\text{zip}_{A,B} : F(A) \times F(B) \to F(A \times B)$$
   satisfying associativity/unitality (and symmetry if you want commutative parallelism).

### C. Program Algebra

6. **Associative Sequential Composition** — Kleisli composition is associative.
7. **Identity Program** — For every state space $A$, $\eta_A : A \to F(A)$ acts as the identity program.
8. **Parallel Composition (When Zip Exists)** — Parallel composition $\otimes$ is defined using zip and obeys monoidal laws up to the usual tuple isomorphisms.

### D. Behavioral Semantics

9. **Evaluation Suites Define Observations** — An evaluation suite $T$ defines metrics with explicit ordering and a certification rule.
10. **Certificates Live in a Product Preorder** — Each commit stores a certificate vector $v \in V_T$ and dominance is componentwise order.
11. **Merge as Join in Contract Space** — Merge validity is defined by dominating the join of parent certificates under the unified suite.

### E. Reproducibility and Decentralization

12. **Explicit Environment Recording** — Every run records environment $E$; commits declare environment constraints for which their certificate is intended.
13. **Decentralization** — No global authority is required for object creation or verification; content addressing + evidence suffice.
14. **Behavioral Reproducibility** — Reproducibility means preserving the evaluation contract and being able to reproduce (or re-certify) the behavioral claim under stated environment constraints — not byte-identical outputs.

---

## 23. What Morph Is

Morph is:

- A semantic DVCS
- A behavioral version control system
- A probabilistic generalization of Git
- A foundation for agentic software development
- A system where merges are behavioral constraints, not only text reconciliation

---

## 24. What Morph Is Not

Morph is not:

- A prompt registry
- A logging dashboard
- A centralized audit database
- A replacement for Git

It complements Git by extending version control into probabilistic systems.

---

## 25. Closing Statement

Git versioned deterministic source code.

Morph versions stochastic transformations.

Git tracked files.

Morph tracks behavior — as certified contracts backed by immutable receipts.

---

## Appendix: "Engineer Superpowers" Cheat Sheet

- **Program type**: `A -> F[B]` — a transformer that returns results "in a box" with effects
- **Sequential composition**: `flatMap` / `bind` — pipelines are lawful because monad laws
- **Parallel composition**: `zip` / `Promise.all` / "pair the effects" — lawful because zip obeys coherence rules
- **Evaluation**: a function from runs to metrics + a certification rule — behavior becomes something you can compare
- **Commit**: program hash + contract + certificate + evidence refs
- **Merge**: unify suites, take certificate join (componentwise max), require candidate dominates it
