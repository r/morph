# Phase 1 Agent Prompt: Morph Reliability Baseline

## Objective

Improve Morph's reliability on the highest-risk user workflows by adding executable tests and fixing any bugs they uncover. When you are done, Morph's `branch`, `checkout`, `merge`, `rollup`, and `upgrade` commands—and their error paths—must be covered by real CLI integration tests and end-to-end scenarios.

---

## Repository Layout

```
morph/
├── morph-core/src/          # Core library (Rust). Each module has #[cfg(test)] mod tests.
│   ├── commit.rs            # create_commit, create_tree_commit, create_merge_commit*,
│   │                        #   rollup, checkout_tree, resolve_head, current_branch,
│   │                        #   set_head_branch, set_head_detached, log_from
│   ├── metrics.rs           # aggregate, check_thresholds, check_dominance,
│   │                        #   check_dominance_with_suite, union_suites, retire_metrics
│   ├── store.rs             # FsStore, GixStore, Store trait
│   ├── tree.rs              # build_tree, flatten_tree, restore_tree
│   ├── index.rs             # read_index, write_index, clear_index
│   ├── migrate.rs           # migrate_0_0_to_0_2, migrate_0_2_to_0_3
│   └── ...
├── morph-cli/
│   ├── src/main.rs          # CLI entry point (clap). All commands defined here.
│   ├── build.rs             # Compiles YAML specs into Rust test functions.
│   └── tests/
│       ├── specs/*.yaml     # YAML integration specs (one file per command group).
│       └── spec_tests.rs    # include!(concat!(env!("OUT_DIR"), "/spec_tests.rs"));
├── morph-e2e/
│   ├── features/*.feature   # Gherkin scenarios (end-to-end, multi-step).
│   └── tests/cucumber.rs    # Step definitions for Gherkin scenarios.
└── docs/
    └── TESTING.md           # Test-contract source of truth.
```

---

## What Exists Today

### CLI YAML spec tests (morph-cli/tests/specs/)
Covered: `init`, `status`, `add`, `prompt`, `pipeline`, `commit`, `record_session`, `run_eval`, `annotate`.

**Not covered (your job):** `branch`, `checkout`, `merge`, `rollup`, `upgrade`, plus error cases for all of the above.

### E2E Gherkin features (morph-e2e/features/)
Covered: `init_and_status`, `add_and_commit`, `run_record_session`, `concurrent_agents`, `prompt_create_materialize`.

**Not covered (your job):** Multi-step workflows that exercise branching, checkout with tree restore, merge with dominance gating, rollup, and upgrade.

### Unit tests (morph-core)
`commit.rs` has 9 tests. `metrics.rs` has 20+ tests. Both have good coverage of happy paths but limited error-path coverage.

---

## Deliverables

Complete ALL of the following. Do not skip any section.

### 1. YAML Spec Tests — `morph-cli/tests/specs/`

Create these new YAML spec files. Follow the exact format used by existing specs (see below for reference). Each file is an array of test specs.

#### `branch.yaml`

| Test name | What it does |
|-----------|-------------|
| `branch_create` | Make a commit, run `morph branch feature`, assert `heads/feature` file exists, run `morph branch` (list) and assert output contains both `main` and `feature`. |
| `branch_list_shows_current` | Make a commit, run `morph branch` and assert output contains `* main` (the asterisk marks current). |
| `branch_no_commit_fails` | On a fresh init (no commits), run `morph branch feature` and expect exit code 1 with stderr containing an error message about no commits. |
| `branch_create_already_exists` | Make a commit, create branch `feature`, then try `morph branch feature` again. Expect the ref to be updated (or an error—test whichever the CLI actually does and document the behavior). |

#### `checkout.yaml`

| Test name | What it does |
|-----------|-------------|
| `checkout_branch` | Commit a file on `main`, create branch `feature`, commit a different file on `feature`, checkout `main`. Assert stdout contains "Switched to branch main". |
| `checkout_restores_tree` | Commit `a.txt` on `main`, create branch `feature`, switch to `feature`, add and commit `b.txt`, switch back to `main`. Assert `b.txt` does NOT exist in the working directory (tree was restored from main's commit). |
| `checkout_detached` | Make a commit, capture the hash, run `morph checkout <hash>`. Assert stdout contains "Detached HEAD". |
| `checkout_nonexistent_fails` | Run `morph checkout nosuchbranch` and expect exit code 1 with stderr containing an error. |

#### `merge.yaml`

| Test name | What it does |
|-----------|-------------|
| `merge_happy_path` | Create two branches with different commits and metrics. Merge the feature branch into main with metrics that dominate both parents. Assert success, assert `morph log` shows "merge" message, assert the merge commit hash is 64 chars. |
| `merge_rejects_weak_metrics` | Same setup as above but provide merged metrics that do NOT dominate one parent. Expect exit code 1, stderr contains "rejected" or "dominate". |
| `merge_nonexistent_branch_fails` | Run `morph merge nosuchbranch ...` and expect exit code 1. |

#### `rollup.yaml`

| Test name | What it does |
|-----------|-------------|
| `rollup_squashes_commits` | Make 3 commits on main, save the first commit hash as `base`. Run `morph rollup <base> HEAD -m "squashed"`. Assert success, assert log shows "squashed", assert the rollup commit hash is 64 chars. |
| `rollup_nonexistent_ref_fails` | Run `morph rollup nosuchref HEAD` and expect exit code 1. |

#### `upgrade.yaml`

| Test name | What it does |
|-----------|-------------|
| `upgrade_latest_no_op` | Fresh init (already at latest version). Run `morph upgrade`. Assert stdout contains "No upgrade needed" or "latest". |
| `upgrade_from_repo_root` | Fresh init, run `morph upgrade`. Expect success (exit 0). |

#### `errors.yaml` (cross-cutting error cases)

| Test name | What it does |
|-----------|-------------|
| `commit_no_staged_files` | Fresh init, no files added, run `morph commit -m "empty"`. The behavior should be defined—either it succeeds with an empty tree or fails with a clear error. Test whichever the CLI actually does. |
| `log_no_commits` | Fresh init, run `morph log`. Expect success with empty output (no commits to show). |
| `status_outside_repo` | Do NOT init. Run `morph status` in a temp dir. Expect exit code 1, stderr contains "not a morph repository". |

### YAML Spec Format Reference

Every YAML spec file is a YAML array. Each element has this shape:

```yaml
- name: test_function_name          # snake_case, unique across all spec files
  init: true                         # default true; set false to skip auto morph init
  dirs:                              # optional: directories to create before steps
    - src
  files:                             # optional: files to create before steps
    foo.txt: "content"
    src/bar.rs: "fn main() {}"
  steps:
    - morph: [commit, -m, "msg"]     # CLI args as YAML array
      capture: var_name              # capture full stdout (trimmed) into variable
      capture_first_line: var_name   # capture only first line of stdout
      assert_hash: true              # assert captured value is 64 hex chars
      expect_exit: 1                 # expected exit code (default 0 = success)
      stdout_contains: "text"        # string or array of strings
      stdout_not_contains: "text"    # string or array of strings
      stderr_contains: "text"        # string or array of strings
    - compute_hash:                  # compute content hash of a JSON string
        var: trace_hash
        json: '{"type":"trace",...}'
    - write_file:                    # write a file mid-test (supports ${var} interpolation)
        path: run.json
        content: '...'
  assert:                            # post-step filesystem assertions
    - kind: dir_exists
      path: .morph
    - kind: file_exists
      path: .morph/refs/heads/main
    - kind: file_not_exists
      path: should_not_be_here
    - kind: file_eq
      path: out.txt
      content: "expected"
    - kind: dir_not_empty
      path: .morph/objects
```

Variables captured with `capture` or `capture_first_line` can be interpolated in later steps as `${var_name}` in morph args, write_file content, or compute_hash json.

**Critical:** The `init: false` field must be explicitly set for tests that should NOT auto-init (like `status_outside_repo`). By default, every spec auto-runs `morph init`.

### 2. Gherkin E2E Scenarios — `morph-e2e/features/`

Create the following new feature files. These test realistic multi-step user workflows through the real CLI binary.

#### `branch_and_checkout.feature`

```gherkin
Feature: Branch and checkout workflow

  Scenario: Create branch, commit on it, switch back to main
    Given a morph repo
    And a file "main_file.txt" with content "on main"
    When I run "morph add main_file.txt"
    And the last command succeeded
    # ... commit on main, create branch, switch to it, commit, switch back
    # Assert main_file.txt is present and branch-only file is absent

  Scenario: Checkout restores working tree from commit
    Given a morph repo
    # ... create file, add, commit on main
    # ... create branch, switch to it, create different file, add, commit
    # ... switch back to main
    # Assert the branch-only file is gone (tree restore)
```

#### `merge_workflow.feature`

```gherkin
Feature: Merge with behavioral dominance

  Scenario: Successful merge when metrics dominate both parents
    Given a morph repo
    # ... commit on main with metrics {acc: 0.9}
    # ... create feature branch, commit with metrics {acc: 0.85}
    # ... merge with metrics {acc: 0.92} → success, log shows merge

  Scenario: Merge rejected when metrics regress
    Given a morph repo
    # ... same setup
    # ... merge with metrics {acc: 0.87} → exit 1, stderr mentions rejected
```

#### `rollup_workflow.feature`

```gherkin
Feature: Rollup squashes commits

  Scenario: Rollup three commits into one
    Given a morph repo
    # ... make 3 commits, capture base hash
    # ... rollup base..HEAD, verify log shows squashed commit
```

**Step definitions you may need to add to `morph-e2e/tests/cucumber.rs`:**

Only add new step definitions if the existing ones are insufficient. The existing cucumber.rs already has:
- `given a morph repo`
- `given a file "<path>" with content "<content>"`
- `given the identity pipeline and a minimal eval suite exist`
- `when I run "<command>"` (runs any morph command with placeholder substitution)
- `when I capture the last output as "<name>"`
- `when I run commit with message "<msg>" using captured pipeline and eval suite`
- `when I run record-session with prompt "<p>" and response "<r>"`
- `when the last command succeeded` / `then the last command succeeded`
- `then stdout contains "<text>"`
- `then the path "<path>" exists as a directory`
- `then the path "<path>" is present` / `does not exist`
- `then the file "<path>" has content "<content>"`

You will likely need these new step definitions:
- `then the last command failed` — assert last exit code != 0
- `then stderr contains "<text>"` — check last_stderr
- `then the file "<path>" does not exist` — check file absence (different from path; specifically a file)

**Do NOT add step definitions you don't use.** Keep cucumber.rs minimal.

### 3. Unit Tests — `morph-core/src/`

Add or extend `#[cfg(test)] mod tests` in these files as needed:

#### `commit.rs` — add tests for:
- `checkout_tree` with a nonexistent branch (should return `MorphError::NotFound`)
- `checkout_tree` in detached HEAD mode (pass a 64-char hex hash)
- `log_from` on an empty repo (should return empty vec)
- `log_from` with an invalid ref (should error)
- `create_tree_commit` with empty index (should still succeed with empty tree)
- `rollup` where base == tip (edge case)

#### `metrics.rs` — add tests for:
- `check_dominance` when merged has extra metrics not in parent (should pass—superset dominates)
- `check_dominance` when merged is missing metrics that parent has (should fail)
- `check_dominance_with_suite` when a metric in the suite is missing from both merged and parent
- `aggregate` with a single-element slice for each method (mean, min, p95, lower_ci_bound)

### 4. Bug Fixes

As you write and run tests, you may discover bugs. Fix them. Common areas of risk:

- **`checkout_tree`**: Does it correctly error when the branch doesn't exist? Does it handle the case where HEAD is already detached?
- **`branch` CLI command**: Does `morph branch` (list) crash on a fresh repo with no commits?
- **`merge` CLI command**: The CLI requires `--pipeline` and `--eval-suite` as required args. But the core `create_merge_commit_full` supports auto-computing the union suite when `eval_suite_hash` is None. Consider whether the CLI should make `--eval-suite` optional and document the behavior.
- **`rollup` CLI command**: Does it update HEAD correctly? Does it handle the case where base_ref doesn't exist?
- **`upgrade`**: Does it correctly handle running upgrade when already at latest version?

When you find a bug, fix the implementation AND test the fix. Do not leave known-broken tests.

---

## Execution Sequence

Follow this exact order:

### Step 1: Read and understand

Read the following files before writing any code:
- `docs/TESTING.md`
- All files in `morph-cli/tests/specs/` (understand the YAML format)
- `morph-cli/build.rs` (understand how YAML specs become Rust tests)
- All files in `morph-e2e/features/` (understand existing Gherkin scenarios)
- `morph-e2e/tests/cucumber.rs` (understand existing step definitions)
- `morph-core/src/commit.rs` (the merge, checkout, rollup, branch logic)
- `morph-core/src/metrics.rs` (dominance, thresholds, union suites)
- `morph-cli/src/main.rs` (how CLI commands call into morph-core)

### Step 2: Write YAML spec tests

Create each `.yaml` file under `morph-cli/tests/specs/`. Run `cargo test -p morph-cli` after each file to verify tests compile and discover any failures.

### Step 3: Fix bugs found by specs

If any spec test fails because the CLI behavior is wrong, fix the CLI or core code. Re-run until all spec tests pass.

### Step 4: Write Gherkin E2E scenarios

Create `.feature` files under `morph-e2e/features/`. Add step definitions to `morph-e2e/tests/cucumber.rs` only if needed. Run `cargo test -p morph-e2e --test cucumber`.

### Step 5: Fix bugs found by E2E

Same as step 3 but for E2E failures.

### Step 6: Write unit tests

Add unit tests to `morph-core/src/commit.rs` and `morph-core/src/metrics.rs`. Run `cargo test -p morph-core`.

### Step 7: Fix bugs found by unit tests

Same pattern.

### Step 8: Full test suite

Run `cargo test --workspace` and ensure everything passes. Report the results.

---

## Test Commands

```bash
# Unit tests only (morph-core)
cargo test -p morph-core

# CLI integration tests (YAML specs)
cargo test -p morph-cli

# E2E tests (Gherkin/Cucumber)
cargo test -p morph-e2e --test cucumber

# Everything
cargo test --workspace
```

---

## Acceptance Criteria

All of the following must be true when you are done:

1. **New YAML spec files exist** for `branch`, `checkout`, `merge`, `rollup`, `upgrade`, and `errors` under `morph-cli/tests/specs/`.
2. **New Gherkin feature files exist** for branch/checkout workflow, merge workflow, and rollup workflow under `morph-e2e/features/`.
3. **New unit tests exist** in `morph-core/src/commit.rs` and `morph-core/src/metrics.rs` covering the cases listed above.
4. **All tests pass.** `cargo test --workspace` reports 0 failures.
5. **Tests use the real CLI binary**, not mocks. YAML specs run `morph` via `assert_cmd`. Gherkin scenarios run `morph` via `Command::cargo_bin("morph")`.
6. **Both happy paths and failure paths are covered.** At minimum: branch-not-found, checkout-nonexistent, merge-rejected, rollup-bad-ref, status-outside-repo.
7. **Any bugs discovered are fixed.** If a test reveals broken behavior, fix the implementation and keep the test.
8. **A final report is included** with the exact `cargo test` commands run and their pass/fail counts.

---

## Anti-Patterns to Avoid

- Do NOT create tests that only test in-memory behavior. All CLI tests must run the real `morph` binary.
- Do NOT add `#[ignore]` to tests. Every test must run and pass.
- Do NOT modify existing passing tests unless you are fixing a bug they failed to catch.
- Do NOT add test helpers or utilities beyond what already exists in the codebase (`setup_repo()`, `make_store()`, `store_blob()`).
- Do NOT create mock implementations of `Store`. Use `FsStore` via `tempfile::tempdir()`.
- Do NOT add new dependencies. The existing `assert_cmd`, `predicates`, `tempfile`, `cucumber`, and `tokio` are sufficient.
- Do NOT write tests with hard-coded hashes. Hashes are content-addressed and will change if serialization changes. Always capture hashes dynamically.

---

## Final Report Template

When done, report:

```
## Test Results

### morph-core (unit tests)
- Command: cargo test -p morph-core
- Result: X passed, Y failed

### morph-cli (YAML spec integration tests)
- Command: cargo test -p morph-cli
- Result: X passed, Y failed

### morph-e2e (Gherkin E2E tests)
- Command: cargo test -p morph-e2e --test cucumber
- Result: X passed, Y failed

### Full workspace
- Command: cargo test --workspace
- Result: X passed, Y failed

### New files created
- morph-cli/tests/specs/branch.yaml
- morph-cli/tests/specs/checkout.yaml
- morph-cli/tests/specs/merge.yaml
- morph-cli/tests/specs/rollup.yaml
- morph-cli/tests/specs/upgrade.yaml
- morph-cli/tests/specs/errors.yaml
- morph-e2e/features/branch_and_checkout.feature
- morph-e2e/features/merge_workflow.feature
- morph-e2e/features/rollup_workflow.feature

### Bugs found and fixed
- [description of each bug and the fix]

### Code changes
- [list of implementation files modified, with a one-line summary of each change]
```
