# Testing

## Test inventory

| Crate | Tests | Location |
|-------|-------|----------|
| **morph-core** | 86 unit tests across 12 modules | `#[cfg(test)]` blocks in each source file |
| **morph-cli** | 13 integration tests | `morph-cli/tests/integration.rs` |
| **morph-mcp** | None yet | -- |
| **morph-serve** | None yet | -- |

### morph-core unit test modules

`hash`, `store` (FsStore + GixStore), `repo`, `working`, `commit`, `metrics` (including direction-aware thresholds), `annotate`, `identity`, `record`, `index`, `tree`, `migrate`.

### morph-cli integration tests

`init`, `status`, `add`, `prompt create/materialize`, `program create/show`, `commit + log`, `run record + eval record`, `annotate + annotations`.

---

## Running tests

```bash
cargo test                    # all workspace tests (unit + integration)
cargo test -p morph-core      # core library only
cargo test -p morph-cli       # CLI integration tests only
cargo test --lib              # unit tests only (no integration)
```

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

Integration tests in `morph-cli/tests/integration.rs` exercise the CLI binary end-to-end using `assert_cmd` and `predicates`.

---

## Known gaps

- **morph-mcp**: No tests. An integration harness that speaks MCP over stdio would cover the primary write path.
- **morph-serve**: No tests. Could test API endpoints with axum's test utilities.
- **GixStore-specific paths**: `status()` and `record_session()` are now backend-aware (use `store.hash_object()`), but explicit GixStore integration tests would catch backend-specific regressions.
- **proptest**: In dev-dependencies but not yet used. Good candidate for property-based tests on hash determinism and serialization round-trips.
- **Error paths**: Many functions have untested error branches (malformed JSON, permission errors, missing refs).
- **CLI gaps**: No tests yet for `branch`, `checkout`, `merge`, `rollup`, `upgrade`, or error cases.
- **Direction-aware dominance**: `check_dominance()` currently assumes all metrics are "maximize". When a suite is available, dominance should respect per-metric direction. Tests exist for direction-aware `check_thresholds()`.
