# Server Setup

This guide is for the person standing up a Morph server: a machine that other people (or other machines belonging to the same person) will `morph push` to and `morph fetch` from. If you're a *user* of an existing server, see [MULTI-MACHINE.md](MULTI-MACHINE.md) instead.

A Morph server is just an SSH-reachable host with a **bare repo** on disk and the `morph` binary in the SSH user's `$PATH`. There's no daemon to run, no port to open beyond SSH, no database to manage. The wire protocol is JSON-RPC over an SSH session driven by the hidden `morph remote-helper` subcommand.

---

## 1. Prerequisites

- An OS where Rust binaries run cleanly (Linux and macOS are tested).
- An SSH server (`sshd`) listening on the network the clients can reach.
- A user account on the host whose login shell can exec `morph`.
- The `morph` binary built from this repo (`cargo install --path morph-cli` or your distribution's package). The version on the server should be **at least as new** as the clients' to avoid `IncompatibleRemote` errors.

Verify on the server:

```bash
morph --version
# morph X.Y.Z (built ...)
```

The server doesn't need any of the IDE integrations (`morph-mcp`, hooks). It only needs the CLI.

---

## 2. Create the bare repository

A **bare** Morph repo is a repo with no working tree and no `.morph/` wrapper. Its contents (`objects/`, `refs/`, `config.json`, ...) live directly at the path you give. This is the only kind of repo you should accept pushes into from multiple clients — pushing into a working repo would race with whatever is editing the working tree there.

```bash
ssh you@server
mkdir -p ~/repos/myproject.morph
morph init --bare ~/repos/myproject.morph
```

What got created:

```
~/repos/myproject.morph/
├── config.json          # repo_version, agent.instance_id, "bare": true
├── objects/             # content-addressed object store
├── refs/
│   ├── heads/           # branch tips
│   └── remotes/
├── prompts/
├── runs/
├── traces/
├── evals/
└── annotations/
```

There's no `.morph/` wrapper and no `.gitignore` because there's nothing to ignore. The convention is to suffix the directory name with `.morph` so users browsing the server can see at a glance what kind of repo it is.

---

## 3. Grant SSH access

The clients reach the server with:

```bash
ssh user@server morph remote-helper --repo-root /abs/path/to/myproject.morph
```

That's literally what `morph push` / `morph fetch` invoke. So all you need to grant somebody push rights is a normal SSH login. The simplest setup:

1. Append the user's public SSH key to `~/.ssh/authorized_keys` on the server.
2. Make sure their SSH login can find `morph` (either install system-wide, or set `PATH` in the SSH user's shell rc, or use `MORPH_REMOTE_BIN` on the client to point at an absolute path on the server).
3. Make sure they have write access to the bare repo directory.

For tighter security you can pin them to the repo with a `command="…"` entry in `authorized_keys`:

```
command="morph remote-helper --repo-root /home/morph/repos/myproject.morph",no-port-forwarding,no-X11-forwarding,no-agent-forwarding,no-pty ssh-ed25519 AAAA... alice@laptop
```

That key can do exactly one thing: drive the remote helper for that specific bare repo. Any other SSH command (`ls`, `cat`, …) will be replaced with `morph remote-helper` and rejected for not being a valid remote-helper request.

---

## 4. Configure server-side policy

Server-side policy lives in the bare repo's `config.json` under the `"policy"` key. You manage it with `morph policy` from the server (or by editing the JSON directly when you need to script it).

A typical workflow: edit a JSON file with the policy shape, then run `morph policy set` to load it.

`policy.json`:

```json
{
  "required_metrics": ["pass_rate"],
  "thresholds": {
    "pass_rate": 1.0,
    "mean_latency_ms": 500
  },
  "directions": {
    "pass_rate": "maximize",
    "mean_latency_ms": "minimize"
  },
  "merge_policy": "dominance",
  "push_gated_branches": ["main"],
  "default_eval_suite": null,
  "ci_defaults": {}
}
```

```bash
cd ~/repos/myproject.morph
morph policy set policy.json
morph policy show
```

What each field controls:

| Field | What it does |
|---|---|
| `required_metrics` | Names that must be present in a commit's `eval_contract.observed_metrics` for `gate_check` to pass |
| `thresholds` | Per-metric numerical floor (or ceiling, depending on direction) |
| `directions` | `"maximize"` (default) or `"minimize"` for each metric |
| `default_eval_suite` | Suite hash certification falls back to when not specified explicitly |
| `merge_policy` | `"dominance"` (default) enforces behavioral dominance on merges; `"none"` skips it |
| `ci_defaults` | Free-form metadata stamped onto certifications |
| `push_gated_branches` | **Server-side**: branch names that must pass `gate_check` before `RefWrite` is accepted. Empty = no gating (legacy behavior) |

`push_gated_branches` is the PR 6 mechanism that turns the server into a quality gate. Each entry is a branch-name pattern (no `refs/heads/` prefix). Patterns understand:

- `*` — zero or more non-`/` characters. `release/*` matches `release/v1.0` but not `release/v1/hotfix`. Top-level `*` matches every single-segment branch.
- `?` — exactly one non-`/` character.
- everything else literal.

Plain names like `"main"` keep their pre-PR9 exact-match meaning, so existing policies upgrade with no behavior change. Common shapes:

```json
"push_gated_branches": ["main", "release/*", "hotfix/*"]
```

Multi-component branches need an explicit deeper pattern (e.g. `"release/*/*"` or `"release/*/hotfix"`); a single `*` segment is the boundary by design.

When somebody runs `morph push origin main`, the server-side helper runs:

1. `verify_closure` — every reachable object is present.
2. If the branch is in `push_gated_branches`, `enforce_push_gate`:
   - read the policy from the bare repo's `config.json`,
   - call `gate_check` against the new tip,
   - if it fails, return `MorphError::Serialization("push gate failed for branch '<name>': <reasons>")`.
3. Only on success is the ref actually written.

Failures bubble back to the client as a `morph push` error. The client's `morph push` exits non-zero with a stderr message that includes the reasons.

---

## 5. The schema handshake

Every connection starts with a `Hello` exchange:

```
client → {"op": "Hello"}
server → {"version": "X.Y.Z", "protocol_version": 1, "repo_version": "0.5"}
```

- `protocol_version` is a single integer (`MORPH_PROTOCOL_VERSION` in `morph_core::ssh_proto`). Clients that speak a different version reject the session with `IncompatibleRemote`.
- `repo_version` is the on-disk store version (`0.5` for PR 6). Clients can use this to decide whether to migrate.
- `version` is the server binary's marketing version. Informational only; **not** used for compatibility checks.

If a client and server need to coexist across a `protocol_version` bump, you have one release of overlap: a server that doesn't include `protocol_version` in its `Hello` is treated as compatible, and a client that doesn't validate the field is treated as legacy. After that overlap window, mismatch becomes a hard error and the older side has to upgrade.

To diagnose protocol issues from the server side, drive the helper directly:

```bash
echo '{"op":"Hello"}' | morph remote-helper --repo-root /abs/path/to/bare
```

The first stdout line is the `Hello` response. If you see a `protocol_version` your clients don't understand, upgrade them; if your clients advertise something newer, upgrade the server.

---

## 6. Multiple repos on one host

A server can host as many bare repos as you have disk for. There's nothing special about hosting multiple — each is independent, each has its own policy, each is reached via its own `--repo-root`:

```
~/repos/
  alpha.morph/
  beta.morph/
  gamma.morph/
```

Clients pick which one with their remote URL:

```bash
morph remote add alpha you@server:repos/alpha.morph
morph remote add beta  you@server:repos/beta.morph
```

Per-repo policy lives in each bare repo's own `config.json`, so different repos can have different push gates, different default eval suites, etc.

---

## 7. Backup and recovery

A bare Morph repo is a directory of plain JSON files (or Git-style hashed blobs in the 0.4+ store). Treat it like any other directory:

- `rsync -a ~/repos/myproject.morph/ backup-host:/.../` — straight file copy.
- `tar czf myproject-$(date +%F).morph.tgz ~/repos/myproject.morph` — point-in-time archive.
- ZFS / Btrfs snapshots are a great fit: the store is content-addressed, so snapshots are nearly free in space.

Restoration is just untar + restart accepting pushes. There is no journal to replay and no lock file to worry about (Morph reads/writes individually-named object files; concurrent readers are fine).

If you need an offline copy from a client:

```bash
# on a client that already has the closure:
morph fetch origin
rsync -a ~/.morph-mirror/ remote-host:/.../   # if you keep a local mirror
```

A full client checkout *is* a complete backup, because every object reachable from the client's tracked branches is in its local store.

---

## 8. Upgrades

The general path is **upgrade the server first**:

1. Stop accepting writes (close SSH, briefly).
2. `cd ~/repos/myproject.morph && morph upgrade` — if the upgrade requires a store-version migration, this runs it in place.
3. Roll the new `morph` binary out to clients.

If the new release bumps `MORPH_PROTOCOL_VERSION`, clients still on the old version will get `IncompatibleRemote` errors against the new server until they upgrade. That's by design — refusing to talk is safer than silently sending garbled bytes.

To check what version a server is currently running:

```bash
ssh you@server morph --version
```

To check the on-disk repo version:

```bash
ssh you@server cat ~/repos/myproject.morph/config.json | grep repo_version
```

---

## 9. Troubleshooting

**Clients see "failed to spawn ssh"**
That's a client-side message — your server is fine; their `MORPH_SSH` (or default `ssh`) couldn't reach the host.

**Clients see "Incompatible remote: remote protocol_version=N, local protocol_version=M"**
The client and server are on different `MORPH_PROTOCOL_VERSION`. Tell them to upgrade their `morph` to match yours, or downgrade yours.

**Pushes succeed but ref doesn't move**
Almost always a closure problem. Run `morph remote-helper --repo-root <path>` by hand against a `RefWrite` request and watch the response. With PR 6 stage F, the helper rejects writes whose closure isn't fully present, so a missing-object problem on the client should now surface as a clear `NotFound` error rather than a phantom successful push.

**`push gate failed for branch 'main'`**
The push-gate mechanism is doing its job. Check `morph policy show` on the server to see what's required, and have the client either (a) certify the commit (`morph certify --metrics-file ...`), (b) push to a non-gated branch, or (c) lobby you for a policy change. The reasons in the error message tell you which check failed (missing metrics, threshold, missing certification annotation, etc.).

**Disk full on the server**
Run `morph gc` from the server inside the bare repo. It removes objects unreachable from any ref. If you're seeing pathological growth, inspect what's referenced by `runs/` and `traces/` — those are usually the bulkiest objects in a busy repo.

**Auditing who pushed what**
Every commit has `author` (human) and `morph_instance` (machine) fields. `morph log` and `morph show <hash>` print both. You can grep `objects/` for a specific instance ID to find every commit a particular machine produced. Annotations are mutable metadata you can attach to commits without altering their hash — use them for audit trails (`morph annotate <hash> -k audit -d ...`).

---

## 10. Reference

- Hidden subcommand serving SSH sessions: `morph remote-helper --repo-root <path>`
- Policy CLI: `morph policy show` / `morph policy set <file.json>` / `morph policy set-default-eval <hash>`
- Wire protocol module: `morph-core/src/ssh_proto.rs`
- Server-side validation: `morph-core/src/sync.rs::verify_closure`, `morph-core/src/policy.rs::enforce_push_gate`
- Bare repo helpers: `morph-core/src/repo.rs::init_bare`, `is_bare`, `resolve_morph_dir`
- Wire constant: `morph_core::ssh_proto::MORPH_PROTOCOL_VERSION`
