#!/usr/bin/env bash
# Cursor hook: afterAgentResponse. Payload includes "text" (full agent response).
# If .morph/hooks/pending-<conversation_id>.jsonl exists, build Trace+Run with real response text and run `morph run record`.
# Logs: .morph/hooks/logs/cursor-invoke.log, .morph/hooks/logs/morph-record.log, .morph/hooks/debug/last-afterAgentResponse.json (payload, text truncated).
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
    events = []
    for seq, line in enumerate(lines):
        row = json.loads(line)
        events.append({
            "id": f"evt_prompt_{seq}",
            "seq": seq,
            "ts": row.get("ts", now),
            "kind": "prompt",
            "payload": {"text": row.get("prompt", "")},
        })
    events.append({
        "id": "evt_response",
        "seq": len(events),
        "ts": now,
        "kind": "response",
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
    program_hash = result.stdout.strip()

    run_obj = {
        "type": "run",
        "program": program_hash,
        "commit": None,
        "environment": {"model": "cursor", "version": "1.0", "parameters": {}, "toolchain": {}},
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
