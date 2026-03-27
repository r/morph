# Morph

**Version control when pipelines are probabilistic.** Morph extends Git's content-addressed Merkle DAG with pipelines (the sequences of prompt calls, tool invocations, retrieval steps, and transforms that make up an LLM application), evaluation suites (versioned definitions of what "good" means), and runs (permanent execution receipts recording exactly what ran, in what environment, and what it produced). A Morph commit bundles a pipeline with an evaluation suite and scores. At merge time, Morph records the scores from both parents and the scores the merged pipeline achieved.

- **Full docs:** [docs/README.md](docs/README.md) — problem, solution, theory, and spec  
- **Paper:** [docs/morph-paper.tex](docs/morph-paper.tex) — formal foundations  
- **Install & run:** [docs/INSTALLATION.md](docs/INSTALLATION.md) — binaries, init, IDE (Cursor / Claude Code)  
- **Cursor from scratch:** [docs/CURSOR-SETUP.md](docs/CURSOR-SETUP.md) — MCP, hooks, rules, committing  

## Install and start in Cursor (quick)

```bash
git clone <morph-repo-url> && cd morph
cargo install --path morph-cli && cargo install --path morph-mcp
```

In your **project** (not the morph repo):

```bash
cd /path/to/your/project
morph init
morph setup cursor   # writes .cursor/ (MCP, hooks, rules)
```

Then open the project in Cursor. Ensure `morph` and `morph-mcp` are on your PATH. The MCP server and hooks will record prompts/responses and let the agent commit via Morph.

## Hosted service (team inspection)

Run the Morph hosted service for shared, browser-based inspection of behavioral history:

```bash
morph serve                              # serve current repo at http://127.0.0.1:8765
morph serve --repo team=/path/to/repo    # named multi-repo mode
morph serve --org-policy org-policy.json # apply org-level policy
```

The service exposes a stable JSON API and browser UI for inspecting commits (with certification/gate status), runs, traces, pipelines, merge dominance, and policy. See [v0-spec.md § 15](docs/v0-spec.md#15-hosted-service-phase-7).

## Develop Morph (this repo)

```bash
cargo test                    # unit + CLI integration tests
cargo test -p morph-e2e --test cucumber   # e2e (Cucumber)
```

See [docs/TESTING.md](docs/TESTING.md) and [CONTRIBUTING.md](CONTRIBUTING.md).
