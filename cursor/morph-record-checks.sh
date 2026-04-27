#!/usr/bin/env bash
# Phase 5b: Cursor stop-hook variant that surfaces unaddressed
# behavioral-evidence gaps before the agent ends a turn.
#
# Reads the same Cursor stop-hook payload as the other scripts in
# this directory, then runs `morph eval gaps --json` against every
# workspace root. Warnings are printed to stderr (Cursor surfaces
# them in the agent log) and never fail the hook so they can stack
# with the existing record-stop pipeline.
#
# Wire alongside the recording hook by appending to
# ~/.cursor/hooks.json:
#
#   {
#     "stop": [
#       "${HOME}/.cursor/hooks/morph-record-stop.sh",
#       "${HOME}/.cursor/hooks/morph-record-checks.sh"
#     ]
#   }
set -e
exec 3<&0
python3 - << 'PY'
import json, os, subprocess, sys
from pathlib import Path

raw = os.fdopen(3).read().strip()
if not raw:
    sys.exit(0)
try:
    payload = json.loads(raw)
except json.JSONDecodeError:
    sys.exit(0)

roots = payload.get("workspace_roots") or []
for root in roots:
    if not root:
        continue
    repo = Path(root).resolve()
    if not (repo / ".morph").is_dir():
        continue
    try:
        result = subprocess.run(
            ["morph", "eval", "gaps", "--json"],
            cwd=repo,
            capture_output=True,
            text=True,
            timeout=15,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired):
        continue
    if result.returncode not in (0, 1):
        continue
    try:
        data = json.loads(result.stdout or "{}")
    except json.JSONDecodeError:
        continue
    gaps = data.get("gaps") or []
    if not gaps:
        continue
    sys.stderr.write(
        f"morph: {len(gaps)} behavioral-evidence gap(s) at {repo}:\n"
    )
    for g in gaps:
        kind = g.get("kind", "?")
        hint = g.get("hint", "")
        sys.stderr.write(f"  - {kind}: {hint}\n")
PY
