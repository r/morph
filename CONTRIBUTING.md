# Contributing to Morph

## Getting started

- **Build and test:** From the repo root, `cargo build` and `cargo test`. See [docs/TESTING.md](docs/TESTING.md) for test inventory, coverage, and e2e.
- **Install and use:** [docs/INSTALLATION.md](docs/INSTALLATION.md) and [docs/CURSOR-SETUP.md](docs/CURSOR-SETUP.md) describe how to install Morph and use it in Cursor or Claude Code.

## Code and design

- **Rust:** Keep the workspace warning-clean. See `.cursor/rules/rust-no-warnings.mdc` for project conventions.
- **Design:** The implementation follows [docs/v0-spec.md](docs/v0-spec.md); [docs/THEORY.md](docs/THEORY.md) describes the underlying model.

## Workflow

1. Run tests before and after changes: `cargo test` (and optionally `cargo test -p morph-e2e --test cucumber`).
2. For behavioral commits (if you use Morph in this repo), include metrics from test runs when committing — see `.cursor/rules/behavioral-commits.mdc` and [eval-driven-development](.cursor/rules/eval-driven-development.mdc).

For more detail on tests and known gaps, see [docs/TESTING.md](docs/TESTING.md).
