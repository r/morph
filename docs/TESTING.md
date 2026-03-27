# Testing

## Test inventory

| Crate | Tests | Location |
|-------|-------|----------|
| **morph-core** | 182 unit tests across 16 modules | `#[cfg(test)]` blocks in each source file |
| **morph-cli** | 76 integration tests | YAML specs in `morph-cli/tests/specs/*.yaml`, compiled by `build.rs` |
| **morph-e2e** | 25 Cucumber e2e scenarios | `morph-e2e/features/*.feature`, step defs in `morph-e2e/tests/cucumber.rs` |
| **morph-mcp** | None yet | -- |
| **morph-serve** | 34+ unit/API tests (views, service, handlers, org policy, multi-repo) | `morph-serve/src/tests.rs` + `org_policy::tests` |

### morph-core unit test modules

`hash` (including paper-aligned fields: review nodes, per-node env, set-valued attribution), `store` (FsStore + GixStore), `repo`, `working`, `commit` (including merge with union suite, **provenance from run**, evidence_refs, env_constraints, contributors), `metrics` (direction-aware thresholds, metric retirement), `merge` (merge planning, dominance explanation, direction-aware reference bar, metric retirement in dominance checks, execute merge), `annotate`, `identity`, `record`, `index`, `tree`, `migrate`, `extract` (pipeline extraction from runs: graph shape, provenance, attribution, error paths), **`sync`** (remote config round-trip, reachable object collection, ancestry checks, push to empty remote, push non-fast-forward rejection, push idempotency, hash preservation across sync, fetch creates remote-tracking refs, fetch does not overwrite local branches, fetch transfers only missing objects, pull fast-forward, pull divergence rejection, evidence-backed commit sync, remote store open/validation, ref listing), **`policy`** (policy round-trip, config preservation, default policy, certification pass/fail with required metrics/thresholds/directions, gate pass/fail for certified/uncertified commits, gate failure reason identification, certification annotation recording).

### morph-cli integration tests

`init`, `status`, `add`, `prompt create/materialize`, `pipeline create/show`, `commit + log`, `run record + eval record`, `annotate + annotations`, `branch`, `checkout`, `merge` (including auto-union suite, explained metric failure, retirement), `merge_plan` (parent inspection, reference bar, retirement preview, incompatible suites), `rollup`, `upgrade`, `errors`, **`provenance`** (evidence-backed commits with `--from-run`, `morph show`), **`pipeline_extract`** (trace-backed pipeline extraction with `--from-run`, reviewer attribution, reuse in commits, error paths), **`remote`** (add/list remotes, push to empty remote, push idempotent, push to missing remote fails, refs listing), **`push_pull`** (fetch creates remote-tracking refs, pull fast-forwards, push non-fast-forward fails, evidence-backed commit closure transfer, refs show remote-tracking after fetch), **`policy`** (round-trip set/show, set-default-eval, default empty policy), **`certify_gate`** (certify with metrics file, certify specific commit, certify fail on missing metrics, certify fail below threshold, certify with runner, gate pass for certified commit, gate fail on missing metrics, gate fail below threshold, gate fail uncertified, JSON output for certify and gate).

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

Each YAML spec supports: file/directory setup (`files`, `dirs`), sequenced CLI commands (`morph` steps), variable capture from stdout (`capture`, `capture_first_line`), variable interpolation (`${var}`), hash computation (`compute_hash`), dynamic file creation (`write_file`), per-step working directory override (`cwd` for multi-repo tests), and assertions on exit code, stdout/stderr content, and filesystem state. See any spec file for examples.

---

## Known gaps

- **morph-mcp**: No tests. An integration harness that speaks MCP over stdio would cover the primary write path.
- **morph-serve**: 34+ tests covering repo list/summary, branch listing, commit list/detail, run/trace/pipeline detail, annotations, policy, gate status, behavioral status (certification/merge), org policy CRUD, multi-repo, backward-compatible endpoints, and error paths.
- **GixStore-specific paths**: `status()` and `record_session()` are now backend-aware (use `store.hash_object()`), but explicit GixStore integration tests would catch backend-specific regressions.
- **proptest**: In dev-dependencies but not yet used. Good candidate for property-based tests on hash determinism and serialization round-trips.
- **Error paths**: Many functions have untested error branches (malformed JSON, permission errors, missing refs).
- **CLI gaps**: `branch`, `checkout`, `merge`, `rollup`, `upgrade`, `errors`, `provenance`, `remote`, `push_pull`, `policy`, and `certify_gate` now have YAML specs. MCP integration tests are still missing.
- **Direction-aware dominance**: `check_dominance()` assumes all metrics are "maximize". The new `merge` module's `MergePlan::check_dominance()` and `check_dominance_with_suite()` respect per-metric direction. Tests cover both maximize and minimize directions during merge planning.
- **Network transport**: Phase 5 sync uses local filesystem paths only. Network transport (HTTP, SSH) tests will be needed when that transport is added.
- **MCP certification/gating**: The certification and gate flows are available via CLI only. MCP exposure would allow IDE-driven certification workflows.
