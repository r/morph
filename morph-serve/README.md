# morph-serve

Library and HTTP service for browser-based Morph repo inspection. Powers both `morph serve` (multi-repo hosted service) and `morph visualize` (single-repo quick look).

## Usage

```bash
morph serve                                       # serve current repo at http://127.0.0.1:8765
morph serve --repo team=/path/to/repo             # named multi-repo mode
morph serve --org-policy org-policy.json          # apply org-level policy
morph serve --port 3000 --interface 0.0.0.0       # custom bind

morph visualize                                   # alias: single-repo quick view
morph visualize /path/to/repo --port 3000         # explicit path + port
```

Open the printed URL in a browser.

## What you see

- **Commit strip** — list of commits from HEAD with behavioral status badges (certified, gate passed/failed, merge dominance).
- **Detail panel** — message, author, pipeline hash, eval contract (suite + observed metrics), prompts, evidence refs.
- **Object browser** — paste any hash to inspect blobs, trees, commits, runs, traces, pipelines, annotations.

## API

All repo-scoped endpoints live under `/api/repos/{name}/...`.

| Endpoint | Method | Returns |
|----------|--------|---------|
| `/api/repos` | GET | List of configured repos with summary stats |
| `/api/repos/{repo}/summary` | GET | Repo summary: head, branches, commit/run counts |
| `/api/repos/{repo}/branches` | GET | Branch listing with current branch |
| `/api/repos/{repo}/commits` | GET | Commit history from HEAD with behavioral badges |
| `/api/repos/{repo}/commits/{hash}` | GET | Full commit detail with behavioral status |
| `/api/repos/{repo}/runs` | GET | Run listing |
| `/api/repos/{repo}/runs/{hash}` | GET | Run detail with agent, environment, metrics |
| `/api/repos/{repo}/traces/{hash}` | GET | Trace events |
| `/api/repos/{repo}/pipelines/{hash}` | GET | Pipeline graph, provenance, attribution |
| `/api/repos/{repo}/objects/{hash}` | GET | Raw object JSON |
| `/api/repos/{repo}/annotations/{hash}` | GET | Annotations on a target |
| `/api/repos/{repo}/policy` | GET | Effective policy (repo + org merged) |
| `/api/repos/{repo}/gate/{hash}` | GET | Gate check result for a commit |
| `/api/org/policy` | GET/POST | Organization-level policy |

Backward-compatible single-repo endpoints (`/api/log`, `/api/runs`, `/api/object/{hash}`, `/api/graph`) route to the default repo.

The single-page app is embedded in the binary via `include_str!`; no separate static directory at runtime.
