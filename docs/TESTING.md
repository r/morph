# Testing

## Test inventory

| Crate | Tests | Location |
|-------|-------|----------|
| **morph-core** | 108+ unit tests across 12 modules | `#[cfg(test)]` blocks in each source file |
| **morph-cli** | 18 integration tests | YAML specs in `morph-cli/tests/specs/*.yaml`, compiled by `build.rs` |
| **morph-e2e** | Cucumber e2e tests | `morph-e2e/features/*.feature`, step defs in `morph-e2e/tests/cucumber.rs` |
| **morph-mcp** | None yet | -- |
| **morph-serve** | None yet | -- |

### morph-core unit test modules

`hash` (including paper-aligned fields: review nodes, per-node env, set-valued attribution), `store` (FsStore + GixStore), `repo`, `working`, `commit` (including merge with union suite), `metrics` (direction-aware thresholds, metric retirement), `annotate`, `identity`, `record`, `index`, `tree`, `migrate`.

### morph-cli integration tests

`init`, `status`, `add`, `prompt create/materialize`, `pipeline create/show`, `commit + log`, `run record + eval record`, `annotate + annotations`.

---

## Running tests

```bash
cargo test                    # all workspace tests (unit + integration)
cargo test -p morph-core      # core library only
cargo test -p morph-cli       # CLI integration tests only
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

Each YAML spec supports: file/directory setup (`files`, `dirs`), sequenced CLI commands (`morph` steps), variable capture from stdout (`capture`, `capture_first_line`), variable interpolation (`${var}`), hash computation (`compute_hash`), dynamic file creation (`write_file`), and assertions on exit code, stdout/stderr content, and filesystem state. See any spec file for examples.

---

## Known gaps

- **morph-mcp**: No tests. An integration harness that speaks MCP over stdio would cover the primary write path.
- **morph-serve**: No tests. Could test API endpoints with axum's test utilities.
- **GixStore-specific paths**: `status()` and `record_session()` are now backend-aware (use `store.hash_object()`), but explicit GixStore integration tests would catch backend-specific regressions.
- **proptest**: In dev-dependencies but not yet used. Good candidate for property-based tests on hash determinism and serialization round-trips.
- **Error paths**: Many functions have untested error branches (malformed JSON, permission errors, missing refs).
- **CLI gaps**: No tests yet for `branch`, `checkout`, `merge`, `rollup`, `upgrade`, or error cases.
- **Direction-aware dominance**: `check_dominance()` currently assumes all metrics are "maximize". When a suite is available, dominance should respect per-metric direction. Tests exist for direction-aware `check_thresholds()`.
