# Running Morph alongside Git

## What Morph actually tracks

Morph is not a replacement for Git. Morph **wraps** Git: it runs in
the same working directory as your existing git repo, mirrors every
git commit into its own behavioral DAG, and adds the layer git can't
represent — runs, traces, prompts, evaluation contracts, evidence.

A Morph commit is a git commit plus everything else: a tree hash plus
a pipeline, an evaluation contract (which tests ran, what scores were
achieved), environment constraints, and evidence references to the
runs and traces that back up the claim. The file tree is the same
content-addressed snapshot git would see; the behavioral record on
top is what git cannot represent.

So Morph tracks three layers:

1. **The codebase** — file tree snapshots, mirrored from git.
2. **The process** — runs and traces recording how the code got
   there (prompts, tool calls, file edits, shell stdout/stderr,
   model + token usage).
3. **The outcome** — evaluation scores certifying that it works.

Morph does not aspire to replace git. Git is the source-of-truth for
code; Morph is the source-of-truth for *why* the code looks the way
it does.

## Reference mode (the only mode)

Since v0.40.0, Morph runs in **reference mode** unconditionally.
There is no "standalone mode" any more, and `.morph/` is **never
checked into git**.

What this means concretely:

- Every `.morph/` directory in the world sits next to a `.git/` and
  is automatically added to `.git/info/exclude` by `morph init`.
- Teammates pulling from your git remote see ordinary git commits
  and a clean working tree. They do not see your prompts, traces,
  or runs unless they pull from a **morph remote** (a separate,
  opt-in channel — see "Sharing behavioral history" below).
- `morph commit` is a behavioral wrapper around `git commit`: it
  runs `git add -A`, performs the underlying git commit (with
  `--allow-empty --allow-empty-message` so empty behavioral
  checkpoints are legal), and records the metrics / evidence /
  contributors that justify the change.
- `morph branch` and `morph checkout` mirror to git, so the git
  working tree and the morph view of "current branch" never drift.

## Day one

You need a git repo. If you don't have one yet, `morph init` will
prompt you:

```
$ morph init
morph requires a git repository here. Run `git init` for you? [y/N]
```

Pressing Enter (or running non-interactively) exits non-zero with
the recipe `not a git repository at <path>; run \`git init\` first
or pass \`--git-init\` to morph init.`. For scripted setups:

- `morph init --git-init` — always run `git init` first, then
  initialize morph.
- `morph init --no-git-init` — never prompt, never init git; fail
  fast if `.git/` is missing.

Once initialized, your directory looks like this:

```
your-project/
  .git/           # git's objects, refs, config
  .morph/         # morph's objects, refs, config, runs, traces
  .gitignore
  src/            # your code — shared working tree
```

`.morph/` is excluded from git automatically; you don't need to
touch `.gitignore`.

## Daily workflow

You don't do everything twice. Tell Morph what your test suite is
once:

```bash
morph config commit.test_command "cargo test --workspace"
```

After that, the common path is one command:

```bash
# you write code (or an agent does), then:
morph commit -m "add retry logic to auth service"
# running configured test command: cargo test --workspace
# attaching evidence from run a3f2c…: pass_rate=1, tests_passed=42, tests_total=42
git push origin main
```

Behind the scenes `morph commit` runs your configured suite, parses
the metrics, then does the `git add -A` and `git commit` for you and
mirrors a behavioral commit on top of the git commit it just
produced. Both worlds stay in sync without you having to keep two
commit messages aligned. If a test fails, the commit is aborted —
Morph treats a red suite as evidence the code is not in a committable
state. Pass `--no-test` to skip the auto-run for a specific commit
(e.g. a quick chore) and `--rerun` to force a fresh run when the
breadcrumb is stale.

If you want to make a pure git commit (no behavioral evidence,
e.g. for a quick chore), `git commit` directly still works — the
git post-commit hook will mirror it into Morph as a commit with
empty metrics. `morph status` will surface that as a metrics gap
the next time you check.

### Branching

```bash
morph branch feature/new-retrieval
morph checkout feature/new-retrieval
```

These commands drive both Morph's refs and the underlying git
branch / checkout, so the git working tree always matches the
branch you think you're on. Same for `morph merge`.

## Sharing behavioral history (opt-in)

Code goes through your **git remote** (GitHub, GitLab, etc.), as
always. Behavioral history goes through a separate **morph remote**.
Both channels are intentional choices the team makes; neither one
is silent.

```bash
morph remote add team /path/to/shared/morph-repo
morph push team main
morph fetch team
```

This separation exists so the team can have different access
policies on each — e.g. the git remote may be open to contractors
while the morph remote is restricted to full-time staff. The
sharing model and the privacy implications are written up in detail
in [`SECURITY.md`](SECURITY.md), including the `morph forget`
flow that lands tombstones on every configured morph remote.

## Reference

### Directory layout

```
your-project/
  .git/         # git's objects, refs, config
  .morph/       # morph's objects, refs, config, runs, traces
                # — auto-excluded from git via .git/info/exclude
  .gitignore    # git ignore rules
  .morphignore  # optional: morph-only ignore rules (same syntax)
  src/          # shared working tree
```

`.morph/` is **never** tracked by git. If you have an old
Standalone-mode repo where `.morph/` was checked in, run
`morph upgrade` and then:

```bash
git rm -r --cached .morph
git commit -m "stop tracking .morph/"
```

### CI integration

In CI, clone the git repo as usual, then use Morph for behavioral
gating against the freshly produced metrics:

```yaml
- name: Run tests
  run: cargo test --workspace 2>&1 | tee test-output.txt

- name: Generate metrics
  run: |
    PASSED=$(grep -c "test .* ok" test-output.txt || echo 0)
    TOTAL=$(grep -c "^test " test-output.txt || echo 0)
    echo "{\"tests_passed\": $PASSED, \"tests_total\": $TOTAL}" > metrics.json

- name: Certify with morph
  run: morph certify --metrics-file metrics.json --runner github-actions

- name: Gate check
  run: morph gate
```

Both `certify` and `gate` support `--json` for machine-readable
output. Gate exit code `0` = pass, `1` = fail.

If your CI also pushes behavioral history to a team morph remote,
add `morph push team main` after `morph certify`.

### Policy

```bash
morph policy set policy.json    # set behavioral policy
morph policy show               # inspect current policy
```

### Hosted service

```bash
morph serve                     # browse morph state in a browser
```

Complements git hosting (GitHub, GitLab) — a behavioral evidence
dashboard alongside your code review tool.
