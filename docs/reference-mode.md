# Reference mode

> **TL;DR.** `morph init --reference` lets you adopt morph in an existing
> git repo without touching the team's git workflow. Morph state lives
> entirely in your local clone; teammates not using morph see nothing.
> When morph and git fall out of step, you get explicit signals — never
> a silent block.

This document explains the contract that makes this work, the two
adoption shapes morph supports, and exactly what morph does (and does
not) do to your git repo.

## Two adoption shapes (submodes)

When you run `morph init --reference` in a git working tree, you're
opting into one of two **submodes**, recorded in
`.morph/config.json` as `repo_submode`. The submode is local to the
clone — it never travels with git, so a teammate flipping their own
clone to Solo cannot surprise anyone else.

### Stowaway submode (default)

You install morph because you want to use it for your own workflow,
but the rest of the team is on plain git. They will keep
`git pull`/`git push`/`git rebase`-ing. **Your morph install must not
disrupt that.** Stowaway is the default for `morph init --reference`
and installs only the four passive observer hooks below.

```sh
$ morph init --reference .       # Stowaway is the default
```

### Solo submode (opt-in)

Every committer on the repo uses morph. You commit with `morph
commit`, you merge with `morph merge`, and the merge gate is
authoritative for both — including for plain `git merge`, which
Solo's `pre-merge-commit` hook gates against the same dominance
contract. The team has bought in to morph semantics for branching
and merging.

```sh
$ morph init --reference --solo .   # opt into Solo
$ morph install-hooks --solo        # flip an existing repo to Solo
$ morph install-hooks --stowaway    # flip back
```

You may move between the two submodes over time — for example, by
adopting morph one teammate at a time and only flipping your own
clone to Solo once everyone has it installed.

### Hooks by submode

The two submodes differ in exactly one hook:

| Hook                | When it fires                               | Stowaway | Solo |
| ------------------- | ------------------------------------------- | :------: | :--: |
| `post-commit`       | After every `git commit`                    |    ✓     |  ✓   |
| `post-checkout`     | After `git checkout <branch>`               |    ✓     |  ✓   |
| `post-rewrite`      | After `git commit --amend`/`rebase`         |    ✓     |  ✓   |
| `post-merge`        | After `git merge` (incl. non-FF `git pull`) |    ✓     |  ✓   |
| `pre-merge-commit`  | **Before** `git merge` records its commit   |          |  ✓   |

The first four are passive: they observe git, mirror into morph, and
never fail. The fifth — Solo only — is active: it runs the
dominance gate on the *worse-of-parents bar* and aborts the merge
when the resulting commit would regress on a parent's certified
metrics.

Two environment-variable escapes are honored:

- `MORPH_INTERNAL=1` — short-circuits *every* morph hook, including
  `pre-merge-commit`. `morph merge` and `morph commit` set this when
  they shell out to git so the wrapper's own gate runs once, not
  twice.
- `MORPH_NO_GATE=1` — Solo-only. Lets a single `git merge` through
  with a stderr warning. Use this for emergency merges where the
  human has explicitly accepted the regression. Subsequent merges
  are gated again.

## What morph never does to your git repo

In reference mode, morph holds itself to four hard rules:

1. **Nothing morph writes is ever tracked by git.** All morph state
   lives in `.morph/`, which `morph init --reference` adds to
   `.git/info/exclude` (a per-clone, untracked file). A stray
   `git add .` cannot pull morph state into the shared repo.
2. **Hooks live in `.git/hooks/`, which git never tracks.** Teammates
   pulling your branches do not receive the hooks. Their git client
   behaves exactly as it always has.
3. **Passive hooks (Stowaway) always exit zero.** The `post-commit`,
   `post-merge`, `post-checkout`, and `post-rewrite` hooks end with
   `>/dev/null 2>&1 || true`. If morph is uninstalled, broken, or
   missing from `PATH`, your `git commit` / `git merge` / `git
   checkout` still succeeds. `MORPH_INTERNAL=1` (and
   `MORPH_NO_GATE=1`) short-circuit the hooks entirely so morph's
   own CLI wrappers (e.g. `morph commit`) cannot recurse into them.

   The one exception is **Solo's `pre-merge-commit` hook**: it
   intentionally *can* exit non-zero so a `git merge` that would
   regress on a parent's certified metrics is blocked. Solo is
   opt-in (`morph init --reference --solo`); choose it only when
   every developer on the project uses morph and you want the
   behavioral gate enforced at git-time.
4. **Git commits produced by `morph commit` are byte-identical to
   ones a non-morph user would produce.** The wrapper just runs
   `git commit -m <message>`; the morph-only metadata
   (`morph_origin`, `git_origin_sha`, certification annotations)
   lives in `.morph/objects/` and never leaks into the git tree.

## Lifecycle: what each hook does

All hooks are thin shell stubs that `exec morph hook <event>` so the
real handler can be upgraded with the binary. The submode table
above shows which hooks each submode installs; the per-hook
behavior is:

| Hook                | What morph does                                                                                  |
| ------------------- | ------------------------------------------------------------------------------------------------ |
| `post-commit`       | Mirror new git commit → morph commit (`morph_origin = "git-hook"`).                              |
| `post-checkout`     | Move morph HEAD to the matching morph branch.                                                    |
| `post-rewrite`      | Mirror new history; mark old morph commits as `rewritten` and link to their successors.         |
| `post-merge`        | Mirror the new merge commit (origin `"git-hook"`).                                               |
| `pre-merge-commit`  | **(Solo only)** Run the merge dominance gate against the parents' effective metrics; exit 1 on regression. "No claim" parents pass with a warning. |

## Drift: when morph runs behind git

Some git operations fire **no hook**:

- `git pull --ff-only` (a fast-forward ref update with no merge commit).
- `git fetch` followed by `git reset --hard origin/main`.
- Direct ref manipulation (`git update-ref`).

When that happens, morph stays pinned to whatever it last saw. This
is the **drift** state. It is a normal, expected thing — not an
error. Morph surfaces it explicitly:

```text
$ morph status
...
Reference mode (git ↔ morph)
  git HEAD:        a1b2c3d4e5f6
  drift:           3 unmirrored git commits — run `morph reference-sync`
  last mirrored:   9f8e7d6c5b4a
```

To resolve drift, run `morph reference-sync`. It walks git's first-
parent chain from HEAD back to the last mirrored commit and mirrors
each one. This is idempotent — running it twice is a no-op when
already in sync. For late adoption (a long pre-existing history), use
`morph reference-sync --backfill` instead.

The MCP tool `morph_eval_gaps` reports drift as a `git_morph_drift`
entry, so agents can detect it programmatically:

```json
{ "kind": "git_morph_drift", "unmirrored_count": 3, "hint": "..." }
```

## Stale certifications

`morph certify` attaches a `kind: "certification"` annotation to a
specific morph commit. If `git commit --amend` or `git rebase` later
rewrites that commit, the certification is now describing
*superseded* code.

Morph never mutates the original certification annotation (object
immutability is a load-bearing property). Instead, the post-rewrite
hook attaches a `kind: "rewritten"` annotation to the old morph
commit, pointing at its successor. Status surfaces both:

```text
$ morph status
...
Reference mode (git ↔ morph)
  ...
  stale certification: 1 (a rewritten commit had certification evidence — re-certify the successor)
```

The successor commit can then be re-certified with `morph certify
--commit <new-hash> --metrics ...`. The chain is preserved in the
object graph for audit.

## What `morph commit` does in reference mode

When you run `morph commit -m <msg>` in a reference-mode repo, the
wrapper:

1. Resolves observed metrics (from `--metrics`, `--from-run`, or the
   `LAST_RUN.json` breadcrumb left by `morph eval run`).
2. Enforces `policy.required_metrics` **before** invoking git, so a
   policy reject never leaves a stranded git commit.
3. Runs `git commit -m <msg>` with `MORPH_INTERNAL=1`.
4. Mirrors the new git HEAD into morph with `morph_origin = "cli"`
   (distinct from passive hook mirrors, so the merge gate can tell
   them apart).
5. If metrics were supplied, attaches them as a `kind:
   "certification"` annotation on the new morph commit.

The git side is normal. Teammates pulling your branch see ordinary
git commits.

`--allow-empty-commit` maps to `git commit --allow-empty` for
audit-only commits (e.g. a certification milestone with no diff).

## What `morph merge` does in reference mode

`morph merge <branch>` is the **canonical merge driver** in
reference mode. It wraps `git merge` end-to-end: it auto-mirrors,
runs the dominance gate, drives `git merge`, and mirrors the result
back into morph. Plain `git merge` keeps working unchanged for
teammates not using morph; in Solo submode the `pre-merge-commit`
hook applies the same gate to plain `git merge` as well.

### Step-by-step

1. **Auto-mirror the current branch.** `git HEAD` may be ahead of
   morph (you committed via plain `git commit` with the hook
   suppressed, or pulled a fast-forward). Morph runs `sync_to_head`
   so the gate compares against your *actual* current state, not a
   stale mirror.
2. **Auto-mirror the merge target.** If `refs/heads/<branch>` exists
   in git but morph hasn't seen it (or is behind), morph mirrors the
   missing commits via `ensure_branch_synced`. This is what makes
   `morph merge feature` work in Stowaway submode where teammates'
   branches arrive via `git fetch` / `git pull` without ever firing
   a morph hook.
3. **Run the dominance gate.** If you supplied `--metrics` (or
   `--from-run`), morph checks them against each parent's effective
   metrics *before* touching git. A doomed merge therefore never
   produces a stranded git commit.
4. **Drive `git merge`.** Morph shells out to `git merge` with
   `MORPH_INTERNAL=1` set so its own hooks stay out of the way. The
   git side does its normal thing: fast-forward, merge commit, or
   conflict.
5. **Mirror the outcome.** On fast-forward / clean merge, morph
   mirrors the new git HEAD with `morph_origin = "cli"` so the
   merge gate can later distinguish it from passive hook mirrors.
6. **Attach certification.** If `--metrics` were supplied and the
   gate passed, the resulting morph commit gets a
   `kind: "certification"` annotation in the same step.

Both auto-mirror steps print explicit messaging when work was
performed:

```text
$ morph merge feature
morph: auto-mirroring 'feature' from git into morph (3 new commits, tip 7c91a2b)
morph: no morph evidence on 'feature' — merge proceeds without behavioral assertion from this side
```

**A parent with no morph evidence (no observed metrics, no
certifications) yields no violations from that side** — there is
nothing to dominate. Morph warns explicitly so you know the merge
gate had nothing to enforce on that side. This is the "no morph
claim" principle: absence of evidence is not absence of permission.

If you want stricter behavior, run `morph certify` on each parent
*before* the merge so the gate actually has metrics to compare. In
Stowaway submode this is rare; in Solo submode it's the default
flow — and the `pre-merge-commit` hook backs it up for plain
`git merge`.

### Stateful flow: conflicts, `--continue`, `--abort`

When step 4 produces a git conflict, morph keeps the merge
*in progress* (the same way `git merge` does) and writes a
breadcrumb at `.morph/MERGE_REF.json`:

```json
{
  "other_branch": "feature",
  "other_git_sha": "7c91a2b…",
  "head_git_sha": "a1b2c3…",
  "message": "Merge branch 'feature'"
}
```

`morph status` surfaces this state explicitly:

```text
$ morph status
...
Reference mode (git ↔ morph)
  merge in progress: resolve conflicts and run `morph merge --continue`
                     (or `morph merge --abort`)
```

Once you've resolved the conflicts and `git add`-ed the files:

```sh
$ morph merge --continue
```

This runs `git ls-files --unmerged` to verify nothing is still
unresolved, calls `git commit -m "<saved message>"` under
`MORPH_INTERNAL=1`, mirrors the new merge commit into morph
(`morph_origin = "cli"`), optionally attaches a certification if
`--metrics` was passed, and clears the breadcrumb. If unmerged
paths remain, `--continue` exits 1 with the list.

To bail out instead:

```sh
$ morph merge --abort
```

This runs `git merge --abort` under `MORPH_INTERNAL=1` and clears
the breadcrumb. No morph commit is created (none ever was), so
there is nothing to roll back on the morph side. `--abort` is
idempotent: running it without an in-progress merge is a no-op.

### Migrating Stowaway → Solo

If you started in Stowaway and the rest of the team has now
adopted morph, you can flip a single clone to Solo without
touching git or other clones:

```sh
$ morph install-hooks --solo
hooks installed: post-commit, post-checkout, post-merge, post-rewrite, pre-merge-commit
config: repo_submode = solo
```

This installs the missing `pre-merge-commit` hook. From this point
on, plain `git merge` on this clone is also gated. Going back is
symmetric:

```sh
$ morph install-hooks --stowaway
hooks removed:   pre-merge-commit
config: repo_submode = stowaway
```

Because submode is local to the clone, no teammate ever sees a
sudden merge gate — they get one only if they explicitly opt in
on their own clone.

## Policy carve-outs

The default reference-mode policy includes
`exempt_origins = ["git-hook"]`. This means:

- Commits made by `morph commit` (origin `"cli"`) **must** satisfy
  `required_metrics` to pass `morph gate`.
- Commits mirrored by the post-commit hook (origin `"git-hook"`) are
  exempt — they were created passively, before the user had a chance
  to certify them.

This carve-out only applies to the manual `morph gate` check.
The merge gate (`morph merge`) still requires evidence on each parent
when both have claims; the "no morph evidence" path above describes
what happens when one or both don't.

## Worked example: Stowaway end-to-end

A complete day-in-the-life trace of a single morph user in a repo
where everyone else is on plain git.

```sh
# Day 1: adopt morph in an existing repo.
$ git pull
$ morph init --reference .
Initialized morph reference-mode repository at .
  - .morph/ added to .git/info/exclude (local to this clone, never tracked)
  - 4 git hooks installed in .git/hooks/ (Stowaway submode)

# Run your first eval and certify.
$ morph eval run -- cargo test --workspace
9abe1f67…
$ morph certify --commit HEAD --metrics '{"tests_passed":42,"tests_total":42,"pass_rate":1.0}'

# Day 2: teammate pushed to feature; pull and merge.
$ git fetch origin
$ git checkout main
$ git pull origin main      # fast-forward; no hooks fire
$ morph status
... drift: 5 unmirrored git commits — run `morph reference-sync`
$ morph reference-sync
Mirrored 5 commits.

# Now merge teammate's branch. Auto-mirror does the rest.
$ morph merge origin/feature -p <pipeline-hash> --metrics '{...}' -m "merge feature"
morph: auto-mirroring 'origin/feature' from git into morph (3 new commits, tip 7c91a2b)
morph: no morph evidence on 'origin/feature' — merge proceeds without behavioral assertion from this side
<merge-commit-hash>

# Day 3: amend a commit; certifications get flagged stale.
$ git commit --amend -m "tweak message"
$ morph status
... stale certification: 1 (a rewritten commit had certification evidence — re-certify the successor)
$ morph certify --commit HEAD --metrics '{"tests_passed":42,"tests_total":42,"pass_rate":1.0}'
```

## What you give up in Stowaway mode

- **No team-wide merge gating.** The merge gate only protects merges
  *you* perform with `morph merge`. A teammate doing `git merge` on
  another machine bypasses it (their morph isn't installed; they
  don't have the gate).
- **Partial evidence.** Commits made by teammates have no morph
  certifications until you (or your CI) attach them later.
- **You are the source of truth.** When morph and a teammate
  disagree (e.g. a teammate rebased a branch you'd certified), morph
  flags the certification as stale and you decide what to do.

Solo mode does not have these gaps because every commit goes through
morph's wrappers.

## Recovery and reset

If you want to wipe morph state and start over without affecting
git:

```sh
rm -rf .morph
morph init --reference .
```

If you want to remove the morph hooks without removing morph entirely:

```sh
rm .git/hooks/post-commit .git/hooks/post-checkout \
   .git/hooks/post-rewrite .git/hooks/post-merge \
   .git/hooks/pre-merge-commit  # only present in Solo submode
```

Either is safe. Neither touches anything teammates can observe.

## See also

- `morph init --help` — flags and defaults (`--reference`, `--solo`).
- `morph install-hooks --help` — flip submode (`--solo` /
  `--stowaway`) without re-initializing.
- `morph merge --help` — `--continue` / `--abort` flags for stateful
  conflict resolution.
- `morph reference-sync --help` — manual mirror including
  `--backfill`.
- `morph status` — drift, stale-certification, and merge-in-progress
  surface.
- `morph_eval_gaps` (MCP tool) — structured evidence-gap list,
  including `git_morph_drift`.
