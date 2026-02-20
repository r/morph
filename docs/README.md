# Morph

**Version control for prompt programs.**

---

## Why Morph Exists

Git solved version control for deterministic source code. You write code, commit it, diff it, branch it, merge it. Two commits are the same if their bytes are the same. Reproducibility means running the same code produces the same output.

That model breaks down for AI-native systems.

When your software involves prompts, LLM calls, retrieval pipelines, and autonomous agents:

- **The same prompt can produce different outputs.** Models are stochastic. Run the same prompt twice, get two different results.
- **Behavior depends on things outside the code.** Model version, decoding parameters, retrieval corpus, tool availability — all affect what happens.
- **"Did it get better?" is a statistical question.** You can't just eyeball a diff. You need to run evaluations and compare metric distributions.
- **Agents are writing code.** When an AI agent produces a patch, you need to know which agent, which model, which prompt, and whether the result actually works.
- **Merging can silently regress behavior.** Two branches can merge cleanly at the text level while producing worse outputs than either parent.

Git tracks *what changed* in your files. It has no concept of *what your program does* or *whether it got better*. For prompt-driven systems, that's the thing that matters most.

Morph extends version control to track behavior — not just text.

---

## What Morph Actually Is

Morph is a content-addressed version control system, structured like Git, but designed for prompt programs instead of source files.

If you know Git, here's the mapping:

| Git | Morph |
|---|---|
| Source files | Prompt programs (DAGs of prompt calls, tool calls, retrieval steps) |
| Commit = snapshot of file tree | Commit = snapshot of a program + its evaluation scores |
| Diff = text comparison | Diff = behavioral comparison (metric scores under an eval suite) |
| Merge = reconcile text | Merge = reconcile text **and** prove behavior didn't regress |
| `.git/objects/` | `.morph/objects/` (same idea: content-addressed, immutable, Merkle DAG) |
| Working tree | Working space (prompts, programs, evals as editable files) |

The key difference: every Morph commit carries an **evaluation contract**. The commit records not just the program definition, but the eval suite it was tested against and the metric scores it achieved. This is what makes behavioral comparison, gated merges, and regression detection possible.

---

## How It Works

### The Object Model

Morph stores everything as immutable, content-addressed objects (SHA-256), just like Git. The core object types are:

- **Blob** — an atomic content unit. Prompts, tool schemas, configs, policies. Each blob has a `kind` (e.g., `prompt`, `schema`, `config`).
- **Tree** — a structured grouping of objects. Analogous to Git trees (directories).
- **Program** — the thing you're versioning. A directed acyclic graph of operators: prompt calls, tool calls, retrieval steps, transforms. This is Morph's equivalent of a source file, but it represents a *pipeline* rather than static text.
- **EvalSuite** — a versioned evaluation definition. Test cases, metrics, aggregation methods, pass thresholds. This is what defines "did the program get better or worse."
- **Commit** — a program snapshot plus an evaluation contract. Records the program hash, the eval suite hash, and the observed metric scores. Commits are *claims* about behavior.
- **Run** — an execution receipt. Records exactly what happened when a program was executed: inputs, outputs, environment (model, version, parameters), metrics, and a full trace. Runs are *evidence* for claims.
- **Trace** — the detailed execution record of a run. A sequence of typed, addressable events (prompt calls, tool calls, file edits, errors). Each event has an ID, so you can annotate individual steps.
- **Annotation** — metadata attached to any object (or a specific event within a trace) without changing its hash. Feedback ratings, bookmarks, tags, notes, cross-references. This is how human judgment enters the system.

### Programs, Not Files

In Git, the unit of versioning is a file. In Morph, it's a **program** — a graph of operations that transforms some input state into output state, potentially involving LLM calls, tool use, and retrieval along the way.

A program node might be:

- `prompt_call` — sends a prompt to an LLM
- `tool_call` — invokes an external tool
- `retrieval` — fetches context from a document store
- `transform` — a deterministic data transformation
- `identity` — a no-op pass-through

Nodes connected by edges execute sequentially (output of one feeds into the next). Nodes with no edges between them can execute in parallel. This gives you composable pipelines where the structure is explicit and versionable.

Programs also track **provenance**: was this program written by hand, extracted from a successful agent session, or composed from existing programs? If it came from a run, the source run and trace are recorded.

### Commits Are Behavioral Claims

A Git commit says: "here's what the files looked like at this point."

A Morph commit says: "here's a program, here's the eval suite I tested it against, and here are the scores it achieved."

```
Commit
├── program: <hash>          # the program definition
├── eval_contract:
│   ├── suite: <hash>        # which eval suite
│   └── observed_metrics:    # what scores were achieved
│       ├── accuracy: 0.92
│       └── latency_p95: 1.2s
├── parents: [<hash>, ...]   # parent commits (same as Git)
├── message: "..."
└── author, timestamp, ...
```

The `observed_metrics` are not just a record — they're a *contract*. When you merge, the merge commit must meet or exceed these scores. This is what prevents behavioral regression.

### Runs Are Execution Receipts

Every time you execute a program, Morph records a **Run** object capturing:

- Which program was run (hash)
- The full environment (model ID, version, decoding parameters, toolchain versions)
- Input state
- Output artifacts
- Metric scores
- A link to the full Trace
- Agent identity (if an agent ran it)

Runs are immutable. They never modify commits or history. They're evidence — receipts you can point to when someone asks "how do you know this program works?"

### Evaluation-Gated Merge

This is where Morph diverges most from Git.

In Git, merge is a text operation: reconcile the diffs, resolve conflicts, done.

In Morph, merge is a **behavioral operation**:

1. Structurally merge the program graphs (like Git merges text).
2. Determine the merge eval contract: the **union** of both parents' eval suites. Every metric from both parents must be satisfied.
3. Run the merged program against this combined eval suite.
4. Check **dominance**: the merged program's scores must meet or exceed both parents' observed metrics — not just pass the base thresholds, but actually be as good as what each parent achieved.
5. If dominance holds, create the merge commit. If not, merge fails.

This means merge can fail even when there are no text conflicts. If parent A achieved 0.95 accuracy and parent B achieved 0.90 latency, the merge must achieve at least 0.95 accuracy *and* 0.90 latency. If the combined program can't hit both, you have a behavioral conflict that needs resolution — just like a text conflict in Git, but at the semantic level.

### Working Space vs. Commit Space

Morph separates exploration from stabilization, similar to Git's working tree vs. committed history:

- **Working space** (`prompts/`, `programs/`, `evals/`) — where you edit prompt files, tweak program definitions, and iterate on eval suites. This is your scratchpad. Nothing here is versioned until you commit.
- **Commit space** (`.morph/objects/`, the commit graph) — stabilized, evaluation-certified snapshots. Once committed, objects are immutable and content-addressed.

A **rollup** (analogous to squash) collapses a sequence of exploratory commits into a single stable commit. Unlike Git squash, rollup never deletes traces — it just creates a new commit that supersedes the old ones. The old commits and their evidence remain addressable by hash.

### Annotations

Morph objects are immutable, but the world's understanding of them evolves. Annotations solve this.

An annotation attaches metadata to any object — a program, a commit, a run, or even a specific event within a trace — without altering that object's hash. Annotations are themselves immutable and content-addressed.

Use cases:

- **Feedback**: "this prompt pattern works well" (rating on a trace event)
- **Bookmarks**: "this is the run where we found the fix" (marking a notable moment)
- **Tags**: categorizing programs by domain, team, or purpose
- **Cross-references**: linking a program to the run it was extracted from

Because annotations never rewrite history, they're safe for human feedback loops: rate outputs, tag good patterns, bookmark key moments — all layered on top of the immutable object graph.

---

## The CLI

Morph's CLI mirrors Git where it makes sense:

```
morph init                        # initialize a repository
morph status                      # show working space status
morph log                         # show commit history

morph prompt create               # create a prompt object
morph prompt materialize <hash>   # write a prompt to the filesystem for review

morph program create              # create a program definition
morph program show                # inspect a program

morph add .                       # stage working space changes
morph commit -m "message"         # evaluate and commit (runs eval suite, records metrics)

morph branch <name>               # create a branch
morph checkout <name>             # switch branches

morph run <program>               # execute a program, produce a Run + Trace
morph eval <program>              # run eval suite, show metrics

morph merge <branch>              # behavioral merge (eval-gated)
morph rollup <range>              # collapse exploratory history

morph annotate <hash> --kind feedback --data '{"rating": "good"}'
morph annotations <hash>          # list annotations on an object
```

If you know Git, most of this is familiar. The main additions are `run`, `eval`, `prompt`, `program`, `annotate`, and `rollup`.

The biggest behavioral difference is `commit` and `merge`: both involve running evaluations and recording metric scores. A commit isn't just a snapshot — it's a certified behavioral claim.

---

## What Morph Is Not

- **Not a prompt registry.** Morph versions programs (pipelines of operations), not just individual prompt templates.
- **Not a logging dashboard.** Runs and traces are first-class versioned objects, not ephemeral logs.
- **Not a replacement for Git.** Morph complements Git. Your source code still lives in Git. Morph handles the prompt programs, evaluations, and behavioral contracts that Git can't express.

---

## What to Read Next

- **`THEORY.md`** — The formal mathematical foundations: how programs compose, what behavioral equivalence means precisely, and the axioms that make the system coherent.
- **`v0-spec.md`** — The concrete v0 system specification: object schemas, storage backend, CLI details, and how each theoretical concept maps to an implementation construct.

---

## Status

Morph is an active research and engineering effort. The goal is to extend version control into the era of AI-native, agent-driven software development.
