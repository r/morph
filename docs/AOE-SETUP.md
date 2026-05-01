# Setting Up Morph with Agent of Empires

This page is the full reference for using Morph alongside [Agent of Empires (AoE)](https://github.com/njbrake/agent-of-empires) — the multi-agent session manager that runs Claude Code, OpenCode, Cursor CLI, and other coding agents in `tmux` panes (and optionally Docker sandboxes) on top of git worktrees. For a single canonical installation flow, see **[Installation](INSTALLATION.md)**.

**What you get:** every AoE session — across every agent AoE can launch — is wrapped by Morph lifecycle hooks. Session creation snapshots a Morph commit, every launch records a `Run` + `Trace`, every destroy commits a final snapshot. When AoE's per-task agent (Claude Code, OpenCode, or Cursor CLI) is one Morph already supports, in-session prompt/response recording also lands.

---

## Quick start (installation order)

1. **Install the Morph binaries** — see [Installation § Install the Morph binaries](INSTALLATION.md#1-install-the-morph-binaries).
2. **Initialize** a Morph repo *inside an existing git repo*: `morph init` — see [Installation § Initialize a Morph repo](INSTALLATION.md#2-initialize-a-morph-repo). (If your project is not a git repo yet, `morph init` will offer to run `git init` for you.)
3. **Install the AoE integration:**

```bash
morph setup aoe
```

This writes (or merges into):

| Path | Purpose |
| --- | --- |
| `.agent-of-empires/config.toml` | morph lifecycle hooks (`on_create`/`on_launch`/`on_destroy`) plus `[sandbox].environment` and `[sandbox].extra_volumes` entries |
| `.agent-of-empires/Dockerfile.morph-aoe` | reference image template for baking morph + morph-mcp into the sandbox |
| `AGENTS.md` | morph guidance picked up by AoE-launched agents |
| `.cursor/`, `.claude/`, `opencode.json`, `.opencode/plugins/morph-record.ts` | per-agent recording for any agent AoE may launch (skip with `--skip-agents`) |

Then run AoE as normal:

```bash
aoe init      # one-time per machine, optional
aoe add .     # creates a session in this repo
```

The first time AoE sees the morph hooks it will prompt you to trust them — that's [AoE's hook trust system](https://www.agent-of-empires.com/guides/repo-config/#hook-trust-system) doing its job.

---

## 1. What the hooks do

`morph setup aoe` writes a deterministic, idempotent block into `.agent-of-empires/config.toml`:

```toml
[hooks]
on_create = [
  "morph init --quiet 2>/dev/null || true",
  "morph add . && morph commit -m \"aoe-create: ${AOE_INSTANCE_ID:-unknown}\" --allow-empty-metrics 2>/dev/null || true",
]
on_launch = [
  "morph run record-session --prompt \"aoe-launch instance=${AOE_INSTANCE_ID:-unknown} branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo unknown)\" --response \"\" --model-name aoe --agent-id aoe 2>/dev/null || true",
]
on_destroy = [
  "morph add . && morph commit -m \"aoe-destroy: ${AOE_INSTANCE_ID:-unknown}\" --allow-empty-metrics 2>/dev/null || true",
  "morph run record-session --prompt \"aoe-destroy instance=${AOE_INSTANCE_ID:-unknown}\" --response \"\" --model-name aoe --agent-id aoe 2>/dev/null || true",
]
```

- **`on_create`** snapshots the worktree as a morph commit at session birth. AoE aborts session creation if a hook fails, so we tolerate a missing morph repo (`morph init --quiet ... || true`) and an empty-metrics policy (`--allow-empty-metrics`). The trailing `|| true` keeps the session creating even if the morph commit can't run for any reason.
- **`on_launch`** writes a `Run` with a tiny trace marking the session start. Failures are warnings only in AoE.
- **`on_destroy`** runs *before* AoE tears the worktree down, so it has one last chance to capture state — first as a final commit, then as a closing trace event.

Re-running `morph setup aoe` only rewrites morph-owned lines (matched by command prefix); your own `on_launch = ["npm install"]`-style entries stay put.

`${AOE_INSTANCE_ID}` is the per-session identifier AoE injects into hook environments — see the AoE config reference.

---

## 2. Sandbox: bind-mount vs. baked image

If you run AoE with Docker sandboxes (`[sandbox].enabled_by_default = true`), the hooks run *inside* the container — which means `morph` and `morph-mcp` need to exist in the container.

`morph setup aoe` ships **two ways** to make that happen, and writes the simpler one by default.

### Default: bind-mount the host binaries

The default config writes:

```toml
[sandbox]
environment = ["MORPH_WORKSPACE", "AOE_INSTANCE_ID"]
extra_volumes = [
  "/usr/local/bin/morph:/usr/local/bin/morph:ro",
  "/usr/local/bin/morph-mcp:/usr/local/bin/morph-mcp:ro",
]
```

This works with `ghcr.io/njbrake/aoe-dev-sandbox:latest` (the AoE default image) without any image build step. The downside is that you need `morph` + `morph-mcp` at exactly `/usr/local/bin/morph{,-mcp}` on the host (typical for `cargo install --path morph-cli` followed by `mv` to /usr/local/bin, or for a Homebrew install). If your binaries live elsewhere, edit the paths or switch to a baked image (next section).

The `environment` block makes AoE forward `MORPH_WORKSPACE` and `AOE_INSTANCE_ID` from your host shell into the sandbox so the hooks can find the morph repo and tag commits with the session id.

### Alternative: bake morph into a custom sandbox image

For air-gapped or repeatable deployments, build a sandbox image that bundles morph:

```bash
# From a checkout of the morph repo:
cargo build --release -p morph-cli -p morph-mcp
docker build \
  -f /path/to/your/repo/.agent-of-empires/Dockerfile.morph-aoe \
  -t aoe-morph:latest \
  .

# Then in your repo's .agent-of-empires/config.toml:
[sandbox]
default_image = "aoe-morph:latest"
# (and remove the morph entries from extra_volumes)
```

`Dockerfile.morph-aoe` documents both a `COPY target/release/...` path and a `curl <release-url>` path — pick whichever fits your release workflow.

To suppress the default bind-mount entries entirely:

```bash
morph setup aoe --no-bind-mount
```

The morph `[sandbox].environment` entries are written either way, since they're needed regardless of how the binaries get into the container.

---

## 3. Per-agent recording: cursor / opencode / claude-code

AoE can launch any of `claude`, `opencode`, `cursor`, `gemini`, `codex`, `copilot`, `droid`, or `mistral` per session. Out of these, Morph ships first-class recording for **Cursor CLI**, **OpenCode**, and **Claude Code**.

By default `morph setup aoe` invokes all three:

```text
Per-agent setups: cursor, opencode, claude-code
```

so a session running any of those agents gets prompt/response recording in addition to the lifecycle hooks. This is equivalent to running:

```bash
morph setup cursor
morph setup opencode
morph setup claude-code
```

If you want only one (e.g., your team only ever uses Claude Code through AoE):

```bash
morph setup aoe --agent claude-code
```

If you don't want any per-agent recording (for example, you're running Codex CLI through AoE and there's no per-agent integration yet):

```bash
morph setup aoe --skip-agents
```

`AGENTS.md` is seeded either way, so AoE-launched agents that read `AGENTS.md` (OpenCode, Cursor CLI, Codex CLI, …) still see morph guidance.

---

## 4. Trusting the hooks

The first time you `aoe add` or `aoe ls` a repo with new hook commands, AoE prompts:

```
This repo has unreviewed hooks. Trust them?
  on_create:
    morph init --quiet 2>/dev/null || true
    morph add . && morph commit -m "aoe-create: ..." ...
  on_launch:
    morph run record-session --prompt "aoe-launch ..." ...
  on_destroy:
    morph add . && morph commit -m "aoe-destroy: ..." ...
    morph run record-session --prompt "aoe-destroy ..." ...
[T]rust / [r]eject?
```

Type `T` once and AoE remembers the decision globally. To skip the prompt in CI:

```bash
aoe add --trust-hooks .
```

If a teammate updates `.agent-of-empires/config.toml` (including via `morph setup aoe`), AoE re-prompts — that's the trust system catching the change.

---

## 5. Verifying it works

After `morph setup aoe` and `aoe add .`:

```bash
# Confirm the hooks ran on session creation:
morph log --limit 5
# Expect to see "aoe-create: <instance-id>" in the latest commit.

# Confirm at least one Run was recorded on launch:
morph run list | head
# Expect a recent Run with prompt starting "aoe-launch instance=...".

# When you tear the session down with `aoe rm`, expect another commit
# ("aoe-destroy: ...") followed by an "aoe-destroy" Run.
```

Inside an AoE session you can also call any of the morph MCP tools directly via the agent (for example, `morph_status`, `morph_record_session`, `morph_eval_gaps`). The MCP server config landed via `morph setup cursor` / `morph setup opencode` / `morph setup claude-code`, so the per-agent integration plus the AoE lifecycle hooks reinforce each other.

---

## 6. Re-running setup

`morph setup aoe` is idempotent. Running it again:

- Rewrites only morph-owned hook lines, sandbox env keys, and bind-mount volumes (matched by content prefix).
- Preserves anything you've added by hand: `[session]`, `[worktree]`, your own `on_launch`, your `default_image`, your `extra_volumes` for `/data:/data:ro`, etc.
- Re-emits `Dockerfile.morph-aoe` and `AGENTS.md` deterministically; if you edited them, your changes will be overwritten — copy them somewhere else first.

To preview the diff without writing:

```bash
git diff -- .agent-of-empires AGENTS.md .cursor .opencode .claude opencode.json
```

---

## 7. CLI reference

```text
$ morph setup aoe --help
Install Agent of Empires (`aoe`) integration: per-repo
`.agent-of-empires/config.toml` with morph lifecycle hooks +
sandbox env/volume entries, a baked-image Dockerfile reference,
AGENTS.md guidance, and (by default) per-agent recording for any
of cursor/opencode/claude-code that AoE may launch.

Usage: morph setup aoe [OPTIONS]

Options:
      --path <PATH>      [default: .]
      --agent <AGENT>    Per-agent integrations to install. Repeatable.
                         One of: cursor, opencode, claude-code.
      --skip-agents      Skip per-agent delegation entirely.
      --no-bind-mount    Don't seed [sandbox].extra_volumes with bind-mounts
                         for the host morph binaries.
      --no-dockerfile    Don't write `.agent-of-empires/Dockerfile.morph-aoe`.
  -h, --help             Print help
```

---

## See also

- [Cursor setup](CURSOR-SETUP.md)
- [OpenCode setup](OPENCODE-SETUP.md)
- [Claude Code setup](CLAUDE-CODE-SETUP.md)
- [Agent of Empires repo-config docs](https://www.agent-of-empires.com/guides/repo-config/)
