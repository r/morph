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

## Quickstart

Install:

```bash
brew tap r/morph && brew install morph    # macOS, installs `morph` + `morph-mcp`
# or from source:
git clone <morph-repo-url> && cd morph && cargo install --path morph-cli && cargo install --path morph-mcp
```

Then in any project:

```bash
cd /path/to/your/project
morph init
morph commit -m "first morph commit"
```

That's it. `morph init` runs alongside `git init` (it'll prompt to create a git repo if there isn't one yet) and installs a relaxed default policy so commits land immediately. `morph commit` wraps `git commit`, snapshots the working tree, and records the commit on Morph's behavioral DAG.

To see your existing work in Morph terms:

```bash
morph status      # changes + recent runs + behavioral-evidence gaps
morph log         # commit history
morph head        # current HEAD
```

When you want more — recording every agent session, attaching test metrics, gating merges on behavioral dominance — keep reading.

## When you want more

### Record every agent session in your IDE

After `morph init`, drop in IDE hooks so every prompt, tool call, file edit, and response is recorded as an immutable `Run` + `Trace`:

```bash
morph setup cursor       # writes .cursor/ (MCP server, hooks, rules)
morph setup claude-code  # writes .claude/ (MCP server, hooks)
morph setup opencode     # writes opencode.json + .opencode/plugins/
```

Make sure `morph` and `morph-mcp` are on your PATH, then open the project in your IDE. See the [Cursor](docs/CURSOR-SETUP.md), [Claude Code](docs/CLAUDE-CODE-SETUP.md), and [OpenCode](docs/OPENCODE-SETUP.md) guides for the per-IDE detail.

### Tie commits to test results

Tell Morph what your test suite is, once:

```bash
morph config commit.test_command "cargo test --workspace"
```

From then on, plain `morph commit` runs the suite, parses the metrics, and attaches them to the commit:

```bash
morph commit -m "fix retry logic"
# running configured test command: cargo test --workspace
# attaching evidence from run a3f2c…: pass_rate=1, tests_passed=42, tests_total=42
# [d4e5f6a7 (cli)] fix retry logic
```

Escape hatches: `--no-test` skips the run for this commit; `--rerun` forces a fresh run even when the most recent `morph eval run` breadcrumb is still current. If you'd rather drive the run yourself, the two-line form still works (`morph eval run -- cargo test --workspace` then plain `morph commit` picks up the breadcrumb).

See [docs/EVAL-DRIVEN.md](docs/EVAL-DRIVEN.md) for the full spec-first workflow (acceptance cases as YAML, suite gating, case provenance through merges).

### Gate merges on behavioral dominance

When you want CI / your collaborators to enforce "no merge that regresses any tracked metric":

```bash
morph policy init                              # require tests_total + tests_passed on every commit
morph policy require-metrics tests_passed pass_rate    # or pick your own
```

Now `morph merge` rejects any merge whose metrics don't dominate both parents. Preview with `morph merge-plan <branch>`. See [docs/MERGE.md](docs/MERGE.md) for the merge engine in detail.

### Run alongside Git on a real project

Morph never asks Git to step aside: Git owns file storage; Morph stores the behavioral overlay (Runs, Traces, EvalSuites, observed metrics, evidence refs). `.morph/` is auto-excluded from Git so teammates not using Morph see ordinary commits. See [docs/MORPH-AND-GIT.md](docs/MORPH-AND-GIT.md).

```bash
morph init                        # Stowaway (default): passive hooks, no surprises for git-only teammates
morph init --solo                 # Solo submode: pre-merge-commit gate is active
morph install-hooks               # (re-)install the git hooks
morph reference-sync --backfill   # rebuild Morph commits from existing git history
```

### Share behavioral history across machines

Code goes through your git remote (GitHub etc.) as always. Behavioral history goes through a separate, opt-in **morph remote**:

```bash
morph remote add team /srv/morph-repo    # local path or ssh://user@host/path
morph push team main
morph fetch team
morph init --bare /srv/repo              # create a bare server repo
morph clone <url> [dest]                 # one-shot init + remote + fetch + checkout
```

See [docs/MULTI-MACHINE.md](docs/MULTI-MACHINE.md) and [docs/SERVER-SETUP.md](docs/SERVER-SETUP.md).

## Reference: full command list

```bash
morph init [--bare] [--solo] [--git-init|--no-git-init]
morph status [--json]
morph add <paths>
morph commit -m <msg> [--from-run <hash>] [--metrics <json>] [--new-cases ids]
                     [--allow-empty-metrics] [--no-auto-run]
                     [--no-test] [--rerun]    # gate commit.test_command auto-run
morph log [<ref>] [-n N] [--oneline] [--json]
morph diff <old> [<new>] [--json]
morph show <hash|ref>
morph head [--json]
morph identify <ref> [--json]
morph branch [<name>] [--set-upstream <remote>/<branch>] [--json]
morph checkout <ref>
morph merge <branch> [--continue|--abort|resolve-node ...] [--retire metric]
morph merge-plan <branch>
morph rollup <base> <tip> [-m <msg>]
morph tag [<name>] [-d]
morph stash save|pop|list
morph revert <hash>

morph clone <url> [<dest>] [--branch B] [--bare]
morph remote add <name> <url> | list [--json]
morph push <remote> <branch>
morph fetch <remote>
morph pull <remote> <branch> [--merge]
morph sync [<branch>]

morph eval add-case <files>... | suite-from-specs <dirs>... | suite-show [--suite H] [--json]
morph eval run -- <cmd>... | from-output [--runner R] [--record] <file> | record <file.json>
morph eval gaps [--json] [--fail-on-gap]
morph policy init|show|set|require-metrics|set-default-eval ...
morph certify --metrics(-file) ... [--commit <hash>] [--runner ...]
morph gate [--commit <hash>] [--json]

morph trace show <hash>
morph tap summary | inspect <hash> | diagnose | export | trace-stats <hash> | preview <hash>
morph traces summary | task-structure <ref> | target-context <ref> | final-artifact <ref>
                    | semantics <ref> | verification <ref>

morph annotate <hash> -k kind -d data
morph annotations <hash> [--json]
morph refs [--json]
morph forget <hash> [--reason ...] [--remote <name>] [--dry-run] [--yes]
morph upgrade
morph gc

morph setup cursor|opencode|claude-code|aoe ...
morph serve [--repo name=path]... [--port N] [--org-policy file]
morph visualize [<path>] [--port N]
```

`morph --help` prints the same list grouped by purpose.

## Inspecting recorded agent work

The IDE hooks parse the agent's full transcript (tool calls, file reads/edits, shell commands, token usage) into structured Trace events. Browse them via:

- `morph tap summary` — repo-level overview of recorded runs.
- `morph tap inspect <run-hash>` — grouped steps for one run.
- `morph traces summary` / `traces task-structure <ref>` / `traces final-artifact <ref>` — structured task views for replay or eval generation.
- `morph run record-session --prompt "..." --response "..."` — record a session manually.

See [docs/SESSION-TRACKING.md](docs/SESSION-TRACKING.md).

## Hosted service (team inspection)

```bash
morph serve                              # serve current repo at http://127.0.0.1:8765
morph serve --repo team=/path/to/repo    # named multi-repo mode
morph serve --org-policy org-policy.json # apply org-level policy
```

Stable JSON API and browser UI for inspecting commits (with certification/gate status), runs, traces, pipelines, merge dominance, and policy. See [v0-spec.md § 16](docs/v0-spec.md#16-hosted-service-phase-7).

## Develop Morph (this repo)

```bash
cargo test --workspace                    # ~1187 unit + YAML acceptance tests
cargo test -p morph-e2e --test cucumber   # end-to-end Cucumber scenarios
```

See [docs/TESTING.md](docs/TESTING.md) for the test inventory, layout, and
the spec-first development loop, and [CONTRIBUTING.md](CONTRIBUTING.md) for
contributor workflow.
