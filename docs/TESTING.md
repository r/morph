# Testing and coverage

## Current setup

- **morph-core**: Unit tests in `hash`, `store`, `repo`, `working`, `commit`, `metrics`, `annotate`, `identity`, `record`. Run with `cargo test -p morph-core`.
- **morph-cli**: Integration tests in `morph-cli/tests/integration.rs` (init, status, add, prompt, program, commit, log, run record, eval record, annotate). Run with `cargo test -p morph-cli`.
- **morph-mcp**: No tests yet.

## Running tests

```bash
cargo test                    # all workspace tests
cargo test -p morph-core      # core library only
cargo test -p morph-cli       # CLI integration tests
```

## Measuring coverage

1. Install [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) (requires `llvm-tools` component):

   ```bash
   rustup component add llvm-tools-preview
   cargo install cargo-llvm-cov
   ```

2. Run coverage for the library (excludes binaries that have no tests):

   ```bash
   cargo llvm-cov -p morph-core --html
   ```

   Open `target/llvm-cov/html/index.html` in a browser.

3. Include integration tests (they exercise the CLI, which pulls in morph-core):

   ```bash
   cargo llvm-cov -p morph-cli --html
   ```

4. Workspace-wide report (core + CLI tests):

   ```bash
   cargo llvm-cov --html
   ```

## How to improve coverage

1. **Run coverage and open the HTML report**  
   Focus on files with low line coverage and add tests for untested branches (error paths, edge cases).

2. **morph-core**
   - **store**: Add tests for `list()` by object type, `get()` with missing hash (NotFound), `ref_read` with symbolic ref if any path uses it.
   - **hash**: Test `Hash::from_hex` with invalid input (wrong length, non-hex).
   - **metrics**: Test `aggregate` with empty slice (error), unknown aggregation method; `aggregate_suite` and `check_thresholds` missing-metric path.
   - **record**: Test `record_eval_metrics` with invalid JSON / missing `metrics` key; `record_run` with trace hash mismatch.
   - **commit**: Test `resolve_head` (detached HEAD, symbolic ref), `rollup`, `log_from` edge cases.
   - **working**: Remaining branches in `find_repo`, `status`, `add_paths` (e.g. non-UTF-8, permission errors if desired).

3. **morph-cli**
   - Add integration tests for: `branch`, `checkout`, `morph run record` / `eval record` error cases (invalid JSON, missing files).
   - Consider testing stderr for failing commands.

4. **morph-mcp**
   - Add integration tests that invoke the MCP server (e.g. via `mcp_test` or a small harness) for the main tools (init, stage, commit, etc.).

5. **CI**
   - Add a CI job that runs `cargo test` and optionally `cargo llvm-cov --lcov -o lcov.info` and fails if coverage drops below a threshold (e.g. with `cargo-llvm-cov`’s `--fail-under-lines`).
