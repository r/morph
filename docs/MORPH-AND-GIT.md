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

Use **Morph** for behavioral versioning: record runs, create commits (pipeline + eval contract), merge with dominance checks, annotate trace events.

Use **Git** for backup and collaboration: stage, commit, push on your own schedule.

Morph and Git commits are independent. No need to keep them in sync.

---

## Morph Remotes

Morph now has its own remote protocol (Phase 5). A Morph remote is another `.morph/` repository reachable via filesystem path.

```bash
morph remote add origin /path/to/shared/morph-repo
morph push origin main          # push Morph history to remote
morph fetch origin               # fetch remote branches
morph pull origin main           # fast-forward local branch
```

Morph remotes are independent from Git remotes. You can use both:

- **Git remotes** for source code collaboration (push/pull `.git/`)
- **Morph remotes** for behavioral history sync (push/pull `.morph/`)

---

---

## CI Integration (Phase 6)

Morph integrates with standard CI/CD pipelines through certification and gating commands. Morph does not run tests — external tools do. Morph validates, records, and gates on the results.

### Canonical Git + Morph + CI Workflow

```
1. Developer works locally
   └─ morph init / morph commit / morph run record-session
   └─ git add / git commit / git push

2. CI pipeline triggers on push/PR
   └─ git clone (includes .morph/ if committed)
   └─ run tests, evaluations, benchmarks
   └─ write results to metrics.json

3. CI certifies the Morph commit
   └─ morph certify --metrics-file metrics.json --runner github-actions

4. CI gates the candidate
   └─ morph gate
   └─ exit code 0 = pass, 1 = fail (blocks merge)

5. Team reviews
   └─ morph log / morph show <hash>
   └─ inspect behavioral evidence before approving PR
```

### Setting Up a Project Policy

```bash
# Create a policy file
cat > policy.json << 'EOF'
{
  "required_metrics": ["tests_passed", "pass_rate"],
  "thresholds": { "pass_rate": 0.95 },
  "merge_policy": "dominance"
}
EOF

# Apply it to the repo
morph policy set policy.json

# Verify
morph policy show
```

### CI Script Example (GitHub Actions)

```yaml
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

### JSON Output for Automation

Both `certify` and `gate` support `--json` for machine-readable output:

```bash
morph certify --metrics-file metrics.json --json
morph gate --json
```

---

## Tips

- **Branches**: Each system has its own. You can align names (`main` in both) for clarity, but Morph never reads Git refs.
- **Remotes**: Morph has its own remote model. Use `morph remote add` to configure Morph remotes, and Git remotes for source. They are independent.
- **CI**: Clone the Git repo (including `.morph/` if committed), then run Morph CLI against the same tree. Use `morph certify` and `morph gate` for behavioral gating.
