# Multi-Machine Workflows

This guide shows you how to share a Morph repository between two or more machines. The model should feel familiar if you know Git: you have a repo on each machine, you push to and pull from a shared **bare** server, and you resolve any merge conflicts locally before continuing.

What's different — and the reason this guide exists — is that Morph carries more than a file tree across machines. It also moves the **runs and traces** that prove the code works, the **eval suite** the code claims to satisfy, and the **per-machine identity** that lets you tell who pushed what. The transport is SSH, the storage on the server is a bare Morph repo, and the merge engine on each client is the same one documented in [MERGE.md](MERGE.md).

If you only need a recipe, jump to the [common scenarios](#common-scenarios). If you want to understand the failure modes (drift, divergence, schema mismatch, push gates) read [How it works](#how-it-works) first.

---

## Common scenarios

### A. Two laptops, your own server

You own a Mac mini and want both your laptops to share the same Morph repo.

**On the server (Mac mini)**:

```bash
ssh you@homelab
mkdir -p ~/repos/myproject.morph
morph init --bare ~/repos/myproject.morph
```

A bare repo has no working tree and no `.morph/` wrapper — its files (`objects/`, `refs/`, `config.json`, ...) live directly at the path you passed. That's the only kind of repo you should `morph push` to from multiple clients.

**On laptop A** (the one with the existing project):

```bash
cd ~/code/myproject
morph remote add origin you@homelab:repos/myproject.morph
morph push origin main
```

`morph remote add` accepts both `ssh://user@host[:port]/path` URLs and `user@host:path` scp-style shortcuts. Plain filesystem paths still work for local-only setups.

**On laptop B** (a fresh checkout):

```bash
morph clone you@homelab:repos/myproject.morph myproject
cd myproject
```

`morph clone` is one-shot: it `init`s the destination, configures `origin`, fetches every branch, checks out the default branch, and wires up the upstream so `morph sync` works immediately. Pass `--branch <name>` to start on a topic branch, or `--bare` to seed a second server from an existing one.

If you'd rather assemble it by hand (e.g. you're scripting a migration) you can run the equivalent steps explicitly:

```bash
mkdir myproject && cd myproject
morph init
morph remote add origin you@homelab:repos/myproject.morph
morph fetch origin
morph branch --set-upstream origin/main
morph checkout main
```

From this point on, on either laptop:

```bash
# day-to-day:
morph add .
morph commit -m "..."
morph sync                 # fetch + fast-forward (or merge) origin/main
morph push origin main
```

`morph sync` reads the upstream you configured with `--set-upstream` and does the equivalent of `morph fetch` followed by `morph pull --merge` against that upstream. Configure it once per branch.

### B. Sharing with a teammate

Same shape as scenario A, just with somebody else's account on the server. Teammate runs the same `morph init --bare` once, gives you SSH access, and you both `morph remote add origin <their-host>:<path>`. Morph pushes are not "force"-style by default: a push that isn't fast-forward is rejected, exactly like Git's default behavior. To recover, fetch, merge locally, and push again.

### C. Solo work, multiple branches

You don't need a server at all if you're working on one machine and just want to track multiple lines of work — that's plain `morph branch <name>` / `morph checkout`. You only need the SSH workflow when objects must travel between filesystems.

### D. Mixed Git + Morph

`morph push` to a Morph remote is independent of `git push` to a Git remote. Most people who use both run `git push origin main` and `morph push origin main` back-to-back, often via a shell alias. The `MORPH-AND-GIT.md` document covers this in more detail.

---

## How it works

### Identity: who pushed what

A Morph commit carries two identity fields:

- `author` (string) — the **human** identity, resolved from `morph commit --author`, then `MORPH_AUTHOR_NAME` / `MORPH_AUTHOR_EMAIL`, then `morph config user.name` / `user.email`, then the `"morph"` default. Set yours once per machine:

  ```bash
  morph config user.name  "Alice Example"
  morph config user.email "alice@example.com"
  ```

- `morph_instance` (string, optional) — the **machine** identity, generated automatically by `morph init` and stored in `.morph/config.json` under `agent.instance_id` (e.g. `morph-3f9a2c`). This is what tells you *"this commit was made on laptop A"* even when both laptops share an author.

Two machines that commit the same content will produce **different commit hashes** because their `morph_instance` differs. That's intentional — it makes provenance traceable across machines without you having to remember to set anything. If you genuinely want bit-identical commits across machines (for reproducibility experiments) you can copy `.morph/config.json` between them, but in practice you almost never want to.

### Closure: what a push actually moves

When you run `morph push origin main`, Morph walks the **reachable closure** of the new tip:

```
commit  ──► tree, pipeline, eval_suite
   │           │       │       │
   │        blobs   nodes    cases
   │
   └──► parents (recursively)
   │
   └──► evidence_refs ──► runs ──► traces, artifacts
```

Every object reachable from the tip is serialized and sent to the server. The server runs `verify_closure` (PR 6 stage F) on the received objects: if any reachable object is missing it refuses the `RefWrite`. That makes partial pushes impossible — either every dependency lands and the ref moves, or nothing changes server-side.

`morph fetch` is the inverse: the server sends the closure of every branch the client doesn't already have, the client stores it, and remote-tracking refs (`refs/remotes/origin/<branch>`) are updated.

### Divergence: what happens when both sides commit

The interesting case is when laptop A and laptop B both commit on `main`, then both try to push. The first one succeeds. The second one's push is rejected because it isn't fast-forward — the server's `main` is no longer where the second client thought it was. The fix:

```bash
morph fetch origin
morph pull origin main --merge   # or: morph sync
# resolve conflicts (see MERGE.md), then
morph add <resolved-files>
morph merge --continue
morph push origin main
```

`morph pull --merge` runs the same engine documented in [MERGE.md](MERGE.md): three-way structural merge of tree / pipeline / eval suite, behavioral dominance check, and evidence union (PR 6 stage C). The merge commit's `evidence_refs` is the deduped sorted union of both parents', so the runs that backed up either side are still reachable after the merge.

### Schema handshake

Every SSH session starts with a `Hello` exchange. Server replies with:

```
{"version": "0.17.0", "protocol_version": 1, "repo_version": "0.5"}
```

The client compares `protocol_version` against its own. On mismatch you get:

```
error: Incompatible remote: remote protocol_version=2, local protocol_version=1
```

This is `MorphError::IncompatibleRemote`. It surfaces at the very first request and tells you which side to upgrade. Legacy helpers that don't advertise `protocol_version` are accepted silently — that's the one-release overlap built into PR 6.

### Server-side push gating

If the server admin has configured a push gate on the branch you're pushing to, your commit must pass `gate_check` *on the server* before the ref moves. Failure looks like:

```
error: push gate failed for branch 'main': commit is not certified
```

Gate patterns can be globs — `release/*` covers every single-segment release branch — so the message will name your concrete branch (e.g. `release/v1.0`) even when the policy uses a wildcard. See [SERVER-SETUP.md](SERVER-SETUP.md) for how gates are configured; from a client perspective the cure is to certify your commit (`morph certify --metrics-file …`) and push again.

---

## Reference: the commands you'll use

| Command | Purpose |
|---|---|
| `morph init [--bare] [path]` | Create a working repo or bare server repo |
| `morph remote add <name> <path-or-ssh-url>` | Register a remote |
| `morph remote list` | List configured remotes |
| `morph push <remote> <branch>` | Send the branch's closure; fast-forward only |
| `morph fetch <remote>` | Update `refs/remotes/<remote>/*` |
| `morph pull <remote> <branch>` | `fetch` + fast-forward local; errors on divergence |
| `morph pull <remote> <branch> --merge` | `fetch` + 3-way merge on divergence |
| `morph branch --set-upstream <remote>/<branch>` | Configure per-branch upstream tracking |
| `morph sync [branch]` | `fetch` + `pull --merge` against the configured upstream |
| `morph config user.name <value>` | Set human identity for commits |
| `morph config user.email <value>` | Set human identity for commits |

Environment variables that matter for SSH:

| Variable | Default | Use |
|---|---|---|
| `MORPH_SSH` | `ssh` | Override the SSH binary used for transport (handy in tests / CI) |
| `MORPH_REMOTE_BIN` | `morph` | The remote-side binary the helper invokes; set if the server's `morph` isn't on `$PATH` |

---

## Troubleshooting

**"Diverged: local <hash> vs remote <hash>"**
You and another machine both committed; pull with `--merge` (or run `morph sync`), resolve, push.

**"Incompatible remote: remote protocol_version=2, local protocol_version=1"**
Upgrade the older side. Both client and server need a Morph release whose `MORPH_PROTOCOL_VERSION` overlaps.

**"push gate failed for branch 'main': …"**
The server has policy gating this branch. Look at the message, certify the commit accordingly, and retry. See [SERVER-SETUP.md](SERVER-SETUP.md).

**"NotFound: <hash>"** during a push
The closure on your side is incomplete — usually because you're pushing a commit whose evidence references runs you haven't ingested. Run `morph run record …` (or `morph eval record …`) first.

**"failed to spawn ssh"**
Either `ssh` isn't on `$PATH` or the SSH connection itself failed. Try the same SSH command by hand: `ssh user@host morph remote-helper --repo-root /abs/path/to/bare`. The hidden `remote-helper` subcommand is what Morph drives over the SSH session; if it doesn't exist on the server, you're running an older Morph release server-side.

**Plain filesystem remote on a shared drive**
`morph remote add origin /mnt/shared/myproject.morph` still works. The whole SSH machinery is bypassed; you just need both clients to be able to read and write the bare-repo files. This is fine for personal multi-machine setups over NFS / iCloud / Dropbox, but the SSH path is what Morph exercises in tests and is recommended for anything beyond one user.
