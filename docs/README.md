# Morph Documentation

Morph is a version control system for transformation programs. It extends Git's content-addressed, Merkle DAG architecture with first-class support for programs, evaluation contracts, execution evidence, and behavioral merge gating.

This directory contains all project documentation, organized in reading order.

---

## Start Here

| Document | What it covers |
|---|---|
| **[THEORY.md](THEORY.md)** | The mathematical model: programs as effectful transformations, behavioral equivalence via certificate vectors, merge as dominance of joined requirements, and the minimal axioms. Read this to understand *why Morph exists*. |
| **[v0-spec.md](v0-spec.md)** | The concrete v0 system design: object schemas, storage backend, CLI commands, and a line-by-line mapping from theory to implementation. Read this to understand *what Morph does*. |

THEORY.md defines the algebra. v0-spec.md projects that algebra into a buildable system. The code implements v0-spec.md. When the spec and theory disagree, the spec wins for v0 (and documents the simplification).

---

## Guides

| Document | Audience | What it covers |
|---|---|---|
| **[CURSOR-SETUP.md](CURSOR-SETUP.md)** | Users | Build, install, configure Morph in Cursor. Record sessions, commit files, debug issues. |
| **[MORPH-AND-GIT.md](MORPH-AND-GIT.md)** | Users | Running Morph and Git side-by-side in the same repository. |
| **[TESTING.md](TESTING.md)** | Contributors | Test architecture, running tests, coverage, known gaps. |

---

## Academic

| Document | What it covers |
|---|---|
| **[morph-paper.tex](morph-paper.tex)** | LaTeX paper formalizing Morph: programs as monadic computations, evaluation contracts, merge monotonicity theorem. Targets academic publication. |

---

## Internal Plans

Historical design documents that guided implementation decisions. Kept for reference; the authoritative state is the code and the documents above.

| Document | Status |
|---|---|
| [plan-blob-store-sqlite.md](plans/plan-blob-store-sqlite.md) | Partially superseded by GixStore |
| [plan-gix-store-option-b.md](plans/plan-gix-store-option-b.md) | Implemented as GixStore (store version 0.2) |
| [plan-morph-viz.md](plans/plan-morph-viz.md) | Implemented as `morph visualize` |

---

## Architecture at a Glance

```
morph-core/     Core library: object model, storage, hashing, commits, metrics, trees
morph-cli/      CLI (read path + manual writes): morph init, add, commit, log, ...
morph-mcp/      Cursor MCP server (primary write path from IDE)
morph-serve/    Browser-based repo visualization (morph visualize)
```

**Storage backend**: Trait-based (`Store`). v0 ships two filesystem implementations (`FsStore` for 0.0, `GixStore` for 0.2+) with identical directory layouts but different hash functions. SQLite and remote backends are anticipated by the trait interface.

**Write path**: Cursor (via MCP) -> morph-mcp -> morph-core -> `.morph/objects/`.
**Read path**: CLI -> morph-core -> `.morph/objects/`.
