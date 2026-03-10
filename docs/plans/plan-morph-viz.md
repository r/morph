# Morph Repo Visualization — Dream & MVP

A browser-based, interactive visualization of a Morph repository: pan, zoom, collapse/expand, open objects. MVP assumes **no branches** (linear history); we can add branch columns and merge curves later.

---

## What We’re Stealing From (Git Visualizations)

### 1. **Gizual** — [gizual/gizual](https://github.com/gizual/gizual), [gizual.com](https://www.gizual.com)

- **Look & feel:** Infinitely zoomable canvas, color-coded by metric, pan/drag, pinch-zoom.
- **Tech:** Browser-based, runs locally; they use Rust + JS (monorepo with apps/packages).
- **Worth copying:** Canvas interaction model (pan/zoom), “infinite” space, performance for large repos. We can adopt the same UX: one big canvas, nodes you can click to expand or open.

### 2. **explain-git-with-d3** — [onlywei/explain-git-with-d3](https://github.com/onlywei/explain-git-with-d3), [live demo](http://onlywei.github.io/explain-git-with-d3/)

- **Look & feel:** Commit nodes as circles, parent→child edges, simple D3 SVG. Very readable.
- **Worth copying:** Minimal commit DAG: nodes + edges. Good reference for “commit as node, line to parent” when we add branches. D3 force or manual layout.

### 3. **DoltHub commit graph** — [blog](https://www.dolthub.com/blog/2024-08-07-drawing-a-commit-graph/), [commit-graph npm](https://www.npmjs.com/package/commit-graph)

- **Algorithm:** Build `childrenMap` from `parents`; assign row = topological order, column = “branch child” vs “merge child” (leftmost branch child, or find empty column for merge). Then draw dots + Bézier curves.
- **Worth copying:** Column/row placement and curve logic when we do multi-branch. For MVP (linear) we only need a single column of commits.

### 4. **CodeFlower** — [majentsch/codeflower](https://github.com/majentsch/codeflower), [demo](https://majentsch.github.io/)

- **Look & feel:** Tree of discs (files/dirs), radius = LOC, click to fold directories, drag to rearrange. D3.
- **Worth copying:** “Click to collapse/expand” and “one node = one navigable thing.” Morph equivalent: commit node → expand to show pipeline / eval contract / runs; pipeline node → expand to show graph.

---

## MVP Scope (No Branches)

- **Data:** Single branch only (e.g. `HEAD` → … → root). Same data as `morph log`: commit hashes + for each commit: message, author, timestamp, `pipeline`, `eval_contract` (suite + observed_metrics), `parents` (0 or 1 in MVP).
- **View 1 — Commit strip:** A vertical (or horizontal) strip of commit nodes, newest at top (or left). Pan along the strip, zoom to see more/fewer commits. Click a commit to “open” it (sidebar or panel) with:
  - message, author, timestamp
  - pipeline hash (link) and optional **mini pipeline graph** (nodes + edges)
  - eval contract: suite hash, observed_metrics (e.g. badges or small table)
- **View 2 — Prompts:** For the commit’s pipeline, show **prompts** (the list of Blob hashes in `pipeline.prompts` and in node `ref`). Each prompt is expandable: **expand to read** the prompt content (the Blob’s `content`, e.g. `content.body` for prompt-kind blobs). So the user can see and read the actual prompt text in the viz.
- **View 3 — Tree browser:** Support **browsing Tree objects**. A Tree has `entries: [{ name, hash }, ...]`. Show entries; click an entry → resolve the hash → show the object (if Blob: show content or “expand to read”; if Tree: show its entries, so you can navigate recursively). Enables “browse the repo” as a hierarchy where Trees exist (e.g. from tools or future staging that builds Trees).
- **View 4 — Optional drill-down:** From a commit, “Open pipeline” shows the Pipeline DAG (prompt_call, tool_call, retrieval, transform, identity nodes and data/control edges) in a small graph (e.g. D3 force or dagre). Collapsible to “just show hash” again.
- **Interactions:** Pan (drag canvas), zoom (wheel or pinch), click commit → detail panel; expand prompts to read; browse trees (expand entries); collapse/expand detail panel. No branch switching in MVP.

Later we can add: multiple branches (column layout + curves à la DoltHub), “click run/trace” from commit, annotations overlay, search by hash/message.

---

## Visual Look We Can Copy

- **Gizual-style:** One canvas, dark or light theme, nodes as circles or rounded cards; zoom gives “infinite” scroll along history. Color could indicate metric (e.g. green = above threshold, grey = no eval) or just neutral.
- **explain-git-with-d3 style:** Simple circles for commits, lines connecting to parent. Clean and minimal.
- **CodeFlower-style:** Collapsible rows/cards: one row per commit, expand to show pipeline + metrics + optional pipeline graph.

A practical MVP look: **vertical commit strip** (like a timeline) with circles or cards; connecting lines (vertical line + optional branch curve later); click opens a **side panel** with commit details and a small **pipeline graph** (nodes + edges). Pan/zoom on the strip. Matches “explain-git” + “Gizual canvas” + “CodeFlower expand.”

---

## Data: Small app pointed at repo (no export)

**Preferred approach:** A small app you point at a Morph repo (e.g. `morph serve /path/to/repo` or run from inside the repo). The app:

- **Reads the repo directly** — opens `.morph/` (object store, refs) via morph-core’s Store interface. No export step; it interrogates the repo at runtime.
- **Serves web pages** — HTML/JS (or a SPA) that you open in a browser to browse commits, expand prompts, and navigate trees.
- **Exposes a thin API** — e.g. `GET /api/log?ref=HEAD`, `GET /api/object/<hash>`. The front end calls these to get commit list and then any object (Commit, Program, Blob, Tree) by hash when you click a commit, expand a prompt, or open a tree entry.

So: no export. You run the app, point it at a repo (or run from repo root), open localhost in the browser, and browse. Everything is read from `.morph/` on demand.

**What the app serves (by reading the store):**

- **Log:** Walk from HEAD (or ref) following parents; return list of commits with `{ hash, message, author, timestamp, pipeline, parents, eval_contract }`.
- **Any object by hash:** `store.get(hash)` → return JSON (Commit, Pipeline, Blob, Tree, Run, etc.). The front end uses this to show pipeline DAG, prompt content (Blob), tree entries (Tree), and so on.

**Optional alternative:** A static export (e.g. `morph viz-export` or `morph log --json` that dumps a single JSON file) still makes sense for “share a snapshot” or “open without running a server,” but the **default and recommended MVP** is the small app that talks to the repo directly.

---

## Tech Stack (Suggestions)

- **Rendering:** Canvas (e.g. PixiJS or raw Canvas 2D) for pan/zoom performance at scale, **or** SVG + D3/React for simpler code and good enough for dozens of commits. MVP: SVG + D3 or plain SVG + pan/zoom (e.g. [d3-zoom](https://github.com/d3/d3-zoom)) is enough.
- **Layout:** Linear strip: one commit per row (or column). No column algorithm needed until we have branches.
- **Pipeline subgraph:** D3 force layout or [dagre](https://github.com/dagrejs/dagre) for pipeline DAG in the detail panel.

---

## Summary

| Aspect | MVP | Later |
|--------|-----|--------|
| Branches | No (linear only) | Yes — columns + Bézier curves (DoltHub-style) |
| Views | Commit strip; detail panel (message, pipeline, eval); **prompts (expand to read)**; **tree browser** | Runs/traces, annotations, search |
| Data | **Small app pointed at repo** — serves pages + API, reads .morph directly (no export) | Optional static export for sharing |
| Look | Steal: Gizual (canvas/pan/zoom) + explain-git (nodes/edges) + CodeFlower (expand/collapse) | Theming, density modes |

We can build the MVP by: (1) a **small app** (e.g. `morph serve` or a separate binary) that takes a repo path (or runs from repo root), uses morph-core’s Store to read `.morph/` directly, and serves static assets + API (`/api/log`, `/api/object/<hash>`), (2) a front end (HTML/JS or SPA) that renders the strip, pan/zoom, detail panel with pipeline graph, prompts (expand to read Blob content), and tree browser (expand Tree entries, fetch by hash), (3) no export step — the app interrogates the repo on demand.
