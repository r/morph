# Reference mode

> **TL;DR.** `morph init --reference` lets you adopt morph in an existing
> git repo without touching the team's git workflow. Morph state lives
> entirely in your local clone; teammates not using morph see nothing.
> When morph and git fall out of step, you get explicit signals — never
> a silent block.

This document explains the contract that makes this work, the two
adoption shapes morph supports, and exactly what morph does (and does
not) do to your git repo.

## Two adoption shapes

When you run `morph init --reference` in a git working tree, you're
opting into one of two shapes:

### Solo mode

Every committer on the repo uses morph. You commit with `morph
commit`, you merge with `morph merge`, and you trust the merge gate
to enforce behavioral dominance. The team has bought in to morph
semantics for branching and merging.

This is what morph is *primarily* designed for, and where the
behavioral version control story is at its strongest.

### Stowaway mode

You install morph because you want to use it for your own workflow,
but the rest of the team is on plain git. They will keep
`git pull`/`git push`/`git rebase`-ing. **Your morph install must not
disrupt that.** This document focuses on what morph guarantees for
this case.

You may move between the two shapes over time — for example, by
introducing morph to one teammate at a time.

## What morph never does to your git repo

In reference mode, morph holds itself to four hard rules:

1. **Nothing morph writes is ever tracked by git.** All morph state
   lives in `.morph/`, which `morph init --reference` adds to
   `.git/info/exclude` (a per-clone, untracked file). A stray
   `git add .` cannot pull morph state into the shared repo.
2. **Hooks live in `.git/hooks/`, which git never tracks.** Teammates
   pulling your branches do not receive the hooks. Their git client
   behaves exactly as it always has.
3. **Hooks always exit zero.** Every hook morph installs ends with
   `>/dev/null 2>&1 || true`. If morph is uninstalled, broken, or
   missing from `PATH`, your `git commit` still succeeds.
   `MORPH_INTERNAL=1` short-circuits the hooks entirely so morph's
   own CLI wrappers (e.g. `morph commit`) cannot recurse into them.
4. **Git commits produced by `morph commit` are byte-identical to
   ones a non-morph user would produce.** The wrapper just runs
   `git commit -m <message>`; the morph-only metadata
   (`morph_origin`, `git_origin_sha`, certification annotations)
   lives in `.morph/objects/` and never leaks into the git tree.

## Lifecycle: what fires and when

Reference mode installs four git hooks. All of them follow the same
shape — a thin shell stub that `exec`s `morph hook <event>` so the
real handler can be upgraded with the binary.

| Hook            | When it fires                      | What morph does                                     |
| --------------- | ---------------------------------- | --------------------------------------------------- |
| `post-commit`   | After every `git commit`           | Mirror new git commit → morph commit (`morph_origin = "git-hook"`) |
| `post-checkout` | After `git checkout <branch>`      | Move morph HEAD to the matching morph branch        |
| `post-rewrite`  | After `git commit --amend`/`rebase` | Mirror new history; mark old morph commits as `rewritten` |
| `post-merge`    | After `git merge` (incl. non-FF `git pull`) | Mirror the new merge commit                  |

`MORPH_INTERNAL=1` suppresses all four hooks. `morph commit` (the
morph→git wrapper) sets it before invoking git so the hook stays out
of its way.

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
before letting them combine; reference-mode commits accumulate that
evidence over time via `morph certify`.

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
   .git/hooks/post-rewrite .git/hooks/post-merge
```

Either is safe. Neither touches anything teammates can observe.

## See also

- `morph init --help` — flags and defaults.
- `morph reference-sync --help` — manual mirror including
  `--backfill`.
- `morph status` — drift and stale-certification surface.
- `morph_eval_gaps` (MCP tool) — structured evidence-gap list,
  including `git_morph_drift`.
