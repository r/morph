# Morph

Morph is a version control system for AI-assisted development. It extends Git's content-addressed, Merkle DAG architecture with first-class support for execution evidence, behavioral contracts, and merge gating.

---

## The Problem

When you use an AI agent to write code — in Cursor, Claude Code, or any agentic tool — the resulting files land in your working directory and you commit them with Git. Git does its job: it snapshots the file tree and tracks line-level diffs.

But Git has no idea *how* those files got there.

It doesn't know which prompt produced a refactor. It doesn't record the conversation that led to a bug fix. It can't tell you whether the agent tried three approaches before settling on the one you committed. And when two branches both contain agent-generated code, `git merge` reconciles text — it has no way to ask "does the merged result still work?"

This is not a gap in Git. Git was designed to version deterministic source code authored by humans. It assumes identity is byte equality, reproducibility means identical output, and merge is syntactic reconciliation. Those assumptions hold for handwritten code. They break when your development process involves:

- **Probabilistic outputs** — the same prompt can produce different code each time
- **Effectful operators** — LLM calls, tool use, retrieval, test runners
- **Mixed authorship** — humans and agents editing the same codebase
- **Behavioral requirements** — "the tests must still pass" matters more than "the diff is clean"

## What Morph Does Differently

Morph versions the *transformation process*, not just the output.

Every prompt, every response, every tool call is recorded as an immutable, content-addressed trace. Commits include both a file tree snapshot (like Git) and a behavioral contract: which evaluation suite was run, and what scores were achieved. Merges require *behavioral dominance* — the merged code must be at least as good as both parents on every declared metric. The formal foundations are in [the paper](morph-paper.tex).

Concretely:

- **Pipelines and Actors** — Morph models the development process as a pipeline: a DAG of typed operators (prompt calls, tool calls, retrieval, transforms, review decisions). Every worker — human, agent, or human+agent pair — is an Actor. Each pipeline node records who contributed (as a set of actors) and what environment ran it.
- **Runs and Traces** — Each agent interaction is recorded as a Run (execution receipt) with a Trace (the full sequence of events: prompts, responses, tool calls, file edits). These are immutable objects you can inspect, compare, and annotate.
- **Behavioral Commits** — A Morph commit stores a file tree hash *and* an evaluation contract (which tests, what thresholds, what scores were observed), plus environment constraints and evidence references to the runs that back up the claim. A plain `morph commit -m "message"` works exactly like Git when you don't need evaluation gating.
- **Merge by Dominance** — Instead of three-way text merge, Morph merge requires the candidate to dominate both parents' certified metrics. If the merged code regresses on any metric, the merge fails. Metric retirement lets you explicitly drop obsolete metrics from the merge contract when the pipeline has fundamentally changed.
- **Annotations** — Attach feedback, bookmarks, tags, or notes to any object — a commit, a run, a specific event within a trace — without altering its hash.

## Why Developers Need This

If you're writing code with AI agents today, you likely have questions that your current tooling can't answer:

- *What prompt produced this code?* Morph links every file change back to the run that created it.
- *Did the agent's approach actually work?* Evaluation contracts let you gate commits on real test results, not just "it looks right."
- *How did this code get to this state?* Traces give you the full history of an agent session — every prompt, every tool call, every intermediate step.
- *Can I safely merge this agent-generated branch?* Behavioral dominance means merge only succeeds when quality is preserved.
- *Can I compare two approaches?* Runs and traces are first-class objects. You can diff them, annotate them, and build tooling on top of them.

Morph sits alongside Git, not instead of it. Both use their own dot-directory (`.morph/` and `.git/`), both track the same working tree, and commits in one are independent of the other. Use Git for backup and collaboration. Use Morph for behavioral versioning.

---

## Documentation

### Start Here

| Document | What it covers |
|---|---|
| **[THEORY.md](THEORY.md)** | The mathematical model: pipelines as effectful transformations, behavioral equivalence via certificate vectors, merge as dominance of joined requirements. Read this to understand *why Morph works the way it does*. |
| **[v0-spec.md](v0-spec.md)** | The concrete v0 system design: object schemas, storage backend, CLI commands, and a line-by-line mapping from theory to implementation. Read this to understand *what Morph builds*. |

THEORY.md defines the algebra. v0-spec.md projects that algebra into a buildable system. The code implements v0-spec.md. When the spec and theory disagree, the spec wins for v0 (and documents the simplification).

### Guides

| Document | Audience | What it covers |
|---|---|---|
| **[INSTALLATION.md](INSTALLATION.md)** | Users | Install Morph (binaries, init, IDE setup). Covers Cursor and Claude Code. |
| **[CURSOR-SETUP.md](CURSOR-SETUP.md)** | Users | Full Cursor reference: MCP server, hooks for always-on recording, rules, committing. |
| **[CLAUDE-CODE-SETUP.md](CLAUDE-CODE-SETUP.md)** | Users | Full Claude Code reference: MCP server, hooks, committing. |
| **[MORPH-AND-GIT.md](MORPH-AND-GIT.md)** | Users | Running Morph and Git side-by-side in the same repository. |
| **[TESTING.md](TESTING.md)** | Contributors | Test architecture, running tests, coverage, known gaps. |

### Academic

| Document | What it covers |
|---|---|
| **[morph-paper.tex](morph-paper.tex)** | LaTeX paper formalizing Morph: pipelines as monadic computations, evaluation contracts, merge monotonicity theorem. |

### Internal Plans

Historical design documents that guided implementation decisions. Kept for reference; the authoritative state is the code and the documents above.

| Document | Status |
|---|---|
| [plan-blob-store-sqlite.md](plans/plan-blob-store-sqlite.md) | Partially superseded by GixStore |
| [plan-gix-store-option-b.md](plans/plan-gix-store-option-b.md) | Implemented as GixStore (store version 0.2) |
| [plan-morph-viz.md](plans/plan-morph-viz.md) | Implemented as `morph visualize` |

---

## Architecture

```
morph-core/     Core library: object model, storage, hashing, commits, metrics, trees
morph-cli/      CLI (read path + manual writes): morph init, add, commit, log, ...
morph-mcp/      MCP server (primary write path from IDE): Cursor, Claude Code
morph-serve/    Browser-based repo visualization (morph visualize)
```

**Storage backend**: Trait-based (`Store`). v0 ships a single filesystem implementation (`FsStore`) supporting two hash modes: legacy (v0.0, plain SHA-256) and Git-format (v0.2+, `"blob "+len+"\0"+data`). SQLite and remote backends are anticipated by the trait interface.

**Write path**: IDE (via MCP) → morph-mcp → morph-core → `.morph/objects/`.
**Read path**: CLI → morph-core → `.morph/objects/`.
