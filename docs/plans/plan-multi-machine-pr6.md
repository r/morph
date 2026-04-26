# PR 6 — Server-readiness

Per-PR working spec. PR 1 landed `objmerge`, PR 2 `pipemerge`, PR 3 `treemerge` + repo_version 0.5, PR 4 the user-facing merge flow + `morph pull --merge`, PR 5 the SSH transport (`morph remote-helper`, `SshStore`, `morph sync`).

After PR 5 a developer can already point `morph` at any host they can `ssh` into. PR 6 turns that ad-hoc capability into something that's actually safe to designate as "the server": durable identity on commits, an explicit bare layout for hosts that aren't anyone's working repo, and server-side validation that rejects malformed pushes before a ref ever moves.

This is the first PR whose value lands primarily on the *receiving* side. Most changes are in `morph-core` and `morph-cli` (the helper), not in the user's day-to-day flow.

## Scope

### In

- **Human identity (`user.name` / `user.email`)**:
  - New keys in `.morph/config.json`. CLI: `morph config user.name "Raffi"` / `morph config user.email "r@example.com"` (read & write). Env overrides: `MORPH_AUTHOR_NAME`, `MORPH_AUTHOR_EMAIL`.
  - Author resolution order: explicit `--author` on `commit` / `merge --continue` > env > config > legacy `"morph"` default.
  - Format on commit: `"Raffi <r@example.com>"` (matches Git). When only name is configured, omit the angle brackets.
  - Plumbed through `create_commit_full`, `create_merge_commit_full`, `execute_merge`, and `continue_merge`.
- **Instance identity (`agent.instance_id`)**:
  - New `instance_id` key under an `agent` block in `.morph/config.json`. Auto-generated on `init` if absent (`morph-<6-char-hex>`). User can override.
  - Surfaced on every new commit as a `morph_instance` field (next to `morph_version`). Optional / backward-compatible: old commits don't have it; new clients render it when present.
  - Useful for the multi-machine case: when two laptops both commit on `main` and one syncs the other, the merge commit's contributor list and the per-parent `morph_instance` make it obvious which agent did what.
- **Evidence union on merge**:
  - `execute_merge` currently sets `commit.evidence_refs = None`. PR 6 changes it to the union of `head_commit.evidence_refs` and `other_commit.evidence_refs` (deduped, deterministic order). Behaviour is opt-in via a new `MergePlan.evidence_refs: Option<Vec<String>>` populated by `prepare_merge` so the existing call sites keep compiling.
  - Runs and traces referenced by either parent are preserved on the merge commit; together with the union eval suite, this means "what did we run" is preserved across the merge boundary, not lost.
- **Bare repo layout (`morph init --bare`)**:
  - New `--bare` flag on `morph init`. Bare repos:
    - Live in a `<name>.morph/` directory (no enclosing project root, no `.morph/.gitignore`).
    - Have `bare = true` in `config.json`.
    - Have no `INDEX`, no `MERGE_*` files, no `HEAD` symref to a working branch (we still keep `refs/HEAD` pointing at `heads/main` as a default for first-push fast-forward, but treat it as a soft default).
  - `morph remote-helper` accepts both layouts: if `<repo-path>/.morph` exists, it's a working repo; if `<repo-path>` itself contains `objects/` and `refs/`, it's a bare repo. Helper rejects working-tree-touching ops (none today, but fenced for future).
  - `is_bare(morph_dir) -> bool` helper for any code that wants to refuse working-tree assumptions.
- **Schema handshake**:
  - The PR 5 `Hello` request already round-trips `morph_version`. PR 6 adds `repo_version` and a `protocol_version` integer (start at `1`).
  - Client refuses to talk to a server whose `repo_version` is older than its own (suggest `morph upgrade` on the remote) and refuses a server whose `protocol_version` is higher than what it understands (suggest upgrading the client). The repo_version-too-new path was already handled implicitly by `require_store_version`; PR 6 makes it surface as a typed `MorphError::IncompatibleRemote { kind, ours, theirs }`.
  - `OkResponse::hello` carries `repo_version` and `protocol_version`; `ErrorKind` gains an `incompatible_remote` variant for the typed round-trip.
- **Server-side closure validation on push (`ref_write`)**:
  - Today the helper accepts `Put { object }` and `RefWrite { name, hash }` independently. A buggy or malicious client could move a ref to a hash whose closure is incomplete on the server, silently corrupting the receiver.
  - PR 6 makes `RefWrite` (when the name starts with `heads/`) walk the reachable closure of the new tip on the server side and reject with `MorphError::Serialization("closure incomplete: missing <hash>")` if anything is missing. Uses the existing `collect_reachable_objects`.
  - This also means the server re-canonicalizes every put: `Put { object }` already verifies `content_hash(object) == claimed_hash` (PR 5 left a TODO, PR 6 enforces). This is the defense-in-depth we promised.
- **Server-side gate-on-push (optional)**:
  - New `push.gate_branches: Vec<String>` field in `RepoPolicy`. When the pushed ref name matches an entry, the helper runs `gate_check` on the new tip after closure validation, and rejects the `RefWrite` with the standard `GateResult` reasons if it fails.
  - Default is empty; existing repos see no behaviour change. Setting `push.gate_branches = ["main"]` on a server makes `main` un-pushable except for commits that already pass `gate_check`.
  - Surfaced as a typed error (`MorphError::Serialization`) carrying the gate reasons so the client can render them inline.
- **`morph init --bare` is documented and end-to-end tested**: spec test plus an SSH integration test that pushes into a bare server, fetches from it, and verifies a second client sees the closure.

### Out (deferred)

- **GPG / SSH signing of commits.** Identity is text-only for now. Cryptographic provenance is a separate PR.
- **Authn/authz beyond what the SSH shell already gives us.** Per-branch ACLs, push tokens, etc. are out.
- **A daemon/long-running server.** The helper is still spawn-per-connection.
- **Server-initiated push notifications / webhooks / sync events.** Out forever, probably.
- **Schema migration on the server.** Server still refuses to talk to old/new repos; it does *not* run `migrate_*` for the user.
- **`morph fsck` / repair.** Closure validation rejects incomplete pushes, but doesn't repair existing damage.

## What it looks like to a user

Setting up a server, one time:

```
$ ssh raffi@server.local 'mkdir -p ~/morph-repos && cd ~/morph-repos && morph init --bare project'
Initialized bare Morph repository at ~/morph-repos/project (repo_version 0.5)
$ ssh raffi@server.local 'cd ~/morph-repos/project && morph config policy.push.gate_branches main'
```

From any laptop:

```
$ morph remote add origin ssh://raffi@server.local/home/raffi/morph-repos/project
$ morph config user.name "Raffi"
$ morph config user.email "r@example.com"
$ morph branch --set-upstream origin/main
$ morph push origin main
Pushed main -> a3b2c1d (origin)  [validated 142 objects, gate: pass]
```

Trying to push a commit that doesn't dominate (gate fails):

```
$ morph push origin main
error: server rejected push: gate_check failed for a3b2c1d on origin/main
  - missing required metric: pass_rate
  - threshold violated: build_time_secs > 600
hint: certify the commit locally first (`morph certify`) and push again.
```

A second laptop joining a project:

```
$ morph clone ssh://raffi@server.local/home/raffi/morph-repos/project   # PR 7-ish or a thin wrapper here
$ morph sync
Synced main -> a3b2c1d (origin/main)
```

The merge commit a CI run produces, viewed in `morph show`:

```
$ morph show HEAD
commit e1d2c3b
author "Raffi <r@example.com>"
morph_version 0.10.0
morph_instance morph-7c4e91
parents 1234abc, a3b2c1d
contributors:
  - "Raffi <r@example.com>"  (instance: morph-7c4e91)
  - "Raffi <r@example.com>"  (instance: morph-2ad5e8)
evidence_refs:
  - <run-hash-from-laptop-1>
  - <run-hash-from-laptop-2>
```

## Out-of-scope hooks for future PRs

- **Mirror / replication**: `morph remote add upstream-mirror ...` and a `morph mirror push` that fanout-pushes after every local `push`. Composes trivially with PR 5 + PR 6.
- **Server-side hooks**: `pre-push`, `post-push` shell hooks under `.morph/hooks/`. Same pattern as Git.
- **Smart HTTP transport**.

## TDD plan: ~30 cycles, six stages

### Stage A — Identity (cycles 1-5)

Cycle 1 (RED→GREEN): `morph_core::identity::resolve_author(morph_dir, override_arg)` returns `"Name <email>"` from explicit > env > config. Pure function on a `BTreeMap<String,String>` config; tested in `morph-core/src/identity.rs`.

Cycle 2: `morph config user.name <value>` and `morph config user.email <value>` (and the `--get` form) round-trip through `.morph/config.json`. Spec test under `morph-cli/tests/specs/config.yaml`.

Cycle 3: First commit after `morph config user.name "Raffi"` carries `author == "Raffi"` (no email yet). Spec test asserts via `morph show HEAD --json | jq -r .author`.

Cycle 4: With both name and email set, author is `"Raffi <r@example.com>"`. Spec test.

Cycle 5: `MORPH_AUTHOR_NAME` / `MORPH_AUTHOR_EMAIL` envs override config but are themselves overridden by explicit `--author`. Spec test.

### Stage B — Instance ID (cycles 6-9)

Cycle 6: `morph init` writes a randomly generated `agent.instance_id` to `.morph/config.json`. Property: idempotent re-init is rejected (existing behaviour); fresh inits get unique IDs. Unit test in `morph-core/src/repo.rs`.

Cycle 7: `Commit { morph_instance: Option<String> }` field added with `#[serde(default, skip_serializing_if = "Option::is_none")]` for back-compat. Existing commits deserialize with `None`. Unit test in `morph-core/src/objects.rs`.

Cycle 8: `create_commit_full` populates `morph_instance` from the resolved config. Spec test: commit, then `morph show --json | jq .morph_instance` returns the configured value.

Cycle 9: Round-trip through fetch — push a commit, fetch it on a fresh client, verify the new client sees the original `morph_instance`. Integration test under `morph-cli/tests/ssh_fetch_integration.rs`.

### Stage C — Evidence union (cycles 10-13)

Cycle 10: `MergePlan.evidence_refs: Option<Vec<String>>` populated by `prepare_merge` as the deduped union of both parents. Unit test in `morph-core/src/merge.rs`.

Cycle 11: `execute_merge` writes those refs to the resulting `Commit.evidence_refs` (replacing `None`). Unit test asserting the merge commit carries the union.

Cycle 12: When neither parent has evidence, `evidence_refs` stays `None` (don't create empty arrays — they'd churn hashes for legacy paths). Unit test.

Cycle 13: End-to-end via `continue_merge`: divergent histories with evidence on both sides, finalize the merge, verify `morph show <merge> --json` lists both runs. Spec test in `morph-cli/tests/specs/merge.yaml` (or `push_pull.yaml`).

### Stage D — Bare repos (cycles 14-19)

Cycle 14: `morph_core::repo::init_bare(root)` creates `objects/`, `refs/heads/`, `prompts/`, `evals/`, `runs/`, `traces/`, `config.json` with `repo_version` *and* `bare = true`. No `.morph/.gitignore`, no `INDEX`. Unit test.

Cycle 15: `is_bare(morph_dir) -> bool` reads `config.json` and returns the flag. Default false. Unit test.

Cycle 16: `morph init --bare <path>` CLI surface. Spec test asserting the directory shape.

Cycle 17: `open_store(path)` accepts both shapes — `path/.morph/` for working repos, `path/` for bare repos. The helper resolves which one applies, and refuses to load any path that has neither. Unit + integration tests.

Cycle 18: `morph remote-helper` works with a bare repo (essentially: no special-casing required, but assert it via integration test against a bare server).

Cycle 19: `morph push` / `fetch` / `sync` against a bare SSH remote — full round trip. Integration test in `ssh_fetch_integration.rs`.

### Stage E — Handshake (cycles 20-23)

Cycle 20: `OkResponse::hello` carries `repo_version` and `protocol_version` (default `1`). Wire-format unit test in `ssh_proto.rs`.

Cycle 21: `SshStore::connect` reads them and stores them on the connection. New typed error `MorphError::IncompatibleRemote { kind: "repo_version" | "protocol_version", ours: String, theirs: String }`. Unit test forcing a synthetic helper that returns `repo_version: "0.4"` and asserting we get `IncompatibleRemote`.

Cycle 22: `MorphError::IncompatibleRemote` round-trips through `ssh_proto::ErrorKind::incompatible_remote` for symmetry with `Diverged`. Unit test.

Cycle 23: Helper version-gates: a 0.4 server refuses to serve a 0.5 client (existing `require_store_version`), and surfaces a `morph upgrade` hint. Spec / integration test against a synthetic-old fixture (manually-written `config.json` with `repo_version = "0.4"`, helper wired to refuse).

### Stage F — Server-side validation + gating (cycles 24-30)

Cycle 24: `Put { object }` on the helper recomputes the canonical hash and rejects if it doesn't match the path-derived hash. Unit + helper integration test.

Cycle 25: `RefWrite { name: "heads/...", hash }` walks `collect_reachable_objects(hash)` against the local store and returns `MorphError::Serialization("closure incomplete: missing <hash>")` if anything is absent. Helper integration test with a manually crafted partial push.

Cycle 26: That same closure check runs on the *server side* of `push_branch` even when the client is well-behaved — defense-in-depth. End-to-end test pushing into a bare repo and asserting all expected objects are present.

Cycle 27: `RepoPolicy::push.gate_branches: Vec<String>` field, defaults to empty. Unit test for serialization back-compat.

Cycle 28: When set, `RefWrite` runs `gate_check` and refuses with the failure reasons. Helper integration test that sets `push.gate_branches = ["main"]` and tries to push a non-dominating commit.

Cycle 29: When the gate passes, `RefWrite` succeeds normally. Integration test with a certified commit.

Cycle 30: End-to-end `morph push` over SSH against a bare server with `gate_branches = ["main"]`: certified commits go through, non-certified are rejected with the gate reasons surfaced in `stderr`. Integration test under `ssh_fetch_integration.rs`.

### Stage G — Wrap (cycles 31-32)

Cycle 31: Workspace test — every crate green. Update `docs/v0-spec.md` references that mention identity / bare layout / push-gate (light touch; deep docs land in PR 7).

Cycle 32: Workspace version bump to `0.11.0`, eval recorded, commit shaped like the previous PRs.

## File-level surface (what's likely to change)

- `morph-core/src/identity.rs` — **new**. `resolve_author`, env var reading.
- `morph-core/src/repo.rs` — `init_bare`, `is_bare`, `agent.instance_id` generation on `init_repo`.
- `morph-core/src/objects.rs` — `Commit { morph_instance: Option<String> }`.
- `morph-core/src/commit.rs` — author resolution wiring; `morph_instance` populated.
- `morph-core/src/merge.rs` — `MergePlan.evidence_refs`, `execute_merge` writes union, `prepare_merge` populates it.
- `morph-core/src/policy.rs` — `RepoPolicy::push.gate_branches`.
- `morph-core/src/store.rs` — `MorphError::IncompatibleRemote` variant.
- `morph-core/src/ssh_proto.rs` — extended `OkResponse::hello`, `ErrorKind::incompatible_remote`.
- `morph-core/src/ssh_store.rs` — handshake parses and stores `repo_version` / `protocol_version`; emits typed errors.
- `morph-cli/src/remote_helper.rs` — `Put` hash re-validation, `RefWrite` closure walk + optional `gate_check`.
- `morph-cli/src/cli.rs` + `main.rs` — `init --bare`, `config user.name|user.email`, `config policy.push.gate_branches` (the latter via the existing config plumbing).
- `morph-cli/tests/specs/config.yaml`, `merge.yaml`, `push_pull.yaml` — new spec tests.
- `morph-cli/tests/ssh_fetch_integration.rs` — bare-server, gate-on-push, evidence-union, instance-id round-trip integration tests.

## Non-goals reiterated

- Encryption, signing, or any form of cryptographic identity — out.
- A long-running daemon — out.
- Server-side migration — out (helper still rejects with `morph upgrade` hint).
- Cloning (`morph clone`) — light wrapper, can land here or in PR 7. Mentioned in the user-facing snippets but not blocking.

## Dependencies

- Builds on PR 5's `Store`-trait-over-SSH fully. `RemoteSpawn`, `LocalSpawn`, `SshStore`, `ssh_proto`, `morph remote-helper` are all assumed.
- Builds on PR 4's `start_merge` / `continue_merge`: evidence union goes into `execute_merge` only, but the path through `continue_merge` is what tests will exercise end-to-end.
- Builds on PR 3's `repo_version 0.5`: handshake's `repo_version` echo is just reading what's already there. No new repo version is introduced by PR 6.

## Risk

- **Backwards compatibility of `Commit`**: `morph_instance` is a new optional field with `skip_serializing_if = "Option::is_none"`, so old commits' canonical hashes are preserved. Tests must verify this against fixture commits from `morph-core/tests/fixtures/` (or generated freshly with `morph_instance = None` and re-hashed).
- **Bare layout ambiguity**: the helper's auto-detect of `<root>/.morph` vs bare-at-`<root>` could mis-fire. We pin both shapes with explicit `bare` flag in `config.json` and prefer the explicit answer when both could apply.
- **Server-side closure walk perf**: a fresh bare server seeing a 10k-object first push will walk all 10k. Acceptable for v0; future PRs can fast-path "all-or-nothing" pushes.
