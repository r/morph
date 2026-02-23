# Running Morph and Git Side by Side

Morph and Git coexist cleanly in the same project directory. Each uses its own dot-directory and ignores the other.

```
your-project/
  .git/         # Git's objects, refs, config
  .morph/       # Morph's objects, refs, config, runs, traces
  .gitignore    # Git ignore rules
  .morphignore  # Morph ignore rules (same syntax as .gitignore)
  src/           # shared working tree
```

---

## Setup

```bash
git init          # if not already a Git repo
morph init        # creates .morph/ only
```

Keep `.gitignore` and `.morphignore` in sync for shared exclusions (e.g. `target/`, `node_modules/`), or let them differ when you want Morph to track something Git ignores.

---

## What to put in Git

**Option A -- Back up Morph too (recommended)**

Don't add `.morph/` to `.gitignore`. Git backs up your Morph repo along with source. Object store can grow large; use Option C if size is a concern.

**Option B -- Git only for source**

Add `.morph/` to `.gitignore`. Morph state stays local. Restore with `morph init` and re-record.

**Option C -- Back up refs, ignore objects**

```gitignore
.morph/objects/
```

Commit refs, config, and small metadata. Object store stays local.

---

## Workflow

Use **Morph** for behavioral versioning: record runs, create commits (program + eval contract), merge with dominance checks, annotate trace events.

Use **Git** for backup and collaboration: stage, commit, push on your own schedule.

Morph and Git commits are independent. No need to keep them in sync.

---

## Tips

- **Branches**: Each system has its own. You can align names (`main` in both) for clarity, but Morph never reads Git refs.
- **Remotes**: Morph v0 has no remote protocol. Back up `.morph/` via Git or a sync tool.
- **CI**: Clone the Git repo (including `.morph/` if committed), then run Morph CLI against the same tree.
