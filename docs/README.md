# Morph

**Version control for transformation programs. Git for behavior, not just bytes.**

---

## The Problem

Git versions deterministic source code. Identity is byte equality. Reproducibility means same input → same output. Merge is text reconciliation.

That model breaks when your software involves LLMs, agents, retrieval pipelines, and tool-calling workflows:

- **Same code, different outputs.** Models are stochastic. Run a prompt twice, get two results.
- **Behavior depends on the environment.** Model version, decoding params, retrieval corpus, tool availability — all affect output. None of this lives in a file tree.
- **"Did it get better?" is a statistical question.** You can't eyeball a diff. You need to run evals and compare scores.
- **Agents produce patches.** You need provenance: which agent, which model, which prompt, and whether the result actually works.
- **Merge can silently regress behavior.** Two branches merge cleanly at the text level while producing worse outputs than either parent.

Git tracks *what changed in your files*. It has no concept of *what your program does* or *whether it got better*.

Morph extends version control to track **certified behavior** — not just text.

---

## Git → Morph Mental Model

If you know Git internals, you already know 80% of Morph. Here's the mapping:

| Git | Morph | What changed |
|---|---|---|
| Source files | **Programs** — DAGs of operators (prompt calls, tool calls, retrieval, transforms) | The versioned unit is a pipeline, not a text file |
| `blob` / `tree` | `blob` / `tree` (same idea) | Unchanged |
| `commit` = snapshot of tree | `commit` = snapshot of program + eval contract + certified metric scores | Commits are behavioral *claims* backed by evidence |
| `diff` = text comparison | `diff` = behavioral comparison (metric deltas under an eval suite) | Semantic, not syntactic |
| `merge` = reconcile text | `merge` = reconcile text **and** prove behavior didn't regress | Merge is eval-gated |
| `.git/objects/` | `.morph/objects/` (content-addressed, immutable, Merkle DAG) | Same architecture |
| Working tree | Working space (`prompts/`, `programs/`, `evals/`) | Same role |
| — | **Run** — immutable execution receipt (env, inputs, outputs, trace, metrics) | New: execution evidence is first-class |
| — | **EvalSuite** — versioned eval definition (test cases, metrics, thresholds) | New: behavioral contracts are first-class |
| — | **Trace** — typed, addressable event log of a run | New: fine-grained execution records |
| — | **Annotation** — metadata on any object without changing its hash | New: feedback, tags, bookmarks layered on the immutable graph |

---

## Object Model

Everything is immutable and content-addressed (SHA-256), stored in `.morph/objects/`. Same principles as Git's object store.

**Blob** — Atomic content: prompt templates, tool schemas, configs, policies. Has an open `kind` field (`prompt`, `schema`, `config`, etc.).

**Tree** — Structured grouping of objects. Analogous to Git trees.

**Program** — The core versioned unit. A DAG of typed operators (`prompt_call`, `tool_call`, `retrieval`, `transform`, `identity`) connected by `data` or `control` edges. Tracks provenance: was it hand-written, extracted from an agent session, or composed from sub-programs?

**EvalSuite** — A versioned evaluation definition. Test cases, metrics (with aggregation and thresholds), and the contract that defines "better" vs "worse." This is what makes behavioral comparison possible.

**Commit** — A program snapshot + evaluation contract. Records the program hash, eval suite hash, and observed metric scores. The scores are a *contract*: merge must meet or exceed them.

**Run** — An execution receipt. Records exactly what happened: program hash, full environment (model, version, decoding params, toolchain), input state, output artifacts, metrics, trace reference, agent identity. Runs never modify commits. They're evidence.

**Trace** — The detailed execution record of a run. A sequence of typed, addressable events (each with an ID). You can annotate individual events.

**Annotation** — Metadata attached to any object (or a specific event within a trace) without altering its hash. Feedback ratings, bookmarks, tags, notes, cross-references. How human judgment enters the system.

---

## How Commits Work

A Git commit says: "here's what the files looked like."

A Morph commit says: "here's a program, the eval suite I tested it against, and the scores it achieved."

```
Commit
├── program: <hash>          # the program definition
├── eval_contract:
│   ├── suite: <hash>        # which eval suite
│   └── observed_metrics:    # certified scores
│       ├── accuracy: 0.92
│       └── latency_p95: 1.2s
├── parents: [<hash>, ...]   # parent commits (same as Git)
├── message: "..."
└── author, timestamp, ...
```

The `observed_metrics` are not decorative — they're the merge contract. A merge commit must dominate these values.

---

## How Merge Works

This is where Morph diverges most from Git.

Git merge: reconcile text diffs, resolve conflicts, done.

Morph merge:

1. **Structural merge** of program graphs (like Git merges text).
2. **Union the eval contracts.** The merge suite is the union of both parents' suites. Every metric from both parents must be satisfied.
3. **Run the merged program** against the combined eval suite.
4. **Check dominance.** The merged program's scores must meet or exceed both parents' `observed_metrics` — not just pass base thresholds, but actually be as good as what each parent achieved.
5. **If dominance holds**, create the merge commit. **If not**, merge fails.

Merge can fail with zero text conflicts. If parent A achieved 0.95 accuracy and parent B achieved 0.90 latency, the merge must hit *both*. If it can't, you have a behavioral conflict — analogous to a text conflict in Git, but at the semantic level.

---

## Working Space vs Commit Space

Same idea as Git's working tree vs committed history:

- **Working space** (`prompts/`, `programs/`, `evals/`) — your scratchpad. Edit prompts, tweak programs, iterate on evals. Nothing is versioned until you commit.
- **Commit space** (`.morph/objects/`, the commit graph) — stabilized, eval-certified snapshots. Immutable and content-addressed once committed.

**Rollup** (analogous to squash) collapses exploratory commits into a single stable commit. Unlike Git squash, rollup never deletes traces — it creates a new commit that supersedes the old ones while keeping all evidence addressable by hash.

---

## CLI

Mirrors Git where it makes sense:

```
morph init                        # initialize a repository
morph status                      # show working space status
morph log                         # show commit history

morph prompt create               # create a prompt object
morph prompt materialize <hash>   # write a prompt to filesystem for review

morph program create              # create a program definition
morph program show                # inspect a program

morph add .                       # stage working space changes
morph commit -m "message"         # evaluate and commit (runs eval, records metrics)

morph branch <name>               # create a branch
morph checkout <name>             # switch branches

morph run <program>               # execute a program, produce a Run + Trace
morph eval <program>              # run eval suite, show metrics

morph merge <branch>              # behavioral merge (eval-gated)
morph rollup <range>              # collapse exploratory history

morph annotate <hash> --kind feedback --data '{"rating": "good"}'
morph annotations <hash>          # list annotations on an object
```

The key behavioral difference from Git: `commit` and `merge` both involve running evaluations and recording metric scores. A commit isn't just a snapshot — it's a certified behavioral claim.

---

## What Morph Is Not

- **Not a prompt registry.** Morph versions programs (pipelines of operations), not individual prompt templates.
- **Not a logging dashboard.** Runs and traces are first-class versioned objects, not ephemeral logs.
- **Not a replacement for Git.** Morph complements Git. Source code stays in Git. Morph handles transformation programs, evaluations, and behavioral contracts that Git can't express.

---

## Document Map

| Document | Purpose |
|---|---|
| **This README** | Engineering overview — what Morph is, how it works, the Git analogy |
| **[THEORY.md](THEORY.md)** | Formal mathematical foundations — how programs compose, what behavioral equivalence means, the axioms that make the system coherent |
| **[v0-spec.md](v0-spec.md)** | Concrete v0 system specification — object schemas, storage backend, CLI details, how each theoretical concept maps to an implementation construct |

The README explains the *why* and the *what*. THEORY.md provides the algebraic underpinnings. v0-spec.md is the buildable blueprint.

---

## Status

Morph is an active research and engineering effort. The goal is to extend version control into the era of AI-native, agent-driven software development.
