# morph-serve

Library that powers `morph visualize`. Serves a Morph repo for browser-based browsing.

## Usage

From any Morph repo (directory containing `.morph/`):

```bash
morph visualize                                    # default: 127.0.0.1:8765
morph visualize /path/to/repo                      # explicit path
morph visualize --port 3000 --interface 0.0.0.0    # custom bind
```

Open the printed URL in a browser.

## What you see

- **Commit strip** -- list of commits from HEAD; click to expand.
- **Detail panel** -- message, author, pipeline hash, eval contract (suite + observed metrics), prompts.
- **Object browser** -- paste any hash to inspect blobs, trees, commits, runs, etc.

## API

| Endpoint | Returns |
|----------|---------|
| `GET /api/log` | JSON array of commits (hash, message, author, timestamp, pipeline, parents, eval_contract) |
| `GET /api/object/<hash>` | JSON for any stored object |

The single-page app is embedded in the binary via `include_str!`; no separate static directory at runtime.
