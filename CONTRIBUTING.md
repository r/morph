# Contributing to Morph

## Getting started

- **Build and test:** From the repo root, `cargo build` and `cargo test`. See [docs/TESTING.md](docs/TESTING.md) for the full test inventory, coverage, and e2e.
- **Install and use:** [docs/INSTALLATION.md](docs/INSTALLATION.md) covers installing the binaries. [docs/CURSOR-SETUP.md](docs/CURSOR-SETUP.md), [docs/CLAUDE-CODE-SETUP.md](docs/CLAUDE-CODE-SETUP.md), and [docs/OPENCODE-SETUP.md](docs/OPENCODE-SETUP.md) cover IDE integration.

## Code and design

- **Rust:** Keep the workspace warning-clean. See `.cursor/rules/rust-no-warnings.mdc` for project conventions.
- **Design:** The implementation follows [docs/v0-spec.md](docs/v0-spec.md); [docs/THEORY.md](docs/THEORY.md) describes the underlying model.

## Crate structure

| Crate | Role |
|-------|------|
| `morph-core` | Library: object model, storage, hashing, commits, metrics, trees, tap, sync, policy |
| `morph-cli` | CLI: `morph init`, `add`, `commit`, `log`, `tap`, `serve`, ... |
| `morph-mcp` | MCP server: primary write path from IDEs (Cursor, Claude Code, OpenCode) |
| `morph-serve` | Hosted service: `morph serve` (multi-repo JSON API + browser UI) |
| `morph-e2e` | End-to-end tests (Cucumber/Gherkin) |

## Workflow

1. Run tests before and after changes: `cargo test` (and optionally `cargo test -p morph-e2e --test cucumber`).
2. For behavioral commits (if you use Morph in this repo), include metrics from test runs when committing — see `.cursor/rules/behavioral-commits.mdc` and [eval-driven-development](.cursor/rules/eval-driven-development.mdc).

For more detail on tests and known gaps, see [docs/TESTING.md](docs/TESTING.md).
