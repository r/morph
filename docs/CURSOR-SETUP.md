# Setting up Morph with Cursor

This guide explains how to use Morph in a Cursor-managed project so that:

1. **Session evidence** (prompts and Cursor output) can be recorded into Morph as Runs.
2. **Filesystem state** is committed explicitly when you choose, via Morph.

---

## Your workflow (and what’s supported)

- **Record Cursor sessions into Morph**  
  Each “session” (your prompt + the model’s replies and actions) can be stored as a **Run** with an optional **Trace** (event log: prompt, response, tool calls, edits). Morph does not run code or the model; it only **ingests** Run/Trace data that something else produces.

- **Explicitly commit the filesystem**  
  When you’re happy with the working tree, you **stage** and **commit** in Morph (e.g. via MCP tools `morph_stage` and `morph_commit`). That creates a Morph commit (program + eval contract + metrics), separate from Git.

So: **yes** — you can have Cursor activity recorded as Runs, and you explicitly commit the filesystem in Morph when you want. The one caveat: “every prompt recorded” is not automatic at the Cursor app level. Cursor doesn’t push every turn to an MCP server by itself. You can get close in two ways: **(1)** have the **agent** call Morph’s MCP tools when it finishes a task (rule-based, §4.3), or **(2)** use **Cursor hooks** to record automatically when you submit a prompt and when a task stops (§6). To capture the **full model response text**, the agent must call **morph_record_session** with prompt and response (§4.3); hooks do not receive the reply. There is no Cursor API to “intercept” the agent UI directly; hooks are the supported way to react to agent lifecycle (§7).

---

## 1. Build the Morph MCP server

From the Morph repo (or after `cargo install` from this repo):

```bash
cd /path/to/morph
cargo build -p morph-mcp -r
```

The binary is `target/release/morph-mcp` (or `target/debug/morph-mcp` if you didn’t use `-r`). You can also install it so it’s on your `PATH`:

```bash
cargo install --path morph-mcp
```

Then ensure `~/.cargo/bin` (or your install prefix) is on your `PATH` so Cursor can run `morph-mcp`.

---

## 2. Add the MCP server in Cursor

1. Open **Cursor Settings** (e.g. **Cursor → Settings → Cursor Settings** or `Cmd+,` then search for “MCP”).
2. Open **MCP** (or “Model Context Protocol”) configuration.  
   Cursor may show a link to edit the config file (e.g. `~/.cursor/mcp.json` or a project-level config).
3. Add the Morph server so it runs in **project** mode (one server per project that has a Morph repo):

**Option A – Global config** (`~/.cursor/mcp.json` or the path Cursor shows):

```json
{
  "mcpServers": {
    "morph": {
      "command": "/path/to/morph/target/release/morph-mcp",
      "args": []
    }
  }
}
```

Replace `/path/to/morph/target/release/morph-mcp` with the actual path to `morph-mcp`, or use `morph-mcp` if it’s on your `PATH`.

**Option B – Project-local**  
If Cursor supports a project-level MCP config (e.g. `.cursor/mcp.json` in the repo), you can put the same `mcpServers.morph` entry there so the server only runs when that project is open.

4. Restart Cursor or reload MCP so it starts the `morph-mcp` process. The server uses **stdio**; Cursor talks to it over stdin/stdout.

---

## 3. Initialize a Morph repo in your project

In the project you want to manage with Morph:

```bash
cd /path/to/your/project
morph init
```

If you don’t have the CLI, from the Morph repo:

```bash
cargo run -p morph-cli -- init
```

This creates `.morph/` (objects, refs, config). The project is now a Morph repo; the MCP server will use it when you run tools with that project as the workspace.

---

## 4. Recording Cursor sessions (Runs + Traces)

Morph does not see your Cursor UI directly. It only ingests **Run** and **Trace** objects that are passed to it. So “record every prompt and output” works by having the **agent** (or a script) produce that data and call the Morph MCP tool.

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

- Use Morph’s **identity program hash** for ad‑hoc Cursor sessions if you’re not tying to a specific Program object. You can get it once by creating a minimal `programs/identity.json` that matches the identity program (single node, `kind: "identity"`, no edges), then run `morph program create programs/identity.json` and `morph program show programs/identity.json` to print its hash. Alternatively use a small script that calls `morph_core::identity_program()` and `morph_core::content_hash()` (or store the object and read the hash from the store).
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

To have “every Cursor session” (or each task) recorded:

**Cursor rule** (e.g. `.cursor/rules/morph-record.mdc` or **Agents → Rules**):

```markdown
---
description: Record Morph sessions with full prompt and response
globs: ["**/*"]
---

When you complete a substantive task in this project and the project has a `.morph` directory, call the MCP tool **morph_record_session** with:
- **prompt**: the user's exact request (the message that started this turn or task).
- **response**: your full reply text (what you wrote back to the user). Do not truncate; include the complete response so it is stored in Morph.

Optional: model_name, agent_id, workspace_path if needed.
```

That gives you a Run and a Trace (one prompt event, one response event) in Morph with the full response text.

**Alternative (file-based):** To build Run/Trace yourself, use **morph_record_run** with run_file and trace_file (§4.1). Include a response event in the Trace with payload.text set to the model's reply. The agent would need to write the JSON files and compute the Trace hash (or use a helper script).


---

## 5. Explicitly committing the filesystem

When you want to snapshot the working tree and record it as a Morph commit:

1. **Stage** what should go into the next commit:
   - MCP: **morph_stage** with `paths` (default `["."]`) and optional `workspace_path`.
   - CLI: `morph add [paths...]`
2. **Commit** (program + eval contract + metrics):
   - MCP: **morph_commit** with `message`, `program`, `eval_suite`, and optional `metrics`, `author`, `workspace_path`.
   - CLI: `morph commit -m "..." --program <hash> --eval-suite <hash> [--metrics-file ...]`

For ad‑hoc Cursor work you can use the **identity program** and a minimal **eval suite** (or the defaults your repo uses). The MCP tools take a `workspace_path` so you can point at the Morph repo root if needed.

---

## 6. Automatic recording with Cursor hooks

Cursor **hooks** run scripts when certain events happen. They receive JSON on stdin (e.g. `conversation_id`, `generation_id`, `prompt`, `workspace_roots`). You can use them to record sessions into Morph without relying on the agent.

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

**Making it easy:** A Cursor **plugin** could bundle (1) these hook scripts, (2) the Morph MCP server config, and (3) a small helper to compute Trace hashes, so that “install Morph plugin” gives you automatic session recording and MCP in one step. See the next section.

---

## 7. Can we build a plugin that intercepts agent windows?

**Short answer:** There is no public Cursor (or VS Code) API to “intercept” or read the contents of every agent/composer message from a separate process or UI overlay. Cursor does not expose a way for a plugin to subscribe to raw chat messages as they are sent or received.

**What you can do:**

- **Hooks** (see §6) are the supported way to react to agent lifecycle. They fire when you submit a prompt (`beforeSubmitPrompt`), when a task stops (`stop`), and at other points (e.g. `afterFileEdit`, `beforeReadFile`). They receive structured JSON (conversation id, prompt text, file paths, etc.) on stdin. So “automatic recording” is implemented by **hook scripts** that build Run/Trace and call `morph run record` (or the MCP tool), not by intercepting the chat UI.

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
| **morph_stage** | Stage paths into the object store (default `"."`). |
| **morph_commit** | Create a commit (message, program, eval_suite, optional metrics/author). |
| **morph_annotate** | Attach an annotation to an object (target_hash, kind, data, etc.). |
| **morph_branch** | Create a new branch at current HEAD. |
| **morph_checkout** | Set HEAD to a branch name or commit hash. |

All tools that need a repo accept an optional **workspace_path**; if omitted, they use the current working directory of the MCP server process (Cursor typically runs it with the project root as cwd).

---

## 7. Summary

- **Goal:** In a Morph-managed project, have Cursor prompts and output recorded into Morph, and explicitly commit the filesystem in Morph when you want.
- **Recording sessions:** Use the MCP tool **morph_record_run** with Run (and optional Trace) JSON files. Use a Cursor rule so the agent produces those files and calls the tool after completing a task.
- **Committing filesystem:** Use **morph_stage** then **morph_commit** (via MCP or CLI) when you want to create a Morph commit.
- **Limitation:** Cursor does not auto-push every prompt to MCP; “every prompt recorded” is achieved by the agent recording each task/session as a Run when the rule is applied.
