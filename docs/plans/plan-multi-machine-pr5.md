# PR 5 — SSH transport for `Store`

Per-PR working spec. PR 1 landed `objmerge`, PR 2 `pipemerge`, PR 3 `treemerge` + repo_version 0.5, PR 4 the user-facing merge orchestration (`start_merge` / `continue_merge` / `abort_merge` / `resolve_node`, `morph pull --merge`, `morph status` integration, upgrade 0.4 → 0.5).

PR 4 still ties multi-machine workflows to a shared filesystem: `RemoteSpec.path` is a local path and `open_remote_store` only handles `<dir>` and `file://`. PR 5 lifts that constraint by speaking the `Store` trait over SSH.

The model is the one Git uses: a hidden `morph remote-helper` subcommand on the remote side reads JSON-RPC requests from stdin and writes responses to stdout; the local side spawns it via `ssh user@host morph remote-helper /path/to/repo` and routes every `Store` call through that pipe. After this PR, `morph push` / `fetch` / `pull` / the new `morph sync` work cross-machine with no extra daemon, no extra port, no extra auth — the user already proved who they are by getting a shell.

This is the first PR where a user without a shared filesystem can collaborate. It is also the first PR that introduces a process-spawning code path on the receiver side, so we are deliberate about what it can and cannot do.

## Scope

### In

- **Store trait refactor**: replace direct `refs_dir()` filesystem walks with a transport-agnostic `Store::list_branches() -> Vec<(String, Hash)>` (and a more general `Store::list_refs(prefix: &str) -> Vec<(String, Hash)>` that `list_branches` delegates to). Update `fetch_remote` in `sync.rs` to use the new method instead of `read_dir(remote_store.refs_dir().join("heads"))`. Provide a default impl for in-process stores that walks the existing refs dir; `SshStore` overrides it with a single RPC.
- **Remote URL parsing in `open_remote_store`**: extend the existing `&str` path argument to accept three forms — `<local-dir>` (existing), `file://<abs-path>` (existing), and `ssh://user@host[:port]/path` plus the SCP-like `user@host:path` (new). Detection by prefix; error early with a clear message on unknown schemes.
- **Hidden CLI subcommand `morph remote-helper <repo-path>`**: not in `--help`, marked `hide = true` on the `clap` derive. Reads newline-delimited JSON-RPC v2 requests from stdin, writes responses to stdout, logs to stderr. Runs `require_store_version` before serving any request — old repos refuse to serve until the user runs `morph upgrade` on them.
- **JSON-RPC protocol** (one method per `Store` trait method, plus a `handshake` method that reports `morph_version`, `repo_version`, and feature flags so future PRs can negotiate). Documented inline at the top of `morph-core/src/transport/ssh.rs`. Methods: `handshake`, `put`, `get`, `has`, `list`, `ref_read`, `ref_write`, `ref_read_raw`, `ref_write_raw`, `ref_delete`, `list_refs`, `list_branches`, `hash_object`. Server-side `put` re-validates the supplied hash matches the canonical hash of the object (defense-in-depth — a malicious client cannot smuggle in mislabeled objects).
- **`SshStore` in `morph-core/src/transport/ssh.rs`** implementing `Store`. Holds a child process handle (`std::process::Child` with piped stdin/stdout) and serialises calls. Each `Store` method is a one-shot RPC. Errors map: protocol/transport errors → `MorphError::Io` with context; server-side `MorphError` payloads round-trip back to the client unchanged (so `Diverged` in particular survives transport).
- **Spawning abstraction** so tests don't need a real SSH server. Define a small `RemoteSpawn` trait with a `spawn(&self) -> io::Result<Child>` method; provide two impls — `LocalSpawn` (spawns `morph remote-helper <path>` directly, used in tests) and `SshSpawn` (spawns `ssh user@host morph remote-helper <path>`). `SshStore::open(spec)` picks one based on the URL.
- **Tests** against `LocalSpawn` only: a fresh `morph remote-helper` child per test, run the full `Store` trait surface (put, get, has, list, list_branches, ref_*) plus an end-to-end `push_branch` / `fetch_remote` / `pull_branch` cycle. Cross-process test that the typed `MorphError::Diverged` survives the round trip.
- **`morph sync` command**: convenience over `fetch` + `pull --merge` for the user's *current* branch and a configured remote. Default remote comes from `branch.<name>.remote` if set, else `origin` if it exists, else error. Prints what it did at each step.
- **Branch-default config**: extend `read_remotes` / `write_remotes` (or a sibling `branch_config.rs`) to support `branch.<name>.remote` keys in `.morph/config.json`. New CLI: `morph branch set-upstream <branch> <remote>`. Hidden init-time default: `morph remote add origin ...` also sets `branch.main.remote = origin` if `main` exists.

### Out (deferred)

- Authentication / authorization beyond what SSH already provides. The remote-helper trusts whatever shell ran it; any further policy lives in PR 6.
- A long-running daemon mode (one-spawn-per-RPC is fine for v0).
- Push-time gate / closure-validation hooks on the server side — PR 6.
- Schema-version negotiation beyond `handshake`'s `repo_version` echo — PR 6 evolves it.
- HTTP transport, smart proxies, push notifications. Not in any near-term PR.
- Retiring `refs_dir()` from the `Store` trait — too much existing code uses it directly. PR 5 just adds `list_branches` / `list_refs`. A future cleanup PR can deprecate `refs_dir`.

## What it looks like to a user

```
$ morph remote add laptop ssh://raffi@laptop.local/Users/raffi/proj
$ morph fetch laptop
fetched laptop/main -> a3b2c1d
fetched laptop/feature/xyz -> e9f8a7b
$ morph pull laptop main --merge
fast-forward not possible (local 1234abc vs remote a3b2c1d); starting merge
Merging suite contracts ... ok
Merging tree ... clean (0 conflicts)
Merged main -> e1d2c3b (laptop)
```

```
$ morph branch set-upstream main laptop
$ morph sync
Fetching from laptop ... 2 refs updated
Pulling main ... fast-forwarded to a3b2c1d
```

If the remote `morph` is too old:

```
$ morph fetch oldserver
error: remote-helper handshake reports repo_version 0.4; this client requires 0.5+. Run `morph upgrade` on the remote.
```

## Why this PR is shaped this way

1. **Trait refactor before transport.** Today `fetch_remote` does `remote_store.refs_dir().join("heads"); read_dir(...)`. That cannot work over SSH. The transport-agnostic `list_branches` is *correctness*, not just an SSH detail — it makes the trait actually transport-neutral, which is what it always claimed to be. Even pre-SSH, the FsStore impl gets clearer.
2. **No new daemon.** We deliberately reuse SSH instead of building our own auth/transport. Most users already have SSH set up to the boxes they care about. Same model as Git's `ssh://` urls.
3. **One spawn per session, not per RPC.** A push transfers many objects; spawning per object would be obviously bad. The session model is: open SSH once, do all your puts/gets/refs through that single child, close.
4. **Server re-validates hashes.** A malicious client otherwise could write `{"hash": "<x>", "object": <y>}` and pollute the receiver's store with mislabeled blobs. Re-hashing on the server is cheap and closes that hole.
5. **Tests don't shell out to ssh.** A real SSH server is operationally awkward in CI and gives us nothing the local-spawn doesn't. Local-spawn tests cover the *protocol*; the SSH adapter is just a different `spawn` recipe and is exercised manually by the author + acceptance test outside CI.
6. **`morph sync` is a workflow shortcut, not a new transport.** It's pure CLI sugar over fetch + pull-merge that tracks the upstream. Useful enough to put in v0; not load-bearing.
7. **Branch upstream config** mirrors git's `branch.<name>.remote`. Already implied by remote-tracking refs (`refs/remotes/<remote>/<branch>`), the config just records the user's choice for `morph sync` defaults.

## TDD plan (35 cycles)

We extend the conventions from PRs 1–4: each cycle is RED → GREEN → REFACTOR; tests live in the same crate as the code they exercise; CLI behavior covered by YAML specs unless the test needs to set up state via core APIs (then a Rust integration test under `morph-cli/tests/`).

### Stage A — Store trait: `list_branches` / `list_refs` (cycles 1–3)

1. *RED*: `list_refs_returns_all_heads_under_prefix` — `Store::list_refs("heads")` returns `(name, hash)` pairs for every file under `.morph/refs/heads/`. Test against `FsStore`.
2. *GREEN*: implement `list_refs(prefix)` on `FsStore` walking `refs/<prefix>/` recursively. Default trait impl just calls `refs_dir()` (so `Box<dyn Store + '_>` keeps working).
3. *RED → GREEN → REFACTOR*: `fetch_remote_uses_list_branches_not_read_dir` — bind a fake `Store` whose `refs_dir()` panics but `list_branches()` returns a hand-coded list, and confirm `fetch_remote` walks it via `list_branches()`. Update `fetch_remote` accordingly.

### Stage B — Hidden `morph remote-helper` subcommand (cycles 4–9)

4. *RED*: `remote_helper_handshake_returns_versions` — spawn `morph remote-helper <repo>` as a child, send `{"jsonrpc":"2.0","method":"handshake","id":1}`, expect `{"morph_version":"0.10.x","repo_version":"0.5","features":[...]}`.
5. *GREEN*: minimal subcommand: parses one line, dispatches `handshake`, prints reply, exits.
6. *RED*: `remote_helper_serves_put_then_get_round_trip`. Pipe two requests; verify the get's payload equals the put's payload.
7. *GREEN*: dispatch `put` / `get` over the same long-running stdin loop; reuse `morph_core::open_store`.
8. *RED*: `remote_helper_refuses_to_serve_old_repo` — set `repo_version = "0.3"`, expect handshake to error with `RepoTooOld`.
9. *GREEN*: call `require_store_version` early in the helper; bubble the typed error back as JSON-RPC error response.

### Stage C — Wire format + error round-tripping (cycles 10–14)

10. *RED*: `protocol_serializes_typed_diverged_error` — round-trip a `MorphError::Diverged { ... }` through the request/response pair (no transport yet, just (de)serialization helpers).
11. *GREEN*: define `RpcRequest` / `RpcResponse` enums + a `MorphError` <-> JSON-RPC error code mapping.
12. *RED*: `protocol_rejects_put_when_hash_does_not_match_object` — server-side validation rejects `put { hash: "abcdef…", object: <y> }` if `content_hash(y) != hash`.
13. *GREEN*: re-hash on server-side `put`; return typed error if mismatch.
14. *RED*: `protocol_handles_concurrent_requests_in_single_session` — even though calls are sequential, ensure pipelined writes/reads don't corrupt each other (test sends multiple requests before reading any response, then drains).

### Stage D — `SshStore` against `LocalSpawn` (cycles 15–22)

15. *RED*: `ssh_store_implements_put_get` against `LocalSpawn { binary: env!("CARGO_BIN_EXE_morph"), repo: <tempdir> }`.
16. *GREEN*: skeleton `SshStore { child: Mutex<Child> }` implementing `put` / `get` only.
17. *RED → GREEN*: `ssh_store_implements_has_list_hash_object`.
18. *RED → GREEN*: `ssh_store_implements_ref_read_write_delete_raw_round_trips`.
19. *RED → GREEN*: `ssh_store_implements_list_branches`.
20. *RED*: `ssh_store_drops_close_child_cleanly` — drop the store, expect the helper child to exit with code 0 (stdin EOF).
21. *GREEN*: implement `Drop for SshStore` closing stdin.
22. *RED → GREEN → REFACTOR*: `push_branch_works_through_ssh_store_to_local_spawn` — full end-to-end push via the JSON-RPC pipe, then verify the receiver's tree on disk.

### Stage E — `open_remote_store` URL parsing + `RemoteSpawn` (cycles 23–26)

23. *RED*: `open_remote_store_handles_local_path_as_before` (regression).
24. *RED*: `open_remote_store_routes_ssh_url_to_ssh_spawn` — expects an `SshStore` whose internal `RemoteSpawn` is `SshSpawn { user, host, port, path }` parsed from `ssh://user@host:22/path`.
25. *RED*: `open_remote_store_handles_scp_like_user_at_host_path` — `raffi@host:/path` parses as ssh.
26. *GREEN*: implement URL parsing; reject unknown schemes with a clear `MorphError::Serialization` ("unsupported remote URL: ...").

### Stage F — `morph fetch` / `pull` / `pull --merge` over SSH (cycles 27–29)

27. *RED → GREEN*: end-to-end `fetch_remote_via_ssh_store_round_trip` (rust integration test in `morph-cli/tests/`, uses `LocalSpawn` URL form `local-helper:///<path>` reserved for tests).
28. *RED → GREEN*: end-to-end `pull_branch_via_ssh_store_diverged_returns_typed_error`.
29. *RED → GREEN*: end-to-end `pull_merge_via_ssh_store_finalizes_clean_three_way` — proves the typed `Diverged` survives transport into PR 4's merge flow.

### Stage G — `morph sync` + `branch set-upstream` (cycles 30–33)

30. *RED*: `branch_config_round_trips_remote_key` — write `branch.main.remote = origin` and read it back.
31. *GREEN*: `read_branch_config` / `write_branch_config` on `.morph/config.json`.
32. *RED → GREEN*: YAML spec `morph branch set-upstream main origin` writes the config; subsequent `morph sync` reads it and runs fetch + pull-merge.
33. *RED → GREEN*: `morph sync` errors with a clear message when no upstream is configured AND no `origin` exists.

### Stage H — Version bump, docs, wrap (cycles 34–35)

34. *RED → GREEN*: workspace `cargo test --workspace` clean; spec `morph --version` updated to new minor.
35. *RED → GREEN*: bump workspace version 0.9.0 → 0.10.0; record eval metrics; commit with metrics.

## Risks and what we'll do about them

- **Spawning `ssh` is platform-flaky.** Mitigation: tests use `LocalSpawn`. The `SshSpawn` impl is a thin shim with one acceptance test the author runs by hand. We document the manual recipe in `docs/MULTI-MACHINE.md` (PR 7).
- **Argument injection on the remote.** The repo path is passed as one `argv` element; we never assemble a shell command from user input. Mitigation: assert in tests that paths with spaces and quotes round-trip safely; reject paths containing newlines.
- **Stdin/stdout buffering deadlocks.** Our wire is line-delimited JSON; we always write a full request line before reading any response. Mitigation: explicit flush after every write; explicit reader-thread for stderr so the child never blocks on a full stderr buffer.
- **Server-side resource leaks.** A crashed client leaves the child running — until it tries the next read on stdin and EOFs. Mitigation: client closes stdin in `Drop`; server's main loop exits on EOF. Confirmed by cycle 20.
- **Non-ASCII filenames in refs.** `list_branches` returns `String`; refs already round-trip through `from_utf8_lossy` today. Mitigation: explicit test with a UTF-8 ref name.
- **Repo-too-old / repo-too-new on the remote.** Already handled by `require_store_version` returning typed errors; we let the JSON-RPC layer carry them through unchanged.
- **PR 6 dependency creep.** Resist the urge to add closure validation, push-time gates, identity, or schema negotiation in PR 5. Those land in PR 6 and need their own tests. PR 5's only job is *the wire works*.

## Acceptance checklist

- [ ] `cargo test --workspace` passes (target ~700 tests after PR 5 additions).
- [ ] `morph remote-helper --help` does NOT list it (`hide = true` works).
- [ ] `morph remote add laptop ssh://...` + `morph fetch laptop` works against a real second machine (manual smoke test).
- [ ] `morph push origin main` / `morph pull origin main --merge` work over SSH transport.
- [ ] `morph sync` runs fetch + pull-merge for the configured upstream.
- [ ] `MorphError::Diverged` survives SSH round-trip (asserted by cycle 28).
- [ ] Hash-mismatch attack on `put` rejected by the helper (asserted by cycle 12).
- [ ] Workspace version bumped to 0.10.0; eval metrics recorded; commit carries metrics.
