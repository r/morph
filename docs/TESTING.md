# Testing

## Test inventory

| Crate | Tests | Location |
|-------|-------|----------|
| **morph-core** | 298 unit tests across 24 modules (23 with test blocks) | `#[cfg(test)]` blocks in each source file |
| **morph-cli** | 140 integration tests + 18 unit tests | YAML specs in `morph-cli/tests/specs/*.yaml`, compiled by `build.rs`; unit tests in `setup.rs` |
| **morph-e2e** | 35 Cucumber e2e scenarios (3 skipped in CI) | `morph-e2e/features/*.feature`, step defs in `morph-e2e/tests/cucumber.rs` |
| **morph-mcp** | 17 integration tests | `#[cfg(test)]` in `morph-mcp/src/main.rs` |
| **morph-serve** | 36 unit/API tests (views, service, handlers, org policy, multi-repo) | `morph-serve/src/tests.rs` + `org_policy::tests` |

### morph-core unit test modules

`hash` (including paper-aligned fields: review nodes, per-node env, set-valued attribution), `store` (FsStore with legacy + Git hash modes, ref_delete), `repo`, `working`, `commit` (including merge with union suite, **provenance from run**, evidence_refs, env_constraints, contributors), `metrics` (direction-aware thresholds, metric retirement), `merge` (merge planning, dominance explanation, direction-aware reference bar, metric retirement in dominance checks, execute merge), `annotate`, `identity`, `record` (**record_run** with trace validation/mismatch/artifacts/error paths, **record_eval_metrics** with validation/error paths, record_session defaults, metadata merge into payload, per-message timestamps, backward compat without metadata, HEAD commit linking), `index`, `tree`, `migrate` (**0.0→0.2** hash rewriting, **0.2→0.3** version bump, idempotency, empty/missing objects dir), `extract` (pipeline extraction from runs: graph shape, provenance, attribution, error paths), **`tap`** (event grouping: simple/multi-step/aliases/tool calls/file events/empty traces/tool-result pairing, task extraction, diagnostics: shallow trace/empty response/prompt lengths/model+agent, trace stats: event details/structured detection, run filtering: by model/by min steps, repo summary, eval export: prompt-only/agentic multi-step/context with file content, kind normalization, placeholder response detection, token usage extraction from run parameters, error paths), **`sync`** (remote config round-trip, reachable object collection, ancestry checks, push/fetch/pull scenarios, evidence-backed sync), **`policy`** (policy round-trip, certification pass/fail, gate pass/fail, annotation recording), **`morphignore`** (load/match patterns, glob, directory, negation, nested paths, multiple patterns, paths outside repo), **`diff`** (empty trees, added/deleted/modified/mixed changes, nested trees, store-backed diff, None-to-tree and tree-to-None), **`tag`** (create/list/delete, duplicate tag error, nonexistent delete error, empty repo), **`stash`** (save/pop roundtrip, empty index error, empty pop error, LIFO ordering, list, no-message save), **`revert`** (parent tree restoration, root commit → empty tree, non-commit error, branch ref update), **`gc`** (garbage collection of unreferenced objects).

### morph-cli integration tests

`init`, `status`, `add`, `prompt create/materialize`, **`prompt show`** (latest, by run hash, no-runs error), `pipeline create/show`, `commit + log`, `run record + eval record`, **`run list`** (populated + empty), **`run show`** (summary, JSON, with-trace, invalid hash), **`trace show`** (event display), **`tap`** (summary empty/populated, inspect single/all, diagnose single/all/new-fields, export prompt-only/with-context/agentic/invalid-mode/to-file/filter-by-model/filter-by-agent, preview run, multi-step messages), `annotate + annotations`, `branch`, `checkout`, `merge`, `merge_plan`, `rollup`, `upgrade`, **`diff`** (added files, modified files, no changes, HEAD shorthand), **`tag`** (create/list, duplicate error, delete, delete nonexistent, list empty), **`stash`** (save/pop, list, empty save error, empty pop error), **`revert`** (undo commit, invalid hash error), **`error_paths`**, **`morphignore`**, `errors`, **`provenance`**, **`pipeline_extract`**, **`remote`**, **`push_pull`**, **`policy`**, **`certify_gate`**.

### morph-mcp integration tests

All 15 MCP tools tested (16 test functions): **init** (success + already-initialized error), **record_session** (hash return), **record_run**, **record_eval** (file-based metrics), **stage** (explicit paths + default `.`), **commit** (basic, with metrics, with `--from-run` provenance), **branch** (success + no-commit error), **checkout** (branch switch), **annotate** (annotation creation), **status** (file listing), **log** (commit history), **show** (object JSON), **diff** (between commits), **merge** (behavioral dominance), **repo_store** (not-found error message).

---

## Running tests

```bash
cargo test                    # all workspace tests (unit + integration)
cargo test -p morph-core      # core library only
cargo test -p morph-cli       # CLI integration tests only
cargo test -p morph-mcp       # MCP server tests
cargo test -p morph-e2e --test cucumber   # e2e (Cucumber; runs real morph CLI)
cargo test --lib              # unit tests only (no integration)
```

E2E tests require the `morph` binary; from the repo root the workspace builds it when you run `cargo test -p morph-e2e --test cucumber`.

---

## Coverage

Install [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov):

```bash
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov
```

Generate reports:

```bash
cargo llvm-cov -p morph-core --html    # core library
cargo llvm-cov --html                  # full workspace
```

Open `target/llvm-cov/html/index.html`.

---

## Test architecture notes

Each morph-core module owns its tests in a `#[cfg(test)] mod tests` block. Tests use `tempfile::tempdir()` for filesystem isolation. Common test patterns:

- **setup_repo()**: Creates a temp dir with `init_repo`, returns `(TempDir, FsStore)`.
- **make_store()**: Creates a bare store (no repo init) for lower-level store tests.
- **store_blob()**: Helper to create and store a blob, returning its hash.

CLI integration tests are defined as YAML specs in `morph-cli/tests/specs/*.yaml`. At build time, `morph-cli/build.rs` reads these specs and generates Rust test functions into `$OUT_DIR/spec_tests.rs`, which is `include!`'d from `morph-cli/tests/spec_tests.rs`. The generated code uses `assert_cmd` and `predicates` under the hood.

Each YAML spec supports: file/directory setup (`files`, `dirs`), sequenced CLI commands (`morph` steps), variable capture from stdout (`capture`, `capture_first_line`), variable interpolation (`${var}`), hash computation (`compute_hash`), dynamic file creation (`write_file`), per-step working directory override (`cwd` for multi-repo tests), and assertions on exit code, stdout/stderr content, and filesystem state. See any spec file for examples.

---

## Known gaps

- **Git-format hash paths**: `status()` and `record_session()` are backend-aware (use `store.hash_object()`), but explicit integration tests for Git-format hashing (via `FsStore::new_git()`) would catch hash-mode-specific regressions.
- **proptest**: In dev-dependencies but not yet used. Good candidate for property-based tests on hash determinism and serialization round-trips.
- **Network transport**: Phase 5 sync uses local filesystem paths only. Network transport (HTTP, SSH) tests will be needed when that transport is added.
- **MCP certification/gating**: The certification and gate flows are available via CLI only. MCP exposure would allow IDE-driven certification workflows.
- **`morph blame`**: Per-file/per-line attribution showing which commit/agent modified each part. Planned but not yet implemented.
- **E2E hosted service**: 3 Cucumber scenarios are skipped due to server binding constraints in CI.
