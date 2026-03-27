# Running Morph and Git Side by Side

## What Morph actually tracks

Morph is not a metadata sidecar for Git. It is a full version control system.

A Morph commit stores the complete file tree — every file in your working directory, content-addressed and snapshotted, exactly like a Git commit. `morph add .` stages files. `morph commit` seals the tree. `morph checkout` restores it. If you've used Git, you already know the mechanics.

The difference is what else is in the commit. A Git commit is a tree hash plus a message. A Morph commit is a tree hash plus a pipeline, an evaluation contract (which tests ran, what scores were achieved), environment constraints, and evidence references to the runs and traces that back up the claim. The file tree is the same; the behavioral record on top of it is what Git cannot represent.

Morph also stores the agent's prompts and responses. Every agent interaction can be recorded as a Run (execution receipt) with a Trace (the full sequence of events — prompts sent, responses received, tool calls made, files edited). These are immutable objects in the same content-addressed store as the file tree.

So Morph tracks three layers:

1. **The codebase** — file tree snapshots, same as Git
2. **The process** — runs and traces recording how the code got there
3. **The outcome** — evaluation scores certifying that it works

In principle, Morph can replace Git entirely. In practice, Morph is new and Git is battle-tested. That's what the rest of this document is about.

## The Early-Days Story

Morph is new. Git is battle-tested. You should use both.

During this period, **Git is your safety net and Morph is the thing you are exercising.** Nothing about your Git workflow changes. You keep staging, committing, pushing, and pulling exactly as you do today. Morph runs in parallel — same working directory, its own dot-directory — recording everything Git records plus the behavioral history Git cannot: which agent wrote which code, what the pipeline did, whether the tests passed, what the merge actually achieved.

The goal is to build confidence in Morph's recording and merge machinery without ever being in a position where you've lost work. If Morph has a bug, your code is still in Git. If you decide Morph isn't for you, `rm -rf .morph/` and you're back where you started.

Here's what the parallel workflow looks like in practice.

### Day one: initialize both

```bash
cd your-project
git init          # if not already a Git repo
morph init        # creates .morph/ alongside .git/
```

Your directory now looks like this:

```
your-project/
  .git/           # Git's objects, refs, config
  .morph/         # Morph's objects, refs, config, runs, traces
  .gitignore
  .morphignore    # same syntax as .gitignore
  src/            # your code — shared working tree
```

Both systems track the same working directory. Neither reads the other's metadata. They are fully independent.

### Daily workflow: do everything twice

This sounds like overhead, but it's mostly automatic if you have the MCP server or hooks set up (see [CURSOR-SETUP.md](CURSOR-SETUP.md) or [CLAUDE-CODE-SETUP.md](CLAUDE-CODE-SETUP.md)). The manual version:

```bash
# You write code (or an agent does), then:

# Git side — exactly what you do today
git add .
git commit -m "add retry logic to auth service"
git push origin main

# Morph side — record the behavioral evidence
morph add .
morph commit -m "add retry logic to auth service" --metrics '{"tests_passed": 42, "tests_total": 42, "pass_rate": 1.0}'
```

Both commits record what the files look like. The Morph commit also records what the pipeline scored. Over time, the Morph history accumulates runs, traces, and evaluation evidence that the Git history simply cannot represent — but both have the full codebase.

You don't need to keep the commit messages in sync. You don't need to commit at the same frequency. Git and Morph commits are independent — make a Git commit whenever you want, make a Morph commit whenever you have behavioral evidence worth recording. In the early days, it's natural for them to roughly correspond, but there's no requirement.

### Branching: align names, keep histories independent

If you're working on a feature branch:

```bash
git checkout -b feature/new-retrieval
morph branch feature/new-retrieval
morph checkout feature/new-retrieval
```

You can align branch names for your own sanity, but Morph never reads Git refs and Git never reads Morph refs. They could have completely different branch structures and nothing would break.

When you merge in Morph, the behavioral gating kicks in — the merged pipeline must dominate both parents on every metric. When you merge in Git, it's the same text reconciliation it's always been. The Morph merge is the one that tells you whether the result actually works.

---

## Should `.morph/` be checked into Git?

**Yes. During the early days, check it in.**

Remove `.morph/` from your `.gitignore`:

```bash
# Edit .gitignore — remove the .morph/ line if present
git add .morph/
git commit -m "track morph metadata in git"
```

Here's why this is the right default for now:

1. **It's your backup.** If Morph has a bug that corrupts its object store, you can recover from Git. This is the entire point of running both systems in parallel during a testing period.

2. **Collaborators get the behavioral history.** When someone clones the repo, they get the full Morph object graph — commits, runs, traces, evaluation evidence. They can run `morph log` and `morph show` without having to reconstruct anything.

3. **It exercises the full round-trip.** Clone → work → morph commit → git commit → push → someone else clones → morph log works. Checking in `.morph/` is what makes this loop testable.

4. **The objects are small.** Morph objects are JSON files, typically a few KB each. In the early days, before you have thousands of runs and traces, the storage overhead is negligible. Git handles it fine.

### When to reconsider

As a project matures and the Morph object store grows (many runs, long traces, large artifacts), you might want to stop checking in everything. At that point, there are two intermediate options:

**Option B — Git only for source, Morph is standalone:**

```gitignore
.morph/
```

Use this when Morph has its own remote (via `morph push`/`morph pull`) and you no longer need Git as a backup for the behavioral history. This is the long-term state.

**Option C — Check in refs and config, ignore the object store:**

```gitignore
.morph/objects/
```

A compromise: Git backs up the branch pointers, config, and small metadata. The bulk of the data (content-addressed objects, runs, traces) stays local or syncs via Morph remotes. Useful when the object store is large but you still want Git to have a record of the Morph branch structure.

### What's the long-term answer?

Morph already stores the full file tree in every commit. It has branching, merging, staging, and checkout. It is architecturally capable of being the only VCS in a project. The reason to keep Git around today is trust, not capability — Git has decades of battle-testing, a mature remote protocol, and an ecosystem of hosting and review tools.

Morph will have its own network remote protocol (today it supports local-path remotes via `morph remote add`; HTTP/SSH transport is planned). Once Morph remotes are mature and the tool has proven itself reliable, a project could drop Git entirely and use Morph as the single system that tracks code, process, and outcomes. Until then, Git is the safety net — and checking `.morph/` into Git is how you get the benefits of both during the transition.

---

## Reference

### Directory layout

```
your-project/
  .git/         # Git's objects, refs, config
  .morph/       # Morph's objects, refs, config, runs, traces
  .gitignore    # Git ignore rules
  .morphignore  # Morph ignore rules (same syntax as .gitignore)
  src/          # shared working tree
```

Keep `.gitignore` and `.morphignore` in sync for shared exclusions (e.g. `target/`, `node_modules/`), or let them differ when you want Morph to track something Git ignores.

### Morph remotes

Morph has its own remote model, independent from Git remotes:

```bash
morph remote add origin /path/to/shared/morph-repo
morph push origin main
morph fetch origin
morph pull origin main
```

Use Git remotes for source code collaboration. Use Morph remotes for behavioral history sync. They are independent.

### Branches

Each system has its own branch namespace. You can align names (`main` in both) for clarity, but Morph never reads Git refs and Git never reads Morph refs.

### CI integration

In CI, clone the Git repo (which includes `.morph/` if you checked it in), then use Morph CLI for behavioral gating:

```yaml
# GitHub Actions example
- name: Run tests
  run: cargo test --workspace 2>&1 | tee test-output.txt

- name: Generate metrics
  run: |
    PASSED=$(grep -c "test .* ok" test-output.txt || echo 0)
    TOTAL=$(grep -c "^test " test-output.txt || echo 0)
    echo "{\"tests_passed\": $PASSED, \"tests_total\": $TOTAL}" > metrics.json

- name: Certify with Morph
  run: morph certify --metrics-file metrics.json --runner github-actions

- name: Gate check
  run: morph gate
```

Both `certify` and `gate` support `--json` for machine-readable output. Gate exit code 0 = pass, 1 = fail.

### Policy

```bash
morph policy set policy.json    # set behavioral policy
morph policy show               # inspect current policy
```

### Hosted service

```bash
morph serve                     # browse Morph state in a browser
```

Complements Git hosting (GitHub, GitLab) — a behavioral evidence dashboard alongside your code review tool.
