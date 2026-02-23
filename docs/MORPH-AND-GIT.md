# Running Morph and Git Side by Side

You can use Morph and Git in the **same directory**: Morph now stores file tree snapshots in commits (like Git) in addition to behavioral versions. Morph can serve as a standalone VCS or complement Git for backup and collaboration.

---

## 1. Same repo, two systems

- **Morph** uses only `.morph/` (objects, refs, config, runs, traces, prompts, evals — see [v0-spec.md](v0-spec.md) §3). It does not touch `.git/`.
- **Git** uses `.git/` and the working tree. It does not care about Morph.

Initialize both in the same project:

```bash
cd /path/to/your/project
git init          # if not already a Git repo
morph init
```

You now have:

- `.git/` — Git’s object store and refs
- `.morph/` — Morph’s object store and refs
- Your source files — shared working tree
- **`.morphignore`** — paths Morph excludes from `status` and `add` (same syntax as `.gitignore`; see [v0-spec.md](v0-spec.md) §3)

Since Morph uses only `.morph/`, the separation from Git is even cleaner: each system touches only its own dot-directory. You can keep `.gitignore` and `.morphignore` in sync (e.g. both ignoring `target/`, `node_modules/`) or let them differ if you want Morph to track something Git ignores (or vice versa).

---

## 2. What to put in Git

**Option A – Back up Morph too (recommended for “Morph managed, Git for backup”)**

- **Do not** add `.morph/` to `.gitignore`. Commit it so Git backs up your Morph repo.
- `.morph/objects/` can grow large (many runs, traces, artifacts). If size is a concern, use **Option B** or a partial ignore (see below).

**Option B – Git only for source and config**

- Add to `.gitignore`:

  ```
  .morph/
  ```

  Then Git only versions your source tree and config; Morph state stays local. Restore Morph from scratch with `morph init` and re-record runs as needed.

**Option C – Back up Morph metadata, ignore big objects**

- Back up refs and config so you don’t lose branch/HEAD state, but avoid committing the full object store:

  ```
  .morph/objects/
  ```

  You can still commit `.morph/refs/`, `.morph/config.json`, and optionally `.morph/runs/` or other small dirs. Clone + pull will get refs and config; object store stays local or is restored by re-recording.

---

## 3. Workflow

- **Morph**  
  Use Morph for behavioral versioning: record runs, create commits (program + eval contract), merge, annotate. Commands: `morph run record`, `morph add`, `morph commit`, `morph merge`, etc., or the same via the MCP server in Cursor.

- **Git**  
  Use Git for backup and sharing: when you’re at a good state, stage and commit as usual.

  ```bash
  git add .
  git commit -m "Backup: Morph + source at current state"
  git push
  ```

You can commit to Git after every Morph commit, or on a different schedule (e.g. daily, or when you push to a remote). Morph and Git commits are independent; no need to keep them 1:1.

---

## 4. Tips

- **Branches**  
  Morph and Git each have their own branches. You can keep names aligned (e.g. `main` in both) for clarity, but Morph does not read or write Git refs.

- **Remotes**  
  Git remote (e.g. `origin`) is only for Git. Morph v0 has no remote protocol; backup/distribution of Morph state is done by backing up the directory (e.g. via Git as above).

- **CI**  
  In CI you can clone the Git repo (including `.morph/` if you commit it), then run Morph CLI or MCP clients against the same tree. You can also run only Git in CI and use Morph locally.

- **Large `.morph/objects/`**  
  If the object store is too large for Git, use Option C (ignore `.morph/objects/`) or a separate backup (e.g. artifact store, sync tool) for `.morph/`.
