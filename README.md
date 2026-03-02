# Morph

**Behavioral version control for AI-assisted development.** Morph extends Git-style content-addressed versioning with execution evidence, behavioral contracts, and merge gating — so you know *how* code was produced and whether it still works.

- **Full docs:** [docs/README.md](docs/README.md) — problem, solution, theory, and spec  
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
