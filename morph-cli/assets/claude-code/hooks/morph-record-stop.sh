#!/usr/bin/env bash
# Claude Code hook: Stop. If .morph/hooks/pending-<session_id>.jsonl exists, build Trace+Run with
# last_assistant_message and run `morph run record`.
# Checks for `transcript_path` or `conversation` in payload for structured events.
# Logs: .morph/hooks/logs/claude-invoke.log, .morph/hooks/logs/morph-record.log, .morph/hooks/debug/last-Stop.json
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec 3<&0  # preserve original stdin before heredoc replaces it
python3 - "$SCRIPT_DIR" << 'PY'
import json, os, subprocess, sys
from pathlib import Path
from datetime import datetime

raw = os.fdopen(3).read().strip()
if not raw:
    sys.exit(0)
try:
    payload = json.loads(raw)
except json.JSONDecodeError:
    sys.exit(0)
cwd = payload.get("cwd") or "."
session_id = payload.get("session_id") or "unknown"
response_text = payload.get("last_assistant_message") or ""
model_name = payload.get("model") or os.environ.get("ANTHROPIC_MODEL") or ""
transcript_path_str = payload.get("transcript_path") or ""
conversation = payload.get("conversation") or []

MAX_CONTENT_LEN = 2000

def truncate(s, limit=MAX_CONTENT_LEN):
    if s and len(s) > limit:
        return s[:limit] + "... [truncated]"
    return s

def write_debug(morph_dir, name, data):
    debug_dir = morph_dir / "hooks" / "debug"
    debug_dir.mkdir(parents=True, exist_ok=True)
    out = data.copy()
    if "last_assistant_message" in out and len(out.get("last_assistant_message", "")) > 500:
        out["last_assistant_message"] = out["last_assistant_message"][:500] + "... [truncated]"
        out["_response_truncated"] = True
    # Always write full payload keys for diagnostic purposes
    out["_payload_keys"] = list(data.keys())
    with open(debug_dir / f"last-{name}.json", "w") as f:
        json.dump(out, f, indent=2)

def log_invoke(morph_dir, hook, sid):
    log_dir = morph_dir / "hooks" / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    with open(log_dir / "claude-invoke.log", "a") as f:
        f.write(f"{datetime.utcnow().isoformat()}Z {hook} session_id={sid}\n")

def log_morph_record(morph_dir, sid, run_hash):
    log_dir = morph_dir / "hooks" / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    with open(log_dir / "morph-record.log", "a") as f:
        f.write(f"{datetime.utcnow().isoformat()}Z session_id={sid} run_hash={run_hash}\n")

FILE_READ_TOOLS = {"Read", "Grep", "Glob", "SemanticSearch"}
FILE_EDIT_TOOLS = {"StrReplace", "Write", "EditNotebook", "Delete"}

def tool_use_to_event(seq, tool_name, tool_input, now):
    inp = tool_input or {}
    if tool_name in FILE_READ_TOOLS:
        kind = "file_read"
        path = inp.get("path") or inp.get("glob_pattern") or inp.get("pattern") or ""
        content = truncate(json.dumps(inp))
        event_payload = {"text": content, "name": tool_name, "path": path}
    elif tool_name in FILE_EDIT_TOOLS:
        kind = "file_edit"
        path = inp.get("path") or inp.get("target_notebook") or ""
        content = truncate(json.dumps(inp))
        event_payload = {"text": content, "name": tool_name, "path": path}
    elif tool_name == "Shell" or tool_name == "Bash":
        kind = "tool_call"
        cmd = inp.get("command") or ""
        content = truncate(cmd)
        event_payload = {"text": content, "name": tool_name, "input": truncate(json.dumps(inp))}
    elif tool_name == "Task":
        kind = "tool_call"
        desc = inp.get("description") or inp.get("prompt") or ""
        content = truncate(desc)
        event_payload = {"text": content, "name": "Task", "input": truncate(json.dumps(inp))}
    else:
        kind = "tool_call"
        content = truncate(json.dumps(inp))
        event_payload = {"text": content, "name": tool_name}
        if inp:
            event_payload["input"] = truncate(json.dumps(inp))
    return {
        "id": f"evt_{seq}",
        "seq": seq,
        "ts": now,
        "kind": kind,
        "payload": event_payload,
    }

def parse_transcript(transcript_path, now):
    """Parse a JSONL transcript file (same format as Cursor transcripts)."""
    events = []
    seq = 0
    try:
        with open(transcript_path) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    entry = json.loads(line)
                except json.JSONDecodeError:
                    continue
                role = entry.get("role") or "unknown"
                message = entry.get("message") or {}
                content_parts = message.get("content") or []
                if isinstance(content_parts, str):
                    content_parts = [{"type": "text", "text": content_parts}]

                for part in content_parts:
                    ptype = part.get("type") or "text"
                    if ptype == "text":
                        kind = "user" if role == "user" or role == "human" else "assistant"
                        text = part.get("text") or ""
                        events.append({
                            "id": f"evt_{seq}", "seq": seq, "ts": now,
                            "kind": kind, "payload": {"text": text},
                        })
                        seq += 1
                    elif ptype == "tool_use":
                        tool_name = part.get("name") or "unknown_tool"
                        tool_input = part.get("input") or {}
                        events.append(tool_use_to_event(seq, tool_name, tool_input, now))
                        seq += 1
                    elif ptype == "tool_result":
                        output = part.get("content") or part.get("output") or ""
                        if isinstance(output, list):
                            output = " ".join(str(o.get("text","")) if isinstance(o, dict) else str(o) for o in output)
                        events.append({
                            "id": f"evt_{seq}", "seq": seq, "ts": now,
                            "kind": "tool_result",
                            "payload": {"text": truncate(str(output))},
                        })
                        seq += 1
    except (IOError, OSError):
        return None
    return events if events else None

def parse_conversation(conv_list, now):
    """Parse a conversation array (list of message objects) from Claude Code payload."""
    events = []
    seq = 0
    for msg in conv_list:
        if not isinstance(msg, dict):
            continue
        role = msg.get("role") or "unknown"
        content = msg.get("content")
        if isinstance(content, str):
            kind = "user" if role == "user" or role == "human" else "assistant"
            events.append({
                "id": f"evt_{seq}", "seq": seq, "ts": now,
                "kind": kind, "payload": {"text": content},
            })
            seq += 1
        elif isinstance(content, list):
            for part in content:
                if not isinstance(part, dict):
                    continue
                ptype = part.get("type") or "text"
                if ptype == "text":
                    kind = "user" if role == "user" or role == "human" else "assistant"
                    events.append({
                        "id": f"evt_{seq}", "seq": seq, "ts": now,
                        "kind": kind, "payload": {"text": part.get("text", "")},
                    })
                    seq += 1
                elif ptype == "tool_use":
                    events.append(tool_use_to_event(seq, part.get("name",""), part.get("input",{}), now))
                    seq += 1
                elif ptype == "tool_result":
                    out = part.get("content") or part.get("output") or ""
                    if isinstance(out, list):
                        out = " ".join(str(o.get("text","")) if isinstance(o, dict) else str(o) for o in out)
                    events.append({
                        "id": f"evt_{seq}", "seq": seq, "ts": now,
                        "kind": "tool_result",
                        "payload": {"text": truncate(str(out))},
                    })
                    seq += 1
    return events if events else None

repo = Path(cwd).resolve()
morph_dir = repo / ".morph"
if not morph_dir.is_dir():
    sys.exit(0)
pending = morph_dir / "hooks" / f"pending-{session_id}.jsonl"
log_invoke(morph_dir, "Stop", session_id)
write_debug(morph_dir, "Stop", payload)
if not pending.exists():
    sys.exit(0)
with open(pending) as f:
    lines = [ln.strip() for ln in f if ln.strip()]
if not lines:
    pending.unlink(missing_ok=True)
    sys.exit(0)

now = datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")

# Try structured sources in order of richness
events = None
if transcript_path_str:
    tp = Path(transcript_path_str)
    if tp.exists():
        events = parse_transcript(tp, now)
if not events and conversation:
    events = parse_conversation(conversation, now)

# Fallback: build events from pending prompts + response text
if not events:
    events = []
    for seq, line in enumerate(lines):
        row = json.loads(line)
        events.append({
            "id": f"evt_prompt_{seq}",
            "seq": seq,
            "ts": row.get("ts", now),
            "kind": "user",
            "payload": {"text": row.get("prompt", "")},
        })
    events.append({
        "id": "evt_response",
        "seq": len(events),
        "ts": now,
        "kind": "assistant",
        "payload": {"text": response_text},
    })

trace_obj = {"type": "trace", "events": events}
runs_dir = morph_dir / "runs"
runs_dir.mkdir(parents=True, exist_ok=True)
stamp = datetime.utcnow().strftime("%Y%m%d-%H%M%S")
trace_path = runs_dir / f"session-{session_id[:8]}-{stamp}.trace.json"
with open(trace_path, "w") as f:
    json.dump(trace_obj, f, indent=2)

result = subprocess.run(
    ["morph", "hash-object", str(trace_path)],
    cwd=repo,
    capture_output=True,
    text=True,
)
if result.returncode != 0:
    sys.stderr.write(f"morph hash-object failed: {result.stderr}\n")
    sys.exit(1)
trace_hash = result.stdout.strip()

result = subprocess.run(
    ["morph", "pipeline", "identity-hash"],
    cwd=repo,
    capture_output=True,
    text=True,
)
if result.returncode != 0:
    sys.stderr.write(f"morph pipeline identity-hash failed: {result.stderr}\n")
    sys.exit(1)
pipeline_hash = result.stdout.strip()

resolved_model = model_name
if not resolved_model:
    for line in lines:
        row = json.loads(line)
        if row.get("model"):
            resolved_model = row["model"]
            break
if not resolved_model:
    resolved_model = "unknown"

# Capture token usage if available
env_params = {}
for key in ("input_tokens", "output_tokens", "total_tokens"):
    val = payload.get(key)
    if val is not None:
        env_params[key] = val

run_obj = {
    "type": "run",
    "pipeline": pipeline_hash,
    "commit": None,
    "environment": {"model": resolved_model, "version": "1.0", "parameters": env_params, "toolchain": {}},
    "input_state_hash": "0" * 64,
    "output_artifacts": [],
    "metrics": {},
    "trace": trace_hash,
    "agent": {"id": "claude-code", "version": "1.0", "policy": None},
}
run_path = runs_dir / f"session-{session_id[:8]}-{stamp}.run.json"
with open(run_path, "w") as f:
    json.dump(run_obj, f, indent=2)

result = subprocess.run(
    ["morph", "run", "record", str(run_path), "--trace", str(trace_path)],
    cwd=repo,
    capture_output=True,
    text=True,
)
if result.returncode != 0:
    sys.stderr.write(f"morph run record failed: {result.stderr}\n")
    sys.exit(1)
run_hash = result.stdout.strip()
log_morph_record(morph_dir, session_id, run_hash)
pending.unlink(missing_ok=True)
PY
