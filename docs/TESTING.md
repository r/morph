# Testing

## Test inventory

| Crate | Tests | Location |
|-------|-------|----------|
| **morph-core** | 539 unit tests across the lib's modules | `#[cfg(test)]` blocks in each source file |
| **morph-cli** | 182 YAML-driven integration tests + 21 unit tests + 20 dedicated integration tests (`remote_helper_integration`, `ssh_fetch_integration`, `status_merge_integration`) | YAML specs in `morph-cli/tests/specs/*.yaml`, compiled by `build.rs`; unit tests in `setup.rs`; dedicated integration files under `morph-cli/tests/`. |
| **morph-e2e** | Cucumber scenarios | `morph-e2e/features/*.feature`, step defs in `morph-e2e/tests/cucumber.rs` |
| **morph-mcp** | 20 integration tests | `#[cfg(test)]` in `morph-mcp/src/main.rs` |
| **morph-serve** | 37 unit/API tests (views, service, handlers, org policy, multi-repo) | `morph-serve/src/tests.rs` + `org_policy::tests` |

Totals: **819 Rust tests** (539 + 21 + 182 + 20 + 2 + 8 + 10 + 20 + 37) plus the Cucumber suite, all green.

### morph-core unit test highlights

The lib's 539 unit tests cover the core object/storage/merge layers. Notable areas (non-exhaustive):

- **Object model**: hash determinism, paper-aligned commit fields (review nodes, per-node `env`, set-valued attribution, `morph_instance`, `morph_version`), legacy compatibility (`from-run` provenance, `pipeline`/`program` aliases).
- **Storage**: `FsStore` in legacy, Git-format flat, and Git-format fan-out modes; ref read/write/delete; type-index directories; collision detection.
- **Migration**: `0.0 → 0.2` hash rewriting, `0.2 → 0.3` version bump, `0.3 → 0.4` fan-out move, `0.4 → 0.5` config-only bump; idempotency, empty/missing objects dir.
- **Working tree**: `working_tree_clean`, `checkout_tree`, `restore_tree`, `apply_workdir_ops`.
- **Index**: staging entries, `unmerged_entries` for merge in progress.
- **Merge**: LCA, `prepare_merge`, `execute_merge`, dominance with direction and retirement, evidence union, `merge_policy: "none"` opt-out, `start_merge`/`continue_merge`/`abort_merge`/`resolve_node`, structural conflicts on tree/pipeline/eval suite, textual fallback via `git merge-file`.
- **Metrics**: aggregation (`mean`, `min`, `p95`, `lower_ci_bound`), direction-aware thresholds, dominance with metric retirement.
- **Sync**: remote config round-trip, reachable closure, ancestry checks, push/fetch/pull scenarios, evidence-backed sync, `verify_closure`, schema handshake, branch upstreams, `clone_repo`.
- **SSH transport**: `SshUrl` parsing (URL + scp shorthand), `validate_hello`, error mapping, protocol-version mismatch.
- **Policy**: round-trip, certification pass/fail, gate pass/fail, `push_gated_branches` glob matching (`*` / `?` / literal), `enforce_push_gate`.
- **Tap & traces**: event grouping, task extraction, diagnostics, trace stats, eval export modes, kind normalization.
- **Misc**: `morphignore` matching, `diff` between commits, `tag` / `stash` / `revert` / `gc` lifecycles, pipeline extraction from runs.

### morph-cli integration tests

YAML specs in `morph-cli/tests/specs/` cover every user-facing CLI command. Categories: repository lifecycle (`init`, `status`, `add`), prompts/pipelines (`prompt create/materialize/show`, `pipeline create/show/extract`), commits (`commit`, `log`, `--from-run` provenance), evidence (`run record`, `run list`, `run show`, `trace show`, `tap`, `traces`), branching (`branch`, `checkout`, `tag`, `stash`, `revert`, `diff`, `rollup`), merging (`merge_plan`, `merge` single-shot, `merge --continue`, `merge --abort`, `merge resolve-node`, textual conflict drop-into-continue flow), remotes (`remote`, `push_pull`, `clone`, `sync`, `branch --set-upstream`), policy (`policy`, `certify_gate`, push-gated branches), and misc (`upgrade`, `morphignore`, error paths). Three dedicated Rust integration files exercise the SSH server (`remote_helper_integration`, `ssh_fetch_integration`) and the merge state machine surfaced in `status` (`status_merge_integration`).

### morph-mcp integration tests

All 15 MCP tools tested (17 test functions): **init** (success + already-initialized error), **record_session** (hash return), **record_run**, **record_eval** (file-based metrics), **stage** (explicit paths + default `.`), **commit** (basic, with metrics, with `--from-run` provenance), **branch** (success + no-commit error), **checkout** (branch switch), **annotate** (annotation creation), **status** (file listing), **log** (commit history), **show** (object JSON), **diff** (between commits), **merge** (behavioral dominance), **repo_store** (not-found error message, accepts upgraded store version 0.4).

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
