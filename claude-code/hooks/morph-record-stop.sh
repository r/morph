#!/usr/bin/env bash
# Claude Code hook: Stop. If .morph/hooks/pending-<session_id>.jsonl exists, build Trace+Run with
# last_assistant_message and run `morph run record`.
# Logs: .morph/hooks/logs/claude-invoke.log, .morph/hooks/logs/morph-record.log, .morph/hooks/debug/last-Stop.json
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 - "$SCRIPT_DIR" << 'PY'
import json, subprocess, sys
from pathlib import Path
from datetime import datetime

raw = sys.stdin.read()
payload = json.loads(raw)
cwd = payload.get("cwd") or "."
session_id = payload.get("session_id") or "unknown"
response_text = payload.get("last_assistant_message") or ""

def write_debug(morph_dir, name, data):
    debug_dir = morph_dir / "hooks" / "debug"
    debug_dir.mkdir(parents=True, exist_ok=True)
    out = data.copy()
    if "last_assistant_message" in out and len(out.get("last_assistant_message", "")) > 500:
        out["last_assistant_message"] = out["last_assistant_message"][:500] + "... [truncated]"
        out["_response_truncated"] = True
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
    ["morph", "program", "identity-hash"],
    cwd=repo,
    capture_output=True,
    text=True,
)
if result.returncode != 0:
    sys.stderr.write(f"morph program identity-hash failed: {result.stderr}\n")
    sys.exit(1)
program_hash = result.stdout.strip()

run_obj = {
    "type": "run",
    "program": program_hash,
    "commit": None,
    "environment": {"model": "claude-code", "version": "1.0", "parameters": {}, "toolchain": {}},
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
    ["morph", "run", "record", "--run-file", str(run_path), "--trace", str(trace_path)],
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
