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

## Develop Morph (this repo)

```bash
cargo test                    # unit + CLI integration tests
cargo test -p morph-e2e --test cucumber   # e2e (Cucumber)
```

See [docs/TESTING.md](docs/TESTING.md) and [CONTRIBUTING.md](CONTRIBUTING.md).
