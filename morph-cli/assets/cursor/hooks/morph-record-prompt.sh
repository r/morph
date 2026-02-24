#!/usr/bin/env bash
# Cursor hook: beforeSubmitPrompt. Appends prompt to .morph/hooks/pending-<conversation_id>.jsonl for each Morph repo in workspace_roots.
# Logs: .morph/hooks/logs/cursor-invoke.log (Cursor called us), .morph/hooks/debug/last-beforeSubmitPrompt.json (payload for inspection).
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 - "$SCRIPT_DIR" << 'PY'
import json, sys
from pathlib import Path
from datetime import datetime

raw = sys.stdin.read()
payload = json.loads(raw)
roots = payload.get("workspace_roots") or []
conversation_id = payload.get("conversation_id") or "unknown"
generation_id = payload.get("generation_id") or ""
prompt = payload.get("prompt") or ""
ts = payload.get("timestamp") or datetime.utcnow().isoformat() + "Z"

# Debug: write last payload (prompt truncated) so you can verify what Cursor sent
def write_debug(morph_dir, name, data):
    debug_dir = morph_dir / "hooks" / "debug"
    debug_dir.mkdir(parents=True, exist_ok=True)
    out = data.copy()
    if "prompt" in out and len(out["prompt"]) > 500:
        out["prompt"] = out["prompt"][:500] + "... [truncated]"
        out["_prompt_truncated"] = True
    with open(debug_dir / f"last-{name}.json", "w") as f:
        json.dump(out, f, indent=2)

def log_invoke(morph_dir, hook, cid):
    log_dir = morph_dir / "hooks" / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    with open(log_dir / "cursor-invoke.log", "a") as f:
        f.write(f"{datetime.utcnow().isoformat()}Z {hook} conversation_id={cid}\n")

for root in roots:
    if not root:
        continue
    morph_dir = Path(root).resolve() / ".morph"
    if not morph_dir.is_dir():
        continue
    hooks_dir = morph_dir / "hooks"
    hooks_dir.mkdir(parents=True, exist_ok=True)
    # 1) Prove Cursor called us
    log_invoke(morph_dir, "beforeSubmitPrompt", conversation_id)
    write_debug(morph_dir, "beforeSubmitPrompt", payload)
    # 2) Append prompt to pending
    pending = hooks_dir / f"pending-{conversation_id}.jsonl"
    line = json.dumps({"ts": ts, "prompt": prompt, "generation_id": generation_id}) + "\n"
    with open(pending, "a") as f:
        f.write(line)
PY
