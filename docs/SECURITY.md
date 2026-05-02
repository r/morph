# Security & privacy

> **Morph records everything.** That is the design point —
> reviewability, replay, attribution, prompt-as-spec, and merge-aware
> behavioral context all depend on it (see
> [`SESSION-TRACKING.md`](SESSION-TRACKING.md) for the why). The
> tradeoff is that traces contain whatever happened in your agent
> session — prompts, model responses, file contents the agent read,
> shell stdout/stderr — and you should know that before you let any
> of it leave your laptop.
>
> This document is the plain answer to "is morph safe?" — what
> morph records, what crosses the wire when, what is and isn't yet
> built, and what to do before you share traces with a team.

---

## What morph records, said plainly

When you run morph alongside an agent (Cursor, Claude Code, OpenCode,
AoE, or anything that talks to `morph-mcp`), morph captures every
agent session as an immutable `Run` + `Trace` pair. A typical trace
includes:

- The **prompt** you (or the rule/hook) sent the agent — verbatim.
- The agent's **responses** — verbatim.
- Every **tool call** the agent made — name, arguments, return value.
  This includes `read(<path>)` calls whose return value is *the file
  contents the agent read*.
- Every **file edit** — old and new contents, or a diff.
- Every **shell command** the agent ran, plus its **stdout and
  stderr** — verbatim, including secrets the program may have
  leaked there.
- The **environment** — current working directory, git branch, host
  OS, model id, model version, tokenizer, token counts.
- A timestamped sequence so the trace can be replayed.

Plus on top of that, the morph commit itself stores:

- The **file tree snapshot** that git would store (the same
  content-addressed tree).
- An **evaluation contract** — which pipeline ran, which suite, what
  metrics were observed, in which environment.
- **Evidence references** — pointers from the commit back to the
  Run and Trace objects above.

Translation: **everything the agent saw, did, or said** is recorded.
If the agent read your `.env`, the contents of `.env` are in the
trace. If the agent ran `aws sts get-caller-identity`, the AWS
account id is in the trace. If you pasted production data into a
prompt, the production data is in the trace. Morph does not redact;
morph records.

We are explicit about this so you can make an informed choice
before you point morph at sensitive code.

## Where it lives, and what's in scope

Inside any morph repo, the layout is:

```
your-project/
  .git/                  # git's objects, refs, config
  .morph/
    objects/             # commits, blobs, pipelines, eval suites
    runs/                # one JSON per agent run (metrics + pointers)
    traces/              # one JSON per trace (the event log)
    prompts/             # prompts referenced from traces
    refs/                # branches, tags
    config.json          # repo-local config (repo_version, repo_submode, init_at_git_sha, policy, …)
  src/
  ...
```

Two facts you should hold in your head about this layout:

1. **`.morph/` is *never* tracked by git.** `morph init` adds
   `.morph/` to `.git/info/exclude` automatically. Even if you
   never read this doc, an unrelated teammate cloning your git
   repo will not see your runs and traces.
2. **`.morph/` is *not encrypted at rest*.** It's plain JSON. A
   process running as your user can read it. Anyone with a backup
   of your home directory can read it. We made the same call your
   agent's on-disk transcripts already made — `~/.claude/projects/...`
   on Claude Code, Cursor's session JSONL, OpenCode's session log,
   AoE's per-task journals — all are unencrypted on disk by
   default. If your threat model needs encryption at rest, use
   FileVault / dm-crypt / a similarly hardened laptop; morph
   inherits whatever protection your filesystem already provides.

If you have something morph absolutely must not record, the answer
is **don't put it in front of an agent that morph is observing.**
Hooks and the MCP record what the agent sees; they cannot redact
what they're not aware is sensitive.

## What crosses the wire when

This is the most important section for teams.

### `git push` — code only, never traces

```
$ git push origin main
```

Pushes the git tree to your git remote (GitHub, GitLab,
self-hosted). **`.morph/` is excluded from git tracking, so the git
push physically cannot include runs, traces, or prompts.** Your
teammate's `git pull` gives them an ordinary git working tree.

This is the safety net: even if you ignore everything else in this
doc, your git workflow does not leak agent state.

### `morph push` — opt-in, separate channel

```
$ morph remote add team /path/to/shared/morph-repo
$ morph push team main
```

Pushes the morph DAG (commits, pipelines, suites, **runs, traces,
prompts**) to a *morph remote*. Morph remotes are independent from
git remotes; they are configured separately, named separately,
and authenticated separately. The default install does not
configure a morph remote. Behavioral history goes nowhere unless
you explicitly point it somewhere.

When you do push, you are pushing whatever was in `.morph/runs/`
and `.morph/traces/` for the commits in scope. Specifically:
prompts you sent, responses you got back, file contents the agent
read, shell stdout/stderr, and the model parameters used. **There
is no client-side filter today.** If you don't want a trace going
to the remote, you must remove it before push. (See "Things morph
does not yet do" below.)

### `morph fetch` / `morph pull` — symmetric

A teammate running `morph fetch team` pulls the runs and traces
you pushed. They land in their `.morph/`, addressable by hash. The
teammate can `morph inspect show <hash>` and read your prompts,
responses, and shell output verbatim.

### Two-channel model, drawn

```
                      git remote (GitHub/GitLab)
                               ▲       ▲
              git push only    │       │   ordinary teammates
              ──────────────── │       │ ────────────────────
                     code      │       │      git pull
                               │       │
              ┌────────────────┴───────┴─────────────────┐
              │                                          │
              │  your laptop                             │
              │  ┌────────┐  ┌──────────────────────┐    │
              │  │  .git  │  │  .morph              │    │
              │  └────────┘  │  objects/            │    │
              │              │  runs/   traces/     │    │
              │              │  prompts/            │    │
              │              └──────────────────────┘    │
              └─────────────────────────┬────────────────┘
                                        │
                                        │  morph push (opt-in)
                                        ▼
                              morph remote (separate)
                              ▲          ▲
              morph fetch     │          │   trusted teammates only
              ─────────────── │          │ ────────────────────────
                runs+traces   │          │   morph fetch
```

Think of git as the public commons and morph as the private
working notebook. Sharing the notebook is intentional, not silent.

## Recommended team setup

For a team that wants the benefits of shared traces without the
"oops, my prompts went to the contractor" failure mode:

1. **Code goes through your existing git remote.** No change.
2. **Behavioral history goes through a *separate* morph remote**,
   accessible only to people you'd trust to read your IDE history.
   For most teams this means full-time staff; for some it means a
   subset of full-time staff. Set the remote up explicitly:
   ```
   morph remote add team ssh://team-server/morph/repo
   morph push team main
   ```
3. **Agree on what gets pushed.** If your traces routinely include
   environment variables, customer data, or proprietary prompts,
   either don't `morph push` automatically, or restrict the remote
   to a small group, or use a per-feature branch you only push when
   you've eyeballed it.
4. **Treat the morph server like a code-search index, not like a
   public README.** Access controls on the morph remote should be
   at least as tight as access controls on your AWS account; the
   trace data is at least as sensitive.

## `morph forget`

*Introduced in v0.41.0.*

`morph forget <hash>` is the first-class way to permanently retire
a `Run`, `Trace`, or prompt `Blob` from your local store and
propagate the deletion to teammates who pull from the same morph
remote. It is the answer to "I leaked a secret into a trace; what
do I do?"

### When to use it

- A prompt or response captured an API key, password, or other
  credential that you don't want sitting in `.morph/`.
- An agent's `read(<file>)` call slurped a sensitive file
  whose contents are now in the trace.
- A shell stdout/stderr leaked production data.
- Compliance or DPA obligations require explicit deletion of
  named-entity content.

### How it works

1. **Local retirement.** `morph forget <hash>` writes an
   immutable `Tombstone` object recording the actor / reason /
   timestamp, deletes the original `objects/<hash>.json`, scrubs
   the per-type index entries, and writes a
   `.morph/forgotten/<hash>.txt` marker pointing at the tombstone.
2. **Audit trail preserved.** The tombstone is itself a
   content-addressed object — `morph show <tombstone-hash>`
   returns who retired what, when, and why. The original bytes are
   gone; the *fact* of the deletion is permanent.
3. **Remote propagation.** With `--remote <name>`, the next
   `morph push <name> <branch>` ships the tombstone alongside any
   normal objects. A teammate's `morph fetch <name>` automatically
   applies the tombstone on receipt: it deletes the original
   locally if present, scrubs the indexes, and writes the same
   `.morph/forgotten/<hash>.txt` marker.
4. **Merge gate is tolerant.** If a commit's `evidence_refs`
   names a tombstoned hash, `morph merge` reads that reference as
   "no claim" rather than a hard error and emits a one-line
   warning. Forgetting evidence does not retroactively break
   commits.

### Refusals — what `morph forget` will not do

- **Forget a `Commit`, `Tree`, regular `Blob`, `Pipeline`,
  `EvalSuite`, `Artifact`, `TraceRollup`, or `Annotation`.**
  These all carry structural meaning the version-control DAG
  depends on. The CLI refuses with a clear "not a forgettable
  kind" message.
- **Forget a hash that's named in a commit's `evidence_refs`,
  unless `--force` is set.** The default refuses and lists the
  referencing commits so the operator can audit the impact
  first. `--force` opts past the check; the merge gate then
  treats those refs as "no claim".
- **Run non-interactively without `--yes`.** A non-TTY caller
  must opt in to silent mode explicitly; this stops a runaway
  script from forgetting the wrong hash. Interactive callers
  type `forget` to confirm.

### What forget still does *not* cover

- **Already-fetched copies on other laptops.** A teammate who
  pulled the trace before your `morph forget --remote` push still
  has the bytes. The next `morph fetch` from the remote will
  apply the tombstone, but **data on disk before the fetch is
  the teammate's choice to delete**. Out-of-band ask remains the
  honest answer for "deleted data on N already-cloned machines."
- **Partial redaction.** `morph forget` is whole-object only.
  You cannot edit out a single secret from a trace and keep the
  rest — the resulting "trace" would have a different hash and
  would be indistinguishable from a fabrication.
- **Forgetting commits / blobs / trees / pipelines / suites /
  artifacts / rollups / annotations.** See above.
- **SSH transport does not yet carry tombstones.** Tombstones
  travel through filesystem morph remotes (the common bare-repo
  / shared-filesystem case) but the SSH remote-helper protocol
  does not yet ship them. Until that protocol extension lands,
  the recipe for SSH-served remotes is "ssh into the remote
  and run `morph forget` there too." The morph-core unit tests
  pin the local + apply-tombstone round-trip; the SSH path will
  inherit it once the protocol upgrade ships.
- **MCP tool.** `morph forget` is CLI-only today; an
  `morph_forget` MCP tool is on the roadmap.

### Recipe — "I leaked a secret"

```
# 1. find the run/trace that holds the secret:
morph run list                       # newest runs first; copy a hash
morph inspect show <run-hash>        # see the prompts/tool calls/files in that run

# 2. forget it locally and queue for the team remote:
morph forget <run-or-trace-hash> \
    --remote team \
    --reason "leaked db password; rotated"

# 3. ship the tombstone:
morph push team main

# 4. rotate the secret. (this is the only step that actually
#    saves you — once a secret is shared, deletion alone is
#    never enough.)

# 5. teammates pull and the tombstone is applied automatically:
#    (no action needed on their side beyond the next fetch)
morph fetch team
```

## Things morph does *not* yet do (everything else)

The following are honest gaps. Some are queued for the next
release line; some are roadmap for later.

- **No client-side redaction filter on `morph push`.** Today push
  is "send everything that's reachable from this commit". A
  redaction-on-push hook is a roadmap item.
- **No selective fetch.** `morph fetch team` pulls the full DAG.
  We don't yet have "fetch only this commit, not the trace
  attached".
- **No encryption at rest in `.morph/`.** Same posture as every
  agent tool's on-disk transcripts. Use disk encryption.
- **No transport-layer guarantees other than what your remote
  gives you.** SSH morph remotes inherit OpenSSH's security; a
  bare-repo morph remote on a shared filesystem inherits that
  filesystem's posture. There's no morph-specific PKI.
- **No automatic secret scanning.** Morph does not look at the
  trace bytes for tokens, API keys, or PII before recording or
  pushing. Use the agent-level guardrails your IDE provides.
- **`morph forget` SSH propagation.** Local-filesystem remotes
  carry tombstones today; SSH-served remotes will once the
  remote-helper protocol upgrade ships (see *`morph forget`*
  above for the workaround).

We say this loud so you don't have to figure it out by reading
the source.

## Before you share traces with a team — a checklist

1. Have you set up a *separate* morph remote that is **not** the
   same as your git remote?
2. Is access to the morph remote at least as tight as access to
   your secrets manager / cloud provider?
3. Have you at least browsed `.morph/traces/` for the commits
   you're about to push? `morph inspect show <run-hash>` is the
   built-in viewer.
4. Are you comfortable that any prompts, responses, file contents,
   or shell output in those traces *can* end up on every teammate's
   laptop after `morph fetch`?
5. If a teammate later leaves the project, what's your plan for
   their already-fetched copy of the trace data? (Today the
   honest answer is "you can't get it back"; `morph forget`
   makes the *future* sharing answer "tombstone propagates on
   next fetch", but already-cloned data stays cloned.)

If you can answer those, you're in good shape. If you can't,
keep `morph push` to a private remote-of-one until you can.

## If you leak a secret into a trace

The recipe lives in *`morph forget`* above. In short:

```
morph forget <run-or-trace-hash> --remote team --reason "leaked db password"
morph push team main
# teammates fetch from the remote; the tombstone is applied
# automatically:
morph fetch team
```

…then **rotate the secret** — that step is the one that actually
saves you. Once a credential is in someone else's hands (or in a
mirror you don't control), deletion alone is never enough.

---

## See also

- [`SESSION-TRACKING.md`](SESSION-TRACKING.md) — why morph records
  every agent session, why the alternatives can't deliver the same
  contract, and the seven first-class things traces unlock.
- [`MORPH-AND-GIT.md`](MORPH-AND-GIT.md) — how morph wraps git in
  reference mode (the only mode since v0.40.0), and why `.morph/`
  is excluded from git automatically.
- [`MULTI-MACHINE.md`](MULTI-MACHINE.md) — sharing a morph repo
  across machines: bare server, SSH transport, push/pull/sync.
