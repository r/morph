# Morph ↔ Tap Gap Analysis

Evidence-driven audit of what Morph records, what tap extracts, and what remains missing for high-quality coding-agent evaluation.

**Date:** 2026-04-17
**Traces inspected:** 188 trace files, 278 runs, 471 prompts
**Method:** Source code audit + real trace inspection

---

## 1. Summary

Morph's recording layer captures user prompts and assistant responses reliably. However, **real traces in this repository contain only `prompt`/`response` (or `user`/`assistant`) events with `payload.text`** — no tool calls, file reads, file edits, or structured events appear in any of the 188 traces inspected. The schema supports these event kinds (open string `kind` field), and tap correctly handles them in code, but the recording hooks (Cursor hooks, MCP `morph_record_session`) do not emit them.

Tap had several gaps relative to what Morph *does* record. This audit identified and fixed:

- Context builder now includes file **contents** (not just paths)
- Context builder now includes tool **outputs** from prior steps
- Tool result pairing improved (reverse-scan for unpaired calls)
- Diagnostics now report prompt/response lengths, model, and agent
- Placeholder response detection expanded
- Summary shallow-trace detection now uses normalizer functions
- `print_trace_events` now recognizes `user`/`assistant`/tool/file kind aliases
- New `trace-stats` command for debugging individual traces
- New `preview` command showing labeled eval sections
- New `--model`/`--agent`/`--min-steps` filters on export
- `filter_runs` and `task_to_eval_cases` exposed as public API

---

## 2. What Morph Records

### 2.1 Event kinds observed in real traces

| Kind | Count (approx.) | Source |
|------|-----------------|--------|
| `prompt` | 203 | Hook-based recording (Cursor prompt/stop hooks) |
| `response` | 185 | Hook-based recording |
| `user` | 3 | `record_session`/`record_conversation` (MCP-based) |
| `assistant` | 3 | `record_session`/`record_conversation` (MCP-based) |

No other kinds observed. No `tool_call`, `file_read`, `file_edit`, `tool_result`, or any structured events.

### 2.2 Event payload structure

Every event has the same payload shape:

```json
{"text": "<string>"}
```

No structured fields (`name`, `input`, `output`, `error`, `path`, `content`) were observed in any trace payload.

### 2.3 Trace object structure

```json
{
  "type": "trace",
  "events": [
    {
      "id": "evt_0",
      "seq": 0,
      "ts": "2026-04-17T12:00:00+00:00",
      "kind": "user",
      "payload": {"text": "..."}
    }
  ]
}
```

Fields are consistent across all 188 traces. `id` format varies (`evt_0`, `evt_prompt`, `evt_stop`). Timestamps are RFC3339. Ordering is by `seq`.

### 2.4 Run object structure

```json
{
  "type": "run",
  "pipeline": "<hash>",
  "trace": "<hash>",
  "environment": {"model": "cursor", "version": "1.0", "parameters": {}, "toolchain": {}},
  "agent": {"id": "cursor", "version": "1.0"},
  "metrics": {},
  "commit": null,
  "input_state_hash": "0000...0000",
  "output_artifacts": []
}
```

Key findings:
- `environment.model` is often `"cursor"` (IDE name, not LLM model)
- `metrics` is always `{}` in hook-recorded runs
- `input_state_hash` is always all-zeros
- `output_artifacts` is always empty
- `commit` is always null

### 2.5 Prompt blob structure

```json
{
  "type": "blob",
  "kind": "prompt",
  "content": {
    "text": "<first user message>",
    "response": "<last assistant message>",
    "timestamp": "...",
    "message_count": 2
  }
}
```

---

## 3. What Tap Uses

| Feature | Status | Notes |
|---------|--------|-------|
| Prompt text | ✅ Used | `payload.text` from `user`/`prompt` events |
| Response text | ✅ Used | `payload.text` from `assistant`/`response` events |
| Kind normalization | ✅ Used | `user`→`prompt`, `assistant`→`response` |
| Step grouping | ✅ Used | New step on each prompt event |
| Multiple responses per step | ✅ Used | Concatenated with `\n\n` |
| Tool call extraction | ✅ Used | `name`, `input` from payload — but **never triggered** by real traces |
| Tool result pairing | ✅ Used | Pairs with preceding call — **never triggered** |
| File read extraction | ✅ Used | `path`, `content` from payload — **never triggered** |
| File edit extraction | ✅ Used | `path`, `content` from payload — **never triggered** |
| Run metadata | ✅ Used | model, agent, agent_version from Run object |
| Timestamps | ✅ Used | First event's `ts` as task timestamp |
| Event ordering | ✅ Used | Respects `seq` order from trace |
| Metrics | ✅ Checked | Diagnostic flags empty metrics |
| Placeholder detection | ✅ Used | Detects `"(task completed; response not captured by hook)"` |

---

## 4. What Tap Should Use (improvements implemented)

### 4.1 Richer context building (DONE)

Previously, context for `WithContext` and `Agentic` export modes included only file **paths** from prior steps. Now includes:

- File contents (truncated to 1000 chars)
- Tool output from prior steps (truncated to 500 chars)

### 4.2 Better tool result pairing (DONE)

Previously, only the last tool call in a step received a tool result. Now:
- Reverse-scans for the first unpaired tool call
- Supports `call_id`/`tool_call_id` payload keys for explicit matching

### 4.3 Prompt quality metrics (DONE)

`TapDiagnostic` now includes:
- `prompt_lengths`: character counts for each prompt event
- `response_lengths`: character counts for each response event
- `model`: actual model string from run
- `agent`: actual agent id from run
- Issue flagging for very short prompts (<10 chars)

### 4.4 Expanded placeholder detection (DONE)

Now detects:
- `"(task completed; response not captured by hook)"`
- `"(task completed"` prefix
- `"(no response)"`

### 4.5 Consistent kind normalization (DONE)

`summarize_repo` shallow-trace check now uses `is_tool_call`/`is_file_read`/`is_file_edit` functions instead of literal string matching, ensuring `tool_use`, `function_call`, `read_file`, etc. are all recognized.

### 4.6 Trace printer (DONE)

`print_trace_events` in CLI now recognizes all kind aliases:
- `user` → labeled as "prompt"
- `assistant` → labeled as "response"
- Tool/file kinds → show structured payload fields

---

## 5. True Morph Gaps (recording-related)

These are genuine missing capabilities in Morph's recording layer — not evaluation logic.

### 5.1 No structured events from hooks (CRITICAL)

**What's missing:** Cursor hooks and the MCP `morph_record_session` tool only record `prompt` and `response` (or `user`/`assistant`) events. No tool calls, file reads, file edits, command executions, or other structured events are captured.

**Why it matters:** Without structured events, tap can only produce "shallow" prompt-response eval cases. Agentic replay is impossible because there's no record of what tools the agent called, what files it read, or what edits it made.

**What Morph schema supports:** The `TraceEvent` schema already supports arbitrary kinds and structured payloads. The `record_conversation` API already accepts arbitrary `role` strings that become event `kind`. The missing piece is callers providing richer messages.

**Recommended fix (Morph-side, general-purpose):**
- `record_conversation` should support richer `ConversationMessage` payloads (not just `content` → `text`). A `metadata` field on `ConversationMessage` that gets merged into `payload` would allow callers to pass structured data.
- This is a general recording improvement, not evaluation-specific.

### 5.2 Model name not recorded accurately (MODERATE)

**What's missing:** Hook-recorded runs have `model: "cursor"` — the IDE name, not the actual LLM model (e.g., `claude-opus-4`, `gpt-4o`).

**Why it matters:** Model-level evaluation comparisons are impossible without knowing which model produced each response. Filtering by model gives meaningless results.

**Current workaround:** The MCP `morph_record_session` accepts `model_name` and records it correctly when agents call it. The issue is hooks that can't determine the model.

### 5.3 Timestamps all identical within a trace (MINOR)

**What's missing:** `record_conversation` uses a single `chrono::Utc::now()` for all events in a trace. Individual events don't have their actual occurrence time.

**Why it matters:** Can't measure per-step latency or time-to-first-response. Multi-step workflow timing analysis is impossible.

**Recommended fix:** If callers provide timestamps per message, `record_conversation` should preserve them. Adding an optional `timestamp` field to `ConversationMessage` would be backward-compatible and broadly useful.

### 5.4 No file state snapshot (MINOR)

**What's missing:** `input_state_hash` is always all-zeros. There's no snapshot of the codebase state before a session.

**Why it matters:** Can't reconstruct the exact file context the agent was working with. Replay would need to guess the starting state.

**Note:** This is partially addressed by Morph's commit/tree system, but runs aren't linked to commits.

---

## 6. Recommended Next Steps

### Priority 1: Enable structured event recording
Add an optional `metadata` field to `ConversationMessage`:

```rust
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    pub metadata: Option<BTreeMap<String, serde_json::Value>>,
}
```

In `record_conversation`, merge metadata into the event payload alongside `text`. This is backward-compatible and broadly useful — any caller can attach structured data without Morph needing to understand it.

### Priority 2: Record actual model names
Cursor hooks should extract the model name from the IDE context. The MCP record_session already supports this; hooks need to pass it through.

### Priority 3: Per-message timestamps
Add optional `timestamp` to `ConversationMessage`. If present, use it instead of the shared `now`.

### Priority 4: Link runs to commits/trees
When recording a session, if a HEAD commit exists, set `run.commit` to it. This enables codebase-state correlation without any evaluation logic in Morph.

---

## 7. Real Trace Examples

### 7.1 Typical hook-recorded trace (prompt + empty response)

```json
{
  "type": "trace",
  "events": [
    {
      "id": "evt_prompt",
      "seq": 0,
      "ts": "2026-04-10T20:30:00+00:00",
      "kind": "prompt",
      "payload": {"text": "Fix the authentication bug in login.rs"}
    },
    {
      "id": "evt_stop",
      "seq": 1,
      "ts": "2026-04-10T20:30:00+00:00",
      "kind": "response",
      "payload": {"text": "(task completed; response not captured by hook)"}
    }
  ]
}
```

**Issues:** Response is a placeholder. No tool/file events. Model is "cursor".

### 7.2 MCP-recorded session (user + assistant)

```json
{
  "type": "trace",
  "events": [
    {
      "id": "evt_0",
      "seq": 0,
      "ts": "2026-04-15T10:00:00+00:00",
      "kind": "user",
      "payload": {"text": "Implement the tap module for trace extraction..."}
    },
    {
      "id": "evt_1",
      "seq": 1,
      "ts": "2026-04-15T10:00:00+00:00",
      "kind": "assistant",
      "payload": {"text": "I'll create the tap module with the following structure..."}
    }
  ]
}
```

**Better:** Has actual response content. Model name may be correct if passed. Still no structured events.

### 7.3 What a rich trace would look like (not yet produced)

```json
{
  "type": "trace",
  "events": [
    {"id": "evt_0", "seq": 0, "ts": "...", "kind": "user", "payload": {"text": "Fix the bug in auth.rs"}},
    {"id": "evt_1", "seq": 1, "ts": "...", "kind": "assistant", "payload": {"text": "Let me read the file first."}},
    {"id": "evt_2", "seq": 2, "ts": "...", "kind": "file_read", "payload": {"path": "src/auth.rs", "content": "fn login()..."}},
    {"id": "evt_3", "seq": 3, "ts": "...", "kind": "tool_call", "payload": {"name": "edit_file", "input": "src/auth.rs"}},
    {"id": "evt_4", "seq": 4, "ts": "...", "kind": "tool_result", "payload": {"output": "file edited"}},
    {"id": "evt_5", "seq": 5, "ts": "...", "kind": "assistant", "payload": {"text": "Fixed the null check. Running tests now."}},
    {"id": "evt_6", "seq": 6, "ts": "...", "kind": "tool_call", "payload": {"name": "shell", "input": "cargo test"}},
    {"id": "evt_7", "seq": 7, "ts": "...", "kind": "tool_result", "payload": {"output": "test result: ok. 42 passed", "error": null}}
  ]
}
```

This trace would enable full agentic replay. Tap already handles this structure correctly.

---

## 8. Recording Gap Closure (v0.6.0)

As of v0.6.0, the recording gaps identified in sections 5.1–5.4 have been addressed:

### Closed gaps

| Gap | Resolution |
|-----|-----------|
| No structured events (§5.1) | Cursor hooks now parse `transcript_path` JSONL for `tool_use` → `file_read`, `file_edit`, `tool_call` events. OpenCode plugin extracts `tool-invocation`/`tool-result` parts. Claude Code hook checks for `transcript_path` and `conversation` fields. |
| Model name = "cursor" (§5.2) | All hooks now resolve `model` from the payload. Cursor hooks check `payload.model`, pending prompt rows, and environment. |
| All events share one timestamp (§5.3) | `ConversationMessage` now has an optional `timestamp` field. Per-message timestamps are preserved in traces when provided. |
| input_state_hash always zeros (§5.4) | `record_conversation` now links `run.commit` to HEAD when a commit exists. |
| No token/cost data | Cursor hooks capture `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens` into `run.environment.parameters`. Tap's `TapTokenUsage` extracts and exposes these. |
| MCP can't pass structured metadata | `MessageParam` now accepts optional `metadata` (arbitrary JSON) and `timestamp` fields. `morph-record.mdc` rule updated with guidance. |
| OpenCode drops tool parts | Plugin's `extractAllMessages` now processes `tool-invocation` and `tool-result` parts into structured messages with metadata. |

### What remains

- **Tool results from Cursor:** Cursor transcripts contain `tool_use` but not `tool_result` in the JSONL (results are streamed to the model but not written to the file). Tool *invocations* with inputs are recorded; outputs are not.
- **Diff contents in edits:** `StrReplace` tool_use can contain very large `old_string`/`new_string`. These are truncated to 2000 chars in the hooks.
- **Thinking blocks:** Cursor transcripts don't include model reasoning. This is an upstream Cursor limitation.
- **Claude Code structured data:** Claude Code may not yet provide `transcript_path` or `conversation`; the hook includes fallback to prompt+response and writes debug dumps for future investigation.

## 9. Conclusion

| Capability | Status |
|-----------|--------|
| Morph is sufficient for | Prompt-only replay, basic prompt/response evaluation, model/agent tracking, **agentic replay from Cursor transcript_path**, **tool-aware evaluation**, **token usage analysis** |
| Morph is insufficient for | Full diff-based assessment (tool results from Cursor unavailable), thinking-block analysis |
| All four recording paths updated | Cursor hooks (transcript parsing), MCP (metadata+timestamp), OpenCode (tool part extraction), Claude Code (structured event support + fallback) |
| The schema, extraction layer (tap), and recording callers are now aligned. |
