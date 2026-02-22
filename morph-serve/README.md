# morph-serve

Library that powers `morph visualize`. Serves a Morph repo for browser-based browsing. No export — reads `.morph/` directly.

## Run via CLI

From a Morph repo (directory that contains `.morph/`):

```bash
morph visualize
# optional path (default .):
morph visualize /path/to/morph/repo
# optional port (default 8765) and interface (default 127.0.0.1):
morph visualize --port 3000 --interface 0.0.0.0
```

Then open the printed URL (e.g. **http://127.0.0.1:8765**) in a browser.

## What you get

- **Commit strip** — list of commits from `HEAD`; click one to see details.
- **Detail panel** — message, author, program hash, eval contract (suite + metrics), and a list of **prompts** (expand to read Blob content).
- **Browse object** — paste any object hash and click Fetch to view Blobs (content), Trees (entries), or other objects.

## API

- `GET /api/log` — JSON array of commits (hash, message, author, timestamp, program, parents, eval_contract).
- `GET /api/object/<hash>` — JSON for that object (commit, program, blob, tree, etc.).

The single-page app is embedded in the binary; no separate static directory is needed at runtime.
