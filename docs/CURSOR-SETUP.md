# Setting up Morph with Cursor

This guide assumes you’re coming from **Cursor**: we walk through **building Morph**, **installing the CLI and MCP server** to a standard location (e.g. `/usr/local/bin` on Linux, same or `~/bin` on macOS), then **configuring Cursor** and **initializing** your project. After that you can record Cursor sessions as Runs and commit your filesystem via Morph.

**What you get:**

- **Session evidence** — Prompts and model replies can be stored as **Runs** (with optional **Traces**). Morph ingests this data; it does not run the model.
- **Explicit filesystem commits** — When you want a snapshot, you **stage** and **commit** in Morph (MCP tools or CLI), separate from Git.

---

## 1. Build and install Morph (CLI + MCP server)

You need two binaries: **morph** (CLI) and **morph-mcp** (MCP server Cursor will run). Build them from the Morph repo, then install to a location on your `PATH`.

### 1.1 Prerequisites

- **Rust** (e.g. [rustup](https://rustup.rs/)).

### 1.2 Clone and build

```bash
git clone https://github.com/your-org/morph   # or your Morph repo URL
cd morph
cargo build -r
```

This produces a release build of the whole workspace. The binaries you need are:

- `target/release/morph` — CLI
- `target/release/morph-mcp` — MCP server for Cursor

### 1.3 Install to a standard location

Pick one of the following so both `morph` and `morph-mcp` are on your `PATH`. Cursor must be able to run `morph-mcp` (by name or by full path).

**Option A – Cargo bin (easiest)**

```bash
cargo install --path morph-cli
cargo install --path morph-mcp
```

This installs into `~/.cargo/bin`. Ensure `~/.cargo/bin` is on your `PATH` (rustup usually does this). Then you can use the command `morph-mcp` in Cursor’s MCP config (see §2).

**Option B – System install (Linux)**

```bash
sudo cp target/release/morph     /usr/local/bin/
sudo cp target/release/morph-mcp /usr/local/bin/
```

**Option C – System install (macOS)**

On macOS, `/usr/local/bin` is the usual place for locally installed tools. Create it if needed, then copy:

```bash
sudo mkdir -p /usr/local/bin
sudo cp target/release/morph     /usr/local/bin/
sudo cp target/release/morph-mcp /usr/local/bin/
```

Alternatively, install only for your user (no sudo):

```bash
mkdir -p ~/bin   # or ~/.local/bin
cp target/release/morph target/release/morph-mcp ~/bin/
```

Then add `~/bin` (or `~/.local/bin`) to your `PATH` in `~/.zshrc` or `~/.bashrc`.

**Verify:** From a new terminal, run `morph --help` (CLI usage). For the MCP server, run `morph-mcp --version` or `morph-mcp --help`; it prints version/usage and exits. If you run `morph-mcp` with no args, it will look like it’s “hanging”—that’s normal. It’s an MCP server and is waiting for Cursor to talk to it over stdio; you only run it yourself to check that the binary works.

---

## 2. Configure the MCP server in Cursor

1. Open **Cursor Settings** (e.g. **Cursor → Settings → Cursor Settings**, or `Cmd+,` and search for “MCP”).
2. Open the **MCP** (Model Context Protocol) config. Cursor often shows a link to the config file (e.g. `~/.cursor/mcp.json`).
3. Add the Morph server.

**If `morph-mcp` is on your PATH** (Option A above or after adding your install dir to PATH):

```json
{
  "mcpServers": {
    "morph": {
      "command": "morph-mcp",
      "args": []
    }
  }
}
```

**If you use a full path** (e.g. system install or a specific copy):

```json
{
  "mcpServers": {
    "morph": {
      "command": "/usr/local/bin/morph-mcp",
      "args": []
    }
  }
}
```

On macOS with a user install in `~/bin`, use `"/Users/yourusername/bin/morph-mcp"` (replace `yourusername`). Use the real path; Cursor must be able to run this executable.

4. **Restart Cursor** (or reload MCP) so it starts the `morph-mcp` process.

**Optional: Default workspace (so the agent doesn't need to pass `workspace_path`)**  
Cursor often starts the MCP server with a working directory that is *not* your project root, so tools can return "not a morph repository" unless the agent passes `workspace_path`. You can set the repo root once so all tools use it by default:

- **Env var (recommended):** In your MCP config, set `MORPH_WORKSPACE` to the **absolute path** of the project that contains `.morph/`. Use **project-level** `.cursor/mcp.json` so each repo can point at itself:

  ```json
  {
    "mcpServers": {
      "morph": {
        "command": "morph-mcp",
        "args": [],
        "env": {
          "MORPH_WORKSPACE": "/Users/you/my-project"
        }
      }
    }
  }
  ```
  Replace `/Users/you/my-project` with your real project root (the folder that has `.morph/`). You can use a different path per machine; the important part is that it is absolute.

- **First argument:** You can pass the workspace path as the first argument instead: `"args": ["/Users/you/my-project"]`. Same idea—use the absolute path to the repo root.

The server uses, in order: the `workspace_path` argument from the tool call (if present), then `MORPH_WORKSPACE`, then `CURSOR_WORKSPACE_FOLDER` / `WORKSPACE_FOLDER` if set by the host, then the process current directory. Setting `MORPH_WORKSPACE` in project-level config makes recording work without the agent having to pass `workspace_path`.

**Troubleshooting (ENOENT):** If Cursor logs `spawn .../morph-mcp ENOENT`, the `command` path does not exist or is wrong. Use the **full path** to the binary (e.g. `/usr/local/bin/morph-mcp` or `$HOME/.cargo/bin/morph-mcp`). Do not use `~/.cursor/bin/morph-mcp` unless you have put the binary there. After fixing the config, restart Cursor or reload MCP.

---

## 3. Initialize a Morph repo in your project

In the project you want to manage with Morph (your Cursor workspace):

```bash
cd /path/to/your/project
morph init
```

This creates **only** `.morph/` (like `git init`)—objects, refs, config. No top-level directories. Morph works as a plain VCS without prompts or evals (just like git); prompts and evals are optional and live under `.morph/prompts/` and `.morph/evals/` if you use them. The project is now a Morph repo; when you use Morph MCP tools in Cursor with this project open, the MCP server will use this repo.

### Verify your setup

After installing (§1), configuring Cursor (§2), and running `morph init` in a project (§3), check that everything works:

1. **MCP server is running**
   - In Cursor, open **Settings → MCP** (or the MCP / Model Context Protocol panel). The **morph** server should be listed and show as connected or running. If it shows an error or "failed to start", fix the `command` path in `~/.cursor/mcp.json` and restart Cursor.

2. **Agent can call Morph tools**
   - With this project (the one where you ran `morph init`) open in Cursor, start a new chat and ask:
     - *"Call the Morph MCP tool morph_record_session with prompt 'test' and response 'test run'."*
   - The agent should invoke the tool. If it says it doesn't have access to Morph tools, the MCP server isn't connected or the project doesn't have a `.morph` directory.

3. **Confirm something was stored**
   - After step 2, from the project root run:  
     `ls -la .morph/objects`  
     You should see one or more object directories (content-addressed storage). A successful `morph_record_session` will have written objects there.

#### Verify with the Morph CLI

From the **project root** (the directory that contains `.morph/`):

| Command | What it shows |
|--------|----------------|
| `morph status` | **All working directory files** (like `git status`) and whether they're in the store. It does **not** list recorded runs or chats. "No files to track" is normal if the working tree is empty—your MCP-recorded sessions live in the object store, not in status. |
| `ls .morph/objects` | **Your recorded sessions.** Each `.json` file is one stored object (Run, Trace, Program, etc.). The files you see here *are* the prompts and chats you recorded via MCP—each Run and Trace is a separate file named by its content hash. |
| `morph log` | Commit history (if you've run `morph commit`). |
| `morph branch` | Branches (if you've created any). |

**Why don't I see my prompts and chats in `morph status`?**  
`morph status` reports **filesystem files** in the working directory (like `git status`)—source code, config, etc. Recorded sessions from the MCP tool (`morph_record_session`) are stored as **objects** in `.morph/objects/` (one `.json` file per object), not as working-tree files. So to verify recording worked, use `ls .morph/objects`—those files are your runs and traces. There is no `morph run list` command yet; the object store is the source of truth.

**Why didn't my last chat get recorded? (number of files in `.morph/objects` didn't grow)**  
Recording only happens when something **calls** `morph_record_session`. The agent does not call it automatically unless (1) you have added a Cursor rule that tells the agent to call it when it finishes a task (§4.3, Rule A), or (2) you explicitly asked in that chat (e.g. "call morph_record_session with this prompt and your response"). If the agent didn't invoke the tool, nothing is written.

**Nothing in `.morph/prompts/` — is the rule working?**  
A successful `morph_record_session` call stores a prompt Blob into `.morph/prompts/<hash>.json` as well as Run/Trace in `.morph/objects/`, `.morph/runs/`, `.morph/traces/`. If `.morph/prompts/` (and `.morph/runs/`) are empty, the agent is likely not calling the tool. Ensure the rule in `.cursor/rules/morph-record.mdc` is present and that the agent is instructed to invoke the tool as the last step. If the tool returns "not a morph repository", either have the agent pass **workspace_path** with the full path to the project root, or set **MORPH_WORKSPACE** in your MCP config (§2) so the server knows the repo root without the agent passing it.

To record a session after the fact you can:

- **From a new chat:** Ask the agent to call `morph_record_session` with the previous prompt and the previous response (paste them in).
- **From the terminal:** Run `morph run record-session --prompt "your prompt text" --response "the model's response text"` from the project root. This writes a Run and Trace into `.morph/objects/` without going through MCP. Use this when the agent didn't record the last turn and you want to capture it.

If any step fails, use the debugging steps below.

### Debugging: "Morph MCP tool not available" or tools not found

When the agent reports that `morph_record_session` (or other Morph tools) are not found, the MCP server either isn't running for this session or isn't exposing tools. Work through these in order:

1. **Check MCP status in Cursor**
   - Open **Cursor → Settings → MCP** (or **Settings**, then search for "MCP").
   - Find the **morph** server in the list. Note whether it shows as connected/running or an error (e.g. "Failed to start", "Connection closed").
   - If there's a **Reload** or **Restart** control for the morph server, try it and then start a new chat and ask the agent to call `morph_record_session` again.

2. **Check MCP logs**
   - Cursor writes MCP logs somewhere it can show (e.g. **Output** panel with "MCP" or "Extension Host", or **Developer → Open Logs**). Open the relevant log and look for the morph server:
     - `spawn ... ENOENT` → the `command` in `~/.cursor/mcp.json` is wrong or the binary isn't at that path.
     - `Client error` / `Connection closed` → the morph-mcp process is exiting (e.g. crash or missing dependency).
   - Fix the config if the path is wrong (§2): use the **full path** to `morph-mcp` (e.g. `/usr/local/bin/morph-mcp` or `$HOME/.cargo/bin/morph-mcp`). Restart Cursor fully after changing `mcp.json`.

3. **Confirm the binary runs**
   - In a terminal: `morph-mcp --version` (or your full path). It should print a version and exit. If you get "command not found" or a permission error, fix your install (§1) or the path in `mcp.json`.

4. **Same workspace as `morph init`**
   - The agent must be chatting in a **workspace that has a `.morph` directory** (i.e. where you ran `morph init`). If you opened a parent folder or a different repo, Cursor may still start the MCP server, but some tools expect to run in a Morph repo. Open the project root where `.morph/` exists and start a new chat there.

5. **Full Cursor restart**
   - Quit Cursor completely and reopen it, then open your Morph project and try again. MCP server processes are started at Cursor startup; a full restart ensures the morph server is started with the current config.

6. **Confirm config file**
   - Open `~/.cursor/mcp.json` and ensure the `mcpServers.morph` entry exists and has the correct `command` (and `args` if you use any). No trailing commas, valid JSON. If you use a project-level MCP config (e.g. `.cursor/mcp.json` in the repo), check that too and ensure the morph server is defined there if that's what Cursor is using.

After each change (path fix, config fix, restart), try again in a **new chat** in the project that has `.morph/`.

### Debugging: "not a morph repository"

If the agent invokes a Morph tool and gets **"not a morph repository"** (or the tool returns an error to that effect), the server couldn't find a Morph repo:

1. **Run `morph init` in the project**
   - From the project root (the folder you have open in Cursor), run: `morph init`. This creates `.morph/`. Without it, no tool can record or commit.

2. **Pass `workspace_path` if the server cwd is wrong**
   - The MCP server looks for `.morph/` in its **current working directory**. Cursor often starts the server with the project root as cwd, but sometimes it isn't. If the tool still fails after `morph init`, have the agent pass **workspace_path** set to the **full path** of your project root (the directory that contains `.morph/`). Example: `workspace_path: "/Users/you/my-project"` on macOS. The agent can use the workspace path Cursor provides.

---


## Your workflow (and what’s supported)

- **Record Cursor sessions into Morph**  
  Each “session” (your prompt + the model’s replies and actions) can be stored as a **Run** with an optional **Trace**. Morph only ingests Run/Trace data; it does not run the model.

- **Explicitly commit the filesystem**  
  When you’re happy with the working tree, **stage** and **commit** in Morph (e.g. via MCP tools `morph_stage` and `morph_commit`). That creates a Morph commit (snapshots the file tree + optional program/eval contract), separate from Git.

“Every prompt recorded” is not automatic at the Cursor app level. You can get close by **(1)** having the **agent** call Morph’s MCP tools when it finishes a task (§4.3), or **(2)** using **Cursor hooks** to record on submit and on task stop (§6). To capture the **full model response text**, the agent must call **morph_record_session** (§4.3); hooks do not receive the reply.

---

## 4. Recording Cursor sessions (Runs + Traces)

Morph does not see your Cursor UI directly. It only ingests **Run** and **Trace** objects that are passed to it. So “record every prompt and output” works by having the **agent** (or a script) produce that data and call the Morph MCP tool.

### How Morph MCP tools are used in Cursor

Once the Morph MCP server is configured (§2) and Cursor has restarted, **all Morph tools (including `morph_record_run`) are available to the agent automatically**. You don’t install or enable individual tools—Cursor discovers them from the running MCP server.

- **In chat/composer:** The agent can call any Morph tool when it’s relevant. You can say e.g. “record that run into Morph using the JSON file I created” and the agent will call `morph_record_run` with the path you mean (or one it created). Or you add a **Cursor rule** (§4.3) so the agent calls a recording tool when it finishes a task.
- **Easiest for session recording:** Use **morph_record_session** (§4.3) so the agent passes `prompt` and `response` strings in one call. Use **morph_record_run** when you already have Run (and optionally Trace) as JSON files and want the agent to ingest them.

### 4.1 MCP tool: `morph_record_run`

The tool **ingests** a Run that already exists as a JSON file, plus optional Trace and artifact files:

- **run_file** (required): path to a JSON file containing a single Run object (see schema below).
- **trace_file** (optional): path to a JSON file containing a Trace object. If provided, Morph stores it and verifies that the Run’s `trace` field equals the Trace’s hash.
- **artifact_files** (optional): list of paths to Artifact JSON files to store and link.

All paths can be relative to the Morph repo root (e.g. your project root after `morph init`).

So the flow is:

1. Something (the agent or a script) builds a **Run** object (and optionally a **Trace** and Artifacts).
2. Writes them to files (e.g. under `.morph/runs/` or a temp dir).
3. Calls the MCP tool **morph_record_run** with `run_file` and optionally `trace_file` and `artifact_files`.

**How to use it in Cursor:** Ensure the Morph MCP server is set up (§2). Then either (1) ask in chat, e.g. “Call morph_record_run with run_file `.morph/runs/my-run.json`” and the agent will invoke the tool, or (2) add a rule that tells the agent to write Run/Trace JSON and then call `morph_record_run` when it finishes. For most users, **morph_record_session** (§4.3) is simpler—the agent just passes prompt and response text and doesn’t need to build JSON or compute hashes.

### 4.2 Run and Trace schema (minimal for Cursor sessions)

**Run** (required fields; see v0-spec §4.6 and `morph-core` for full shape):

```json
{
  "type": "run",
  "program": "<program_hash_or_identity>",
  "commit": null,
  "environment": {
    "model": "cursor-ai",
    "version": "1.0",
    "parameters": {},
    "toolchain": {}
  },
  "input_state_hash": "<tree_or_zero>",
  "output_artifacts": [],
  "metrics": {},
  "trace": "<trace_hash>",
  "agent": {
    "id": "cursor",
    "version": "1.0",
    "policy": null
  }
}
```

- Use Morph’s **identity program hash** for ad‑hoc Cursor sessions if you’re not tying to a specific Program object. Create a minimal JSON file (e.g. `prog.json` in the repo root) with the identity program structure (single node, `kind: "identity"`, no edges), then run `morph program create prog.json`—it stores the program in the object store and prints its hash. Alternatively use a small script that calls `morph_core::identity_program()` and `morph_core::content_hash()` (or store the object and read the hash from the store).
- **trace** must be the **hash** of the Trace object (canonical JSON, then SHA-256). So you must either compute that hash (e.g. with morph-core hashing) or write the Trace to a file and use a small helper to get the hash before building the Run JSON.
- **input_state_hash** can be a tree hash or a placeholder (e.g. all zeros) for simple session recording.

**Trace** (v0-spec §4.8):

```json
{
  "type": "trace",
  "events": [
    {
      "id": "evt_1",
      "seq": 0,
      "ts": "2025-02-21T12:00:00Z",
      "kind": "prompt",
      "payload": { "text": "Your prompt here..." }
    },
    {
      "id": "evt_2",
      "seq": 1,
      "ts": "2025-02-21T12:01:00Z",
      "kind": "response",
      "payload": { "text": "Model reply..." }
    }
  ]
}
```

`kind` can be e.g. `prompt`, `response`, `tool_call`, `file_edit`, `file_read`, `error`. The agent can summarize the conversation into a few events.

### 4.3 Getting the agent to record sessions (including response text)

To have the agent record a session when it finishes a task, add a Cursor rule that tells it to call a Morph MCP tool. Use **morph_record_session** (Rule A, below) or **morph_record_run** (Rule B, file-based).

**Where to add the rule:** Create a file under `.cursor/rules/` **in the top level of your repo** (e.g. `morph/.cursor/rules/morph-record.mdc`), not in your home directory. Or use **Cursor → Settings → Rules** (or **Agents → Rules**), which may create that path in the project. Use `globs: ["**/*"]` so the rule applies in every conversation. The rule should require that the project has a `.morph` directory before recording.

**Rule A – morph_record_session (recommended):** The agent sends prompt and response in one call; no JSON or hashes. Paste this into `.cursor/rules/morph-record.mdc`:

```markdown
---
description: Record Morph sessions with full prompt and response when a task is done
globs: ["**/*"]
alwaysApply: true
---

When you complete a substantive task in this project and the project has a `.morph` directory:

1. Call the MCP tool **morph_record_session** as the **last action** before you end your reply. Do not conclude your turn without calling it.
2. **Parameters:** **prompt** = the user's exact request; **response** = your **entire** reply (first to last character—do not truncate or summarize). If the tool returns "not a morph repository", set **workspace_path** to the full path of the project root, or set MORPH_WORKSPACE in `.cursor/mcp.json` (§2).

Do not skip this step when the user asked for code or changes and you have finished the work.
```

That gives you a Run and a Trace (one prompt event, one response event) in Morph with the full response text.

**Making session recording reliable:** Recording depends on the agent actually calling the tool. To make it more stable:

1. **Set `alwaysApply: true`** in the rule's frontmatter so Cursor is supposed to include the rule in every chat. Rules with only `globs` or "Apply Intelligently" are agent-decided and may be skipped.
2. **Set a default workspace** so the tool doesn't fail with "not a morph repository". In the **project's** `.cursor/mcp.json`, set `env.MORPH_WORKSPACE` to the **absolute path** of the project root (the folder that contains `.morph/`). Then the MCP server always knows the repo even when its process cwd is wrong. See §2 (Optional: Default workspace).
3. **Keep the rule in `.cursor/rules/`** with `globs: ["**/*"]` so it applies in every conversation. If the agent still doesn't call the tool, remind it in chat (e.g. "When you're done, record this session with morph_record_session").
4. **If a turn wasn't recorded**, you can record after the fact: from a new chat, ask the agent to call `morph_record_session` with the previous prompt and response (paste them), or from the project root run:  
   `morph run record-session --prompt "..." --response "..."`

**Debugging rule application (do rules fire?):** Cursor does not guarantee that rules are applied every time, and there is no built-in way to see the raw context or which rules were injected.

- **Rule types:** Only **Always Apply** (`alwaysApply: true`) is intended to run on every chat; even that can sometimes be skipped on the first request. Rules that use only globs or "Apply Intelligently" are chosen by the agent and are non-deterministic.
- **Check rule status:** **Cursor → Settings → Rules, Commands** shows project rules and whether they're enabled. The "Active Rules" tooltip in chat is buggy: it only lists always-apply rules, not glob- or description-based ones.
- **Verify frontmatter:** The `.mdc` file must have valid YAML frontmatter between `---` markers. A missing closing `---` or invalid YAML can cause the rule to be ignored with no error.
- **Probe in chat:** To see if the rule was loaded, ask the agent: "Which project rules do you have in context? List their names or key instructions." If the morph rule is listed, it was injected; if not, it may have been skipped.
- **Fallback:** Manually mention the rule in chat (e.g. `@morph-record` or "follow the morph-record rule") to force it into context when recording is critical.


#### Rule B: Call morph_record_run when you finish (file-based)

Use this when you want the agent to write Run and Trace JSON files and then call **morph_record_run**. Create `.cursor/rules/morph-record-run.mdc` with:

```markdown
---
description: Record Morph sessions via Run/Trace files and morph_record_run when a task is done
globs: ["**/*"]
---

When you complete a substantive task in this project and the project has a `.morph` directory:

1. Create `.morph/runs/` if it does not exist.

2. Write a **Trace** JSON file to `.morph/runs/<task-id>.trace.json` (e.g. use a short timestamp or slug as task-id). The Trace must have `"type": "trace"` and an `events` array with at least:
   - one event with `"kind": "prompt"` and `payload.text` set to the user's request;
   - one event with `"kind": "response"` and `payload.text` set to your full reply.
   Each event needs `id`, `seq`, `ts` (ISO 8601), `kind`, and `payload`. See §4.2 in CURSOR-SETUP.md for the schema.

3. Compute the **content hash** of the Trace (SHA-256 of the Trace JSON in canonical form). Run:
   `python3 -c "import json, hashlib, sys; t=json.load(sys.stdin); print(hashlib.sha256(json.dumps(t, sort_keys=True).encode()).hexdigest())" < .morph/runs/<task-id>.trace.json`
   Use the output as the `trace` value in the Run.

4. Write a **Run** JSON file to `.morph/runs/<task-id>.run.json`. Include `"type": "run"`, `"trace": "<hash from step 3>"`, `"program": "<identity program hash>"`, plus the other required fields (environment, input_state_hash, output_artifacts, metrics, agent). For ad-hoc Cursor sessions use the repo's identity program hash (e.g. from `morph program create prog.json` as in §4.2). See §4.2 for the full Run schema.

5. Call the MCP tool **morph_record_run** with:
   - **run_file**: path to the Run JSON (e.g. `.morph/runs/<task-id>.run.json`);
   - **trace_file** (optional): path to the Trace JSON. Providing it lets Morph verify and store the Trace.

Do steps 1–5 as the last step before you conclude your reply. If you do not know the identity program hash, use the rule that calls **morph_record_session** (Rule A) instead.
```

**When to use which:** Prefer **Rule A** (morph_record_session) for "record when you finish"; use **Rule B** only if you need file-based Run/Trace or tooling that reads `.morph/runs/`.


---

## 5. Explicitly committing the filesystem

When you want to snapshot the working tree and record it as a Morph commit:

1. **Stage** what should go into the next commit (like `git add`—stages any file from the working directory):
   - MCP: **morph_stage** with `paths` (default `["."]`) and optional `workspace_path`.
   - CLI: `morph add [paths...]`
2. **Commit** (snapshots the file tree + optional program/eval contract):
   - MCP: **morph_commit** with `message`, `program`, `eval_suite`, and optional `metrics`, `author`, `workspace_path`.
   - CLI: `morph commit -m "..." --program <hash> --eval-suite <hash> [--metrics-file ...]`

For ad‑hoc Cursor work you can use the **identity program** and a minimal **eval suite** (or the defaults your repo uses). The MCP tools take a `workspace_path` so you can point at the Morph repo root if needed.

---

## 6. Automatic recording with Cursor hooks

Cursor **hooks** run scripts when certain events happen. The **Cursor app** invokes them at those events—the agent (LLM) never sees or decides whether a hook runs, so **the agent cannot ignore a hook**. They receive JSON on stdin (e.g. `conversation_id`, `generation_id`, `prompt`, `workspace_roots`). You can use them to record sessions into Morph without relying on the agent.

**Relevant events:**

| Hook | When it runs | Payload (typical) |
|------|----------------|-------------------|
| `beforeSubmitPrompt` | User submits a prompt (before the model sees it) | `conversation_id`, `generation_id`, `prompt`, `attachments`, `workspace_roots` |
| `stop` | Task is completed | `conversation_id`, `generation_id`, `workspace_roots`, etc. |

**Config location:** Project-level `.cursor/hooks.json` or user-level `~/.cursor/hooks.json`. Format:

```json
{
  "version": 1,
  "hooks": {
    "beforeSubmitPrompt": [{"command": "/path/to/morph-record-prompt.sh"}],
    "stop": [{"command": "/path/to/morph-record-stop.sh"}]
  }
}
```

**Design for “record every session”:**

1. **beforeSubmitPrompt**  
   Your script reads the JSON from stdin. If the workspace is a Morph repo (e.g. `workspace_roots` contains a path with `.morph/`), append a record to a pending file keyed by `conversation_id`, e.g. `.morph/hooks/pending-<conversation_id>.jsonl` with one line per prompt: `{"ts": "...", "prompt": "...", "generation_id": "..."}`.

2. **stop**  
   Your script reads stdin, gets `conversation_id` and `workspace_roots`. If there is a pending file for that conversation:
   - Build a **Trace** object: `events` = one “prompt” event per line in the pending file (and optionally a single “response” or “task_complete” event; the hook payload does **not** include the model’s reply text, so that event can be a placeholder).
   - Compute the Trace’s content hash (SHA-256 of Morph’s canonical JSON; same as `morph-core`). You can do this with a small script (e.g. Python: `json.dumps(obj, sort_keys=True)` + hashlib) or a future `morph hash-object` CLI.
   - Build a **Run** object with that `trace` hash, identity program hash, and required fields (see §4.2).
   - Write Run and Trace to `.morph/runs/session-<conversation_id>-<timestamp>.run.json` and `.trace.json`.
   - Run: `morph run record --run-file <run.json> [--trace <trace.json>]` (from the repo root).
   - Remove or clear the pending file for that conversation.

**Limitation:** Hook payloads do not include the model’s response text. So your Trace will have accurate “prompt” events and a “task completed” (or placeholder “response”) event, but not the actual assistant text. **To get the full model response text**, use agent-driven recording: a rule that tells the agent to call **morph_record_session** with the user's prompt and the agent's full reply (§4.3).

**Debug and log files (morph repo):** The hook scripts in the morph repo write logs under `.morph/hooks/` so you can separate "Cursor called the script" from "Morph accepted the run." Check **`.morph/hooks/logs/cursor-invoke.log`** for one line per hook run (timestamp, hook name, conversation_id)—if lines appear there, Cursor is invoking the hooks. Check **`.morph/hooks/logs/morph-record.log`** for one line per successful record (conversation_id, run_hash)—if lines appear there, the script called `morph run record` and Morph stored the run. Inspect **`.morph/hooks/debug/last-beforeSubmitPrompt.json`** and **`last-stop.json`** to see the last payload Cursor sent. See `cursor/README.md` in the morph repo for the full table.

**Where hook scripts live (morph repo):** All Cursor support is in the tree: **`cursor/`** holds the canonical hook scripts and a README; **`.cursor/hooks.json`** points at `../cursor/morph-record-prompt.sh` and `../cursor/morph-record-stop.sh`. Use the same layout in your own repo if you want one directory that contains everything needed for Cursor (hooks + optional rules/MCP).

**Making it easy:** A Cursor **plugin** could bundle (1) these hook scripts, (2) the Morph MCP server config, and (3) a small helper to compute Trace hashes, so that “install Morph plugin” gives you automatic session recording and MCP in one step. See the next section.

---

## 7. Can we build a plugin or VS Code extension that hooks into the agent lifecycle?

**Short answer:** There is no public Cursor (or VS Code) API to "intercept" or read the contents of every agent/composer message from a separate process or UI overlay. Cursor does not expose a way for a plugin or extension to subscribe to raw chat messages as they are sent or received.

**VS Code extension?** Cursor runs VS Code extensions (same core). The standard [VS Code Chat Participant API](https://code.visualstudio.com/api/extension-guides/ai/chat) only lets you *add* your own participant (e.g. `@morph`): you see prompts and responses only when users talk to *your* participant. There is no documented API to subscribe to the *default* Cursor agent's traffic (e.g. `onDidSendPrompt` or "every agent turn"). So a VS Code extension cannot reliably capture all prompts and responses unless Cursor adds a custom extension API (none is documented as of now).

**What you can do:**

- **Hooks** (see §6) are the supported way to react to agent lifecycle. The app runs them; the agent cannot ignore them. They fire when you submit a prompt (`beforeSubmitPrompt`), when a task stops (`stop`), and at other points (e.g. `afterFileEdit`, `beforeReadFile`). They receive structured JSON (conversation id, prompt text, file paths, etc.) on stdin. So “automatic recording” is implemented by **hook scripts** that build Run/Trace and call `morph run record` (or the MCP tool), not by intercepting the chat UI.

- **Cursor plugins** (marketplace) can bundle **MCP servers**, **rules**, **commands**, and **hooks**. So you can build a “Morph” plugin that:
  - Registers the Morph MCP server (or points to it),
  - Installs hook scripts that record sessions on `beforeSubmitPrompt` + `stop`,
  - Optionally includes a rule that asks the agent to record richer sessions when it finishes a task.

That gives you “as automatic as Cursor allows” without any unsupported UI interception.

---

## 8. Quick reference: MCP tools

| Tool | Purpose |
|------|--------|
| **morph_init** | Create a Morph repo (path optional; default current dir). |
| **morph_record_run** | Ingest a Run from JSON file; optional trace and artifact paths. |
| **morph_record_session** | Record a single prompt/response as a Run + Trace in one call. **Use this to capture the full model response text;** the agent passes `prompt` and `response` strings. |
| **morph_record_eval** | Ingest metrics from a JSON file with a `metrics` key. |
| **morph_stage** | Stage working directory files into the object store and update the staging index (like git add; default paths: ["."]). |
| **morph_commit** | Create a commit (snapshots staged file tree; program and eval_suite are optional, default to identity/empty). |
| **morph_annotate** | Attach an annotation to an object (target_hash, kind, data, etc.). |
| **morph_branch** | Create a new branch at current HEAD. |
| **morph_checkout** | Switch HEAD to a branch or commit and restore the working tree from the commit's file tree. |

All tools that need a repo accept an optional **workspace_path**. If omitted, they use the current working directory of the MCP server process (Cursor typically runs it with the project root as cwd). **workspace_path** must be the **full path** to the directory that contains `.morph/` (your project root). If you get "not a morph repository", run `morph init` and/or pass **workspace_path** explicitly (see debugging in §3).

---

## Summary

- **Goal:** In a Morph-managed project, have Cursor prompts and output recorded into Morph, and explicitly commit the filesystem in Morph when you want.
- **Recording sessions:** Use the MCP tool **morph_record_run** with Run (and optional Trace) JSON files. Use a Cursor rule so the agent produces those files and calls the tool after completing a task.
- **Committing filesystem:** Use **morph_stage** then **morph_commit** (via MCP or CLI) when you want to create a Morph commit.
- **Limitation:** Cursor does not auto-push every prompt to MCP; “every prompt recorded” is achieved by the agent recording each task/session as a Run when the rule is applied.
