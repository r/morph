# Morph

**Git for AI-assisted development.** Morph is a version control system that tracks not just *what the files are*, but *what produced them and whether it still works*. Every agent session is recorded as an immutable Run with a full Trace (prompts, tool calls, file reads and edits). Every commit can carry a behavioral contract — which evaluation suite was run, what scores were achieved, under which environment. Merge is gated on **behavioral dominance**: the merged code must be at least as good as both parents on every declared metric, not just a clean text diff.

Morph sits alongside Git and uses the same Merkle-DAG foundations. Run both; drop Git later if you want to. See [docs/MORPH-AND-GIT.md](docs/MORPH-AND-GIT.md).

## Why you might want this

If your development loop involves an AI agent, your current tooling probably can't answer:

- **What prompt produced this code?** — every file change links back to the Run that created it.
- **Did the agent's approach actually work?** — commits carry evaluation scores under a declared environment, not just "it looks right."
- **How did this code get to this state?** — Traces capture every prompt, tool call, file read, and edit.
- **Can I safely merge this agent branch?** — merge succeeds only when behavioral dominance is preserved.
- **Can I compare two approaches?** — Runs, Traces, and Pipelines are first-class content-addressed objects. Diff them, annotate them, build on them.

Git assumes identity is byte equality, reproducibility is identical output, and merge is syntactic reconciliation. Those assumptions are fine for handwritten code and break down for probabilistic, effectful, partly-agent-authored code. Morph's answer: make behavior the thing you version.

- **Full docs:** [docs/README.md](docs/README.md) — problem, solution, architecture
- **Theory:** [docs/THEORY.md](docs/THEORY.md) — the algebra (pipelines, certificate vectors, dominance)
- **Spec:** [docs/v0-spec.md](docs/v0-spec.md) — concrete system design, object schemas, CLI reference
- **Paper:** [docs/morph-paper.tex](docs/morph-paper.tex) — formal foundations
- **Install:** [docs/INSTALLATION.md](docs/INSTALLATION.md) · **IDE guides:** [Cursor](docs/CURSOR-SETUP.md) · [Claude Code](docs/CLAUDE-CODE-SETUP.md) · [OpenCode](docs/OPENCODE-SETUP.md)
- **Morph + Git:** [docs/MORPH-AND-GIT.md](docs/MORPH-AND-GIT.md)
- **Multi-machine:** [docs/MULTI-MACHINE.md](docs/MULTI-MACHINE.md) (clients) · [docs/SERVER-SETUP.md](docs/SERVER-SETUP.md) (server) · [docs/MERGE.md](docs/MERGE.md) (merge engine)

## Install and start in Cursor (quick)

Install with Homebrew (recommended for macOS):

```bash
brew tap r/morph
brew install morph
```

This installs both `morph` and `morph-mcp`.

Or build from source:

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

Then open the project in Cursor. Ensure `morph` and `morph-mcp` are on your PATH. The MCP server and hooks will record every prompt/response and let the agent commit via Morph. For Claude Code and OpenCode, see the IDE guides above.

## Core commands

Git-shaped workflow, plus behavioral gating and agent-session recording:

```bash
morph init                       # initialize a morph repo (writes a default policy
                                 #   requiring tests_total + tests_passed; see policy section)
morph status                     # working tree + recent runs + behavioral evidence summary
morph add .                      # stage files
morph commit -m "message"        # create a commit (--pipeline/--eval-suite/--metrics/
                                 #   --from-run <hash>/--allow-empty-metrics/
                                 #   --new-cases id1,id2)
morph log                        # view commit history
morph diff <ref1> <ref2>         # compare two commits/branches
morph show <hash>                # inspect any stored object as pretty JSON
morph branch <name>              # create a branch
morph checkout <ref>             # switch branch or detach to a commit
morph merge <branch> ...         # behavioral merge (dominance required)
morph merge-plan <branch>        # preview merge: parents, union suite, bar, case provenance
morph tag <name>                 # tag the current commit
morph stash save | pop | list    # save/restore staged work
morph revert <hash>              # undo a commit
morph remote add | push | pull   # named remotes (local path or ssh://user@host/path)
morph fetch <remote>             # update remote-tracking refs without merging
morph branch --set-upstream origin/main   # configure per-branch upstream
morph sync [branch]              # fetch + pull --merge against the configured upstream
morph init --bare /srv/repo      # create a bare server repo (for `morph push`)
morph clone <url> [dest]         # one-shot init + remote add + fetch + checkout
morph certify --metrics-file f   # certify a commit against policy metrics
morph gate                       # check if HEAD passes policy (exit 1 on fail)
morph policy init|show|set|require-metrics ...   # manage repository policy
morph upgrade                    # migrate the store to the latest version
morph gc                         # remove unreachable objects
```

### Eval-driven workflow

Morph treats acceptance tests and metric-bearing runs as first-class
objects so behavioral merge gating actually has evidence to compare.
See [docs/EVAL-DRIVEN.md](docs/EVAL-DRIVEN.md) for the full guide.

```bash
morph eval add-case specs/login.yaml         # ingest YAML / Cucumber specs as EvalCases
morph eval suite-from-specs specs/           # bulk-ingest a directory
morph eval suite-show                        # display the registered default suite
morph eval run -- cargo test --workspace     # exec, parse metrics, store a Run linked to HEAD
morph eval from-output --runner pytest f.txt # parse already-captured stdout
morph eval record metrics.json               # ingest a precomputed metrics file
morph eval gaps [--json] [--fail-on-gap]     # report missing behavioral evidence
```

### Default policy on `morph init`

Fresh repos get an opinionated `RepoPolicy` so commits without test
results fail loudly:

```json
{ "required_metrics": ["tests_total", "tests_passed"], "merge_policy": "dominance" }
```

Override per-commit with `--allow-empty-metrics` (or pass `metrics`),
or change the policy globally via `morph policy require-metrics`.

## Recording and inspecting agent work

```bash
morph run record-session --prompt "..." --response "..."   # one-shot record
morph tap summary                 # repo-level overview of recorded runs
morph tap inspect <run-hash>      # grouped steps (prompt, tool calls, files) for one run
morph tap diagnose                # recording-quality report
morph tap export --mode agentic   # export eval cases (for promptfoo, custom harnesses)
morph tap trace-stats <hash>      # per-event payload / kind / length stats
morph tap preview <run-hash>      # labeled prompt/context/response preview
morph traces summary              # newest traces with task phase and target files
morph traces task-structure <ref> # task phase, scope, target files/symbols, goal
morph traces target-context <ref> # the scoped code snippet the agent was working on
morph traces final-artifact <ref> # the final function/file/patch produced
```

IDE hooks parse the agent's full transcript (tool calls, file reads/edits, shell commands, token usage) into structured Trace events — so `tap` and `traces` see real agentic behavior, not just prompt/response pairs.

## Hosted service (team inspection)

```bash
morph serve                              # serve current repo at http://127.0.0.1:8765
morph serve --repo team=/path/to/repo    # named multi-repo mode
morph serve --org-policy org-policy.json # apply org-level policy
```

Stable JSON API and browser UI for inspecting commits (with certification/gate status), runs, traces, pipelines, merge dominance, and policy. See [v0-spec.md § 16](docs/v0-spec.md#16-hosted-service-phase-7).

## Develop Morph (this repo)

```bash
cargo test --workspace                    # 800+ unit + CLI integration tests
cargo test -p morph-e2e --test cucumber   # e2e scenarios (Cucumber)
```

See [docs/TESTING.md](docs/TESTING.md) and [CONTRIBUTING.md](CONTRIBUTING.md).
