#!/usr/bin/env bash
# Claude Code hook: UserPromptSubmit. Appends prompt to .morph/hooks/pending-<session_id>.jsonl.
# Logs: .morph/hooks/logs/claude-invoke.log, .morph/hooks/debug/last-UserPromptSubmit.json
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec 3<&0  # preserve original stdin before heredoc replaces it
python3 - "$SCRIPT_DIR" << 'PY'
import json, os, sys
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
prompt = payload.get("prompt") or ""
ts = datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")

def write_debug(morph_dir, name, data):
    debug_dir = morph_dir / "hooks" / "debug"
    debug_dir.mkdir(parents=True, exist_ok=True)
    out = data.copy()
    if "prompt" in out and len(out["prompt"]) > 500:
        out["prompt"] = out["prompt"][:500] + "... [truncated]"
        out["_prompt_truncated"] = True
    with open(debug_dir / f"last-{name}.json", "w") as f:
        json.dump(out, f, indent=2)

def log_invoke(morph_dir, hook, sid):
    log_dir = morph_dir / "hooks" / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    with open(log_dir / "claude-invoke.log", "a") as f:
        f.write(f"{datetime.utcnow().isoformat()}Z {hook} session_id={sid}\n")

repo = Path(cwd).resolve()
morph_dir = repo / ".morph"
if not morph_dir.is_dir():
    sys.exit(0)
hooks_dir = morph_dir / "hooks"
hooks_dir.mkdir(parents=True, exist_ok=True)
log_invoke(morph_dir, "UserPromptSubmit", session_id)
write_debug(morph_dir, "UserPromptSubmit", payload)
pending = hooks_dir / f"pending-{session_id}.jsonl"
line = json.dumps({"ts": ts, "prompt": prompt}) + "\n"
with open(pending, "a") as f:
    f.write(line)
PY
