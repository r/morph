#!/usr/bin/env bash
# Cursor hook: afterAgentResponse. Payload includes "text" (full agent response) and "transcript_path" (rich JSONL).
# Parses transcript_path for tool_use events (Read, StrReplace, Shell, etc.) to build structured traces.
# Falls back to pending-*.jsonl prompt text + response text if transcript_path is unavailable.
# Logs: .morph/hooks/logs/cursor-invoke.log, .morph/hooks/logs/morph-record.log, .morph/hooks/debug/last-afterAgentResponse.json.
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
roots = payload.get("workspace_roots") or []
conversation_id = payload.get("conversation_id") or "unknown"
response_text = payload.get("text") or ""
model_name = payload.get("model") or "unknown"
transcript_path_str = payload.get("transcript_path") or ""

MAX_CONTENT_LEN = 2000

def truncate(s, limit=MAX_CONTENT_LEN):
    if s and len(s) > limit:
        return s[:limit] + "... [truncated]"
    return s

def write_debug(morph_dir, name, data):
    debug_dir = morph_dir / "hooks" / "debug"
    debug_dir.mkdir(parents=True, exist_ok=True)
    out = data.copy()
    if "text" in out and len(out["text"]) > 500:
        out["text"] = out["text"][:500] + "... [truncated]"
        out["_text_truncated"] = True
    with open(debug_dir / f"last-{name}.json", "w") as f:
        json.dump(out, f, indent=2)

def log_invoke(morph_dir, hook, cid):
    log_dir = morph_dir / "hooks" / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    with open(log_dir / "cursor-invoke.log", "a") as f:
        f.write(f"{datetime.utcnow().isoformat()}Z {hook} conversation_id={cid}\n")

def log_morph_record(morph_dir, cid, run_hash):
    log_dir = morph_dir / "hooks" / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    with open(log_dir / "morph-record.log", "a") as f:
        f.write(f"{datetime.utcnow().isoformat()}Z conversation_id={cid} run_hash={run_hash}\n")

FILE_READ_TOOLS = {"Read", "Grep", "Glob", "SemanticSearch"}
FILE_EDIT_TOOLS = {"StrReplace", "Write", "EditNotebook", "Delete"}

def tool_use_to_event(seq, tool_name, tool_input, now):
    """Map a Cursor transcript tool_use block to a structured trace event."""
    inp = tool_input or {}
    if tool_name in FILE_READ_TOOLS:
        kind = "file_read"
        path = inp.get("path") or inp.get("glob_pattern") or inp.get("pattern") or ""
        content = truncate(json.dumps(inp))
        payload = {"text": content, "name": tool_name, "path": path}
    elif tool_name in FILE_EDIT_TOOLS:
        kind = "file_edit"
        path = inp.get("path") or inp.get("target_notebook") or ""
        content = truncate(json.dumps(inp))
        payload = {"text": content, "name": tool_name, "path": path}
    elif tool_name == "Shell":
        kind = "tool_call"
        cmd = inp.get("command") or ""
        content = truncate(cmd)
        payload = {"text": content, "name": "Shell", "input": truncate(json.dumps(inp))}
    elif tool_name == "Task":
        kind = "tool_call"
        desc = inp.get("description") or inp.get("prompt") or ""
        content = truncate(desc)
        payload = {"text": content, "name": "Task", "input": truncate(json.dumps(inp))}
    elif tool_name == "CallMcpTool":
        kind = "tool_call"
        mcp_tool = inp.get("toolName") or ""
        content = f"{inp.get('server','')}/{mcp_tool}"
        payload = {"text": truncate(content), "name": f"mcp:{mcp_tool}", "input": truncate(json.dumps(inp))}
    else:
        kind = "tool_call"
        content = truncate(json.dumps(inp))
        payload = {"text": content, "name": tool_name}
        if inp:
            payload["input"] = truncate(json.dumps(inp))
    return {
        "id": f"evt_{seq}",
        "seq": seq,
        "ts": now,
        "kind": kind,
        "payload": payload,
    }

def parse_transcript(transcript_path, now):
    """Parse Cursor transcript JSONL into structured trace events."""
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
                        kind = "user" if role == "user" else "assistant"
                        text = part.get("text") or ""
                        events.append({
                            "id": f"evt_{seq}",
                            "seq": seq,
                            "ts": now,
                            "kind": kind,
                            "payload": {"text": text},
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
                            "id": f"evt_{seq}",
                            "seq": seq,
                            "ts": now,
                            "kind": "tool_result",
                            "payload": {"text": truncate(str(output))},
                        })
                        seq += 1
    except (IOError, OSError):
        return None
    return events if events else None

for root in roots:
    if not root:
        continue
    repo = Path(root).resolve()
    morph_dir = repo / ".morph"
    if not morph_dir.is_dir():
        continue
    pending = morph_dir / "hooks" / f"pending-{conversation_id}.jsonl"
    log_invoke(morph_dir, "afterAgentResponse", conversation_id)
    write_debug(morph_dir, "afterAgentResponse", payload)
    if not pending.exists():
        continue
    with open(pending) as f:
        lines = [ln.strip() for ln in f if ln.strip()]
    if not lines:
        pending.unlink(missing_ok=True)
        continue

    now = datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")

    # Try parsing transcript_path for rich structured events
    events = None
    if transcript_path_str:
        tp = Path(transcript_path_str)
        if tp.exists():
            events = parse_transcript(tp, now)

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
    trace_path = runs_dir / f"session-{conversation_id[:8]}-{stamp}.trace.json"
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
        continue
    trace_hash = result.stdout.strip()

    result = subprocess.run(
        ["morph", "pipeline", "identity-hash"],
        cwd=repo,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        sys.stderr.write(f"morph pipeline identity-hash failed: {result.stderr}\n")
        continue
    pipeline_hash = result.stdout.strip()

    # Resolve model: payload > pending prompts > fallback
    resolved_model = model_name
    if not resolved_model or resolved_model == "unknown":
        for line in lines:
            row = json.loads(line)
            if row.get("model"):
                resolved_model = row["model"]
                break
    if not resolved_model:
        resolved_model = "unknown"

    # Capture token usage from Cursor payload
    env_params = {}
    for key in ("input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens"):
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
        "agent": {"id": "cursor", "version": "1.0", "policy": None},
    }
    run_path = runs_dir / f"session-{conversation_id[:8]}-{stamp}.run.json"
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
        continue
    run_hash = result.stdout.strip()
    log_morph_record(morph_dir, conversation_id, run_hash)
    pending.unlink(missing_ok=True)
    break
PY
