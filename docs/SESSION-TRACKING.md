# Session tracking

> **A diff plus a commit message is not enough to review AI-authored code.
> The next person needs the prompt.** This document explains why morph
> records every agent session as a content-addressed Run + Trace pair,
> why neither Claude/Cursor/OpenCode's on-disk transcripts nor OTEL
> hooks nor Langfuse/Phoenix can deliver what morph delivers, and what
> the morph trace contract actually guarantees.

When you read this, also read [`SECURITY.md`](SECURITY.md) — it tells
you exactly what is in a trace, what crosses the wire when, and what
you should know before you let any of it leave your laptop.

## Why a diff is not enough for AI-authored code

Code review of human-authored code rests on a contract that is so
familiar nobody states it: **the human committer can explain why the
code is the way it is.** If you don't understand a diff, you ask the
human; the human's recollection of their reasoning is the source of
truth, and the code review process is a conversation around that
recollection.

That contract collapses when an agent writes the code. The human who
runs `morph commit` (or `git commit`) often does not know why the
agent made specific choices — they read the diff, decided it looked
reasonable, and shipped it. If you ask "why did you use a HashMap
here?" the honest answer is "the agent picked it; I don't remember
the prompt." Code review is then reduced to "does it compile and pass
tests" — which is necessary but radically insufficient. The diff
shows you *what* changed; only the prompt + the trace shows you *why*.

So morph records the why. Every agent session — every prompt, every
response, every tool call, every file read, every file edit, every
shell stdout/stderr — lands as an immutable `Run` (the execution
receipt) plus a `Trace` (the full event log). Both are
content-addressed objects in the same DAG as the file tree. The
commit references them via `evidence_refs`, so when a reviewer asks
"what trace produced commit `abc123`?", the commit itself answers.

## Seven things you can only do because the prompt is in the commit graph

Each of these is impossible — or merely probabilistic — without
session tracking that is part of the version control DAG. With morph,
they are first-class.

### 1. Review the transformation, not just the output

A reviewer reading agent-authored code needs to see "the agent was
asked X, looked at files A/B/C, edited D, and ran the tests." Without
the prompt + trace, the reviewer is reverse-engineering the diff
into an intent — guessing at the agent's reasoning from the file
changes alone.

```
$ morph tap inspect abc123
=== Run abc123 ===
  prompt:    "Add retry logic to the auth service. The current
              implementation gives up on the first 5xx; we want
              exponential backoff up to 3 attempts."
  reads:     src/auth/service.rs, src/auth/client.rs
  edits:     src/auth/service.rs (+22/-3)
  shell:     cargo test --package auth   (passed)
  response:  "Added a `retry_with_backoff` helper. Three attempts,
              200ms initial delay, capped at 1.6s..."
```

A reviewer reading this knows what the agent was *trying* to do, what
it looked at to decide how, and what evidence it produced that the
change works. The diff alone gives you the third thing only.

### 2. The prompt is the closest thing to a spec

Months later, when someone asks "what was this code supposed to do?",
the prompt is the most truthful answer — closer to original intent
than the comments or even the tests. Comments rot. Tests describe
behavior at the boundary, not intent. The prompt is the human's
articulation of the goal at the moment they reached for an agent to
solve it. Morph keeps it.

This is a stronger guarantee than developer notes or PR descriptions:
the prompt is *causal* (the agent acted on it) and *immutable* (it's
content-addressed). If the prompt said "make this faster" and the
diff added a hash map, you can confirm that the agent's
interpretation of "faster" matches yours. If it doesn't, that's the
review finding.

### 3. Replay and regenerate

When the model upgrades or the codebase shifts, you want to re-run
the agent on the same prompt to see whether quality regressed. This
is impossible without the prompt stored next to the commit it
produced.

```
$ morph traces target-context abc123 > context.json
$ morph traces final-artifact abc123 > artifact.diff
# replay against a newer model:
$ <your-replay-tool> --prompt-from abc123 --model claude-opus-5
```

This is also how you catch when the agent's behavior on your team's
recurring tasks gets *better*: you replay last quarter's prompts
against this quarter's model, score the diffs, and see whether the
quality moved.

### 4. Attribution when something breaks

A bug shows up in production. Was it the prompt that was unclear?
The agent's interpretation? The model? The reviewer who waved it
through? The trace narrows the question; absence of the trace makes
it unanswerable.

Without morph: "I think the agent wrote this, but I don't remember
which session." With morph: `morph blame src/auth/service.rs:42` →
the `Run` that touched that line → the prompt that drove the run.

### 5. Promote prompts → acceptance cases

Morph's eval-driven workflow says every behavior change starts with
an acceptance case (see [`EVAL-DRIVEN.md`](EVAL-DRIVEN.md)). A prompt
that produced a working change is a candidate spec — you can extract
acceptance cases from prompts and check them into the eval suite.
Only possible if the prompt is stored.

```
$ morph eval add-case --from-trace abc123 --case-id auth_retry_on_5xx
```

The case captures the prompt as the intent, the test command as the
verifier, and the observed metrics as the baseline. Now the team's
eval suite reflects what the agent has been asked to do, not just
what the maintainers thought to write tests for.

### 6. Merge-aware behavioral context

When two agent branches merge, knowing what each agent *was trying
to do* is essential to deciding whether the merge preserves both
intents. The diff doesn't tell you that — two clean diffs can
silently merge into a regression because they were solving
different problems and now their solutions interfere. The prompt
does tell you: branch A was asked "make retries idempotent",
branch B was asked "speed up the auth path"; reading both prompts,
the reviewer can see whether the merged code still satisfies both
goals.

```
$ morph merge-plan main feature
=== bar to beat ===
  pass_rate: max(0.94, 0.91) = 0.94
  p95_ms:    min(340,  280)  = 280
=== case provenance ===
  introduces: auth_retry_on_5xx (from feature: prompt "make retries idempotent")
  introduces: auth_perf_p95     (from feature: prompt "speed up the auth path")
```

Merge gating uses the metrics; merge *review* uses the prompts.

### 7. Cross-tool portability

A team running Cursor, Claude Code, and OpenCode has three different
on-disk transcript formats in three locations. Morph normalizes them
into one shape, in one place, addressable by one hash — so reviewing
a teammate's work doesn't depend on what tool they used. The morph
trace contract is the same whether the underlying agent is
Anthropic's, OpenAI's, or a local model behind OpenCode.

## What morph records, concretely

A morph **Run** is a small JSON object:

```json
{
  "type": "run",
  "agent":   { "id": "claude-code", "model": "claude-sonnet-4" },
  "trace":   "46f82f6205ba...",
  "metrics": { "tests_passed": 42, "tests_total": 42, "pass_rate": 1.0 },
  "environment": { "cwd": "/repo", "branch": "feature/retry" },
  "contributors": [{ "actor": "raffi@example.com", "kind": "human" }]
}
```

A **Trace** is the event log the run points at — every prompt,
response, tool call, file read, file edit, shell command, in
order:

```json
{ "seq": 0, "kind": "prompt",    "text": "Add retry logic..." }
{ "seq": 1, "kind": "tool_call", "name": "read",  "input": "src/auth/service.rs" }
{ "seq": 2, "kind": "tool_call", "name": "edit",  "input": "src/auth/service.rs", "output": "@@ -1,3 +1,22 @@ ..." }
{ "seq": 3, "kind": "shell",     "input": "cargo test", "output": "test result: ok. 42 passed; 0 failed" }
{ "seq": 4, "kind": "response",  "text": "Added a `retry_with_backoff` helper..." }
```

Both objects are content-addressed (hash of canonical bytes), so
identity is mathematical rather than nominal. The commit references
the run via `evidence_refs`; the run references the trace; the trace
events reference the file paths the agent touched. The whole
transformation is queryable from the commit hash outward.

## Why Claude / Cursor / OpenCode on-disk transcripts aren't enough

Every agent tool worth its salt writes a session log to disk. Claude
Code keeps `~/.claude/projects/<encoded-path>/*.jsonl`. Cursor stores
sessions inside its own application support directory. OpenCode
maintains its own JSONL stream per session. This is a great
foundation — and it is also the floor, not the ceiling.

| Property morph needs | Claude / Cursor / OpenCode logs |
|---|---|
| Linked to a specific commit | No — correlated only by timestamp, if at all. |
| Content-addressed (immutable) | No — files can be rotated, edited, or deleted. |
| Visible to teammates | No — lives only on the developer's laptop. |
| Same shape across tools | No — each tool has its own JSONL schema. |
| Travels with the repo | No — lives outside the repo. |
| Survives a laptop wipe | Only if the developer separately backed up the directory. |

A reviewer cannot ask "show me the prompt that produced commit
`abc123`" and get a deterministic answer from on-disk transcripts.
At best they can sort the JSONL files by mtime, look for one whose
edits land roughly at that commit, and hope. Morph turns that
hope into `morph traces final-artifact abc123`.

## Why OTEL hooks aren't enough

A second class of solutions: emit OpenTelemetry spans from the
agent and ship them to a tracing backend (Tempo, Honeycomb,
Jaeger, etc.). This is fine for *operational* visibility — knowing
which agent calls are slow, which model errors are spiking — but
it is the wrong shape for code review.

| Property morph needs | OTEL spans in a tracing backend |
|---|---|
| Linked to a specific commit | At best correlated by tag/timestamp. The link is probabilistic, not causal. |
| Content-addressed | No — span IDs are random, not derived from contents. |
| Reviewable as part of code review | No — the trace lives in a separate dashboard. |
| Merge-aware | No — there's no way to ask "show me both branches' traces side-by-side". |
| Local-first | No — telemetry ships to a remote collector by default. |

The OTEL world is built around the assumption that you have one
production system emitting spans you sample for performance
analysis. The dev-loop world is built around the assumption that
every commit is a thing you'll review with a person. Morph picks
the latter.

## Why Langfuse / Phoenix / Helicone aren't enough

A third class: hosted LLM observability platforms — Langfuse,
Phoenix, Helicone, LangSmith, and friends. These are good products
for *production LLM observability*: you're running an inference
workload at scale, you want dashboards over latency, cost, output
quality, and prompt drift. Morph's job is different.

| Property morph needs | Langfuse / Phoenix / Helicone |
|---|---|
| Tied to a specific commit/branch/merge | No — they index by application, session, user, not by VCS commit. |
| Part of the version-control DAG | No — they're a separate system with separate identity. |
| Local-first | No — data leaves your laptop to hit the SaaS endpoint. |
| Merge-aware | No — they don't have a notion of "merge" at all. |
| Priced for per-commit visibility | No — pricing assumes high-volume production traffic, not one trace per developer per commit. |
| Available offline | No. |

If you are running an LLM-backed product in production, Langfuse or
Phoenix is probably right for that. They will not, however, answer
"what prompt produced commit `abc123`?" — that's not the question
they're optimized for.

## The morph trace contract

The properties morph traces have, that the alternatives don't:

1. **Content-addressed.** A trace's identity is its hash. You can
   reference it from a commit and trust the reference cannot be
   silently rewritten.
2. **Immutable.** Trace bytes are written once and never altered.
   New evidence creates new objects; rewrites are explicit — and
   the `morph forget` command (v0.41.0) leaves a content-addressed
   `Tombstone` object rather than silently deleting, so retirement
   is auditable even though the original bytes are gone.
3. **Linked from the commit.** The commit's `evidence_refs` field
   points at every Run that contributed to it. From a commit hash,
   the trace is one dereference away.
4. **Merge-aware.** When two branches merge, the merge commit
   inherits both parents' evidence references. The `morph
   merge-plan` command surfaces case provenance — which prompt
   introduced which acceptance case — across branches.
5. **Tool-agnostic.** Cursor's hook, Claude Code's hook, OpenCode's
   hook, and AoE all parse the agent's session into the same
   `Trace` shape. Reading a teammate's trace doesn't require
   knowing which agent they used.
6. **Local-first.** Traces live in `.morph/` on the developer's
   laptop. Sharing them with the team is opt-in via a morph
   remote — see [`SECURITY.md`](SECURITY.md) for the sharing model
   and the privacy implications.
7. **Designed to be read.** `morph tap`, `morph traces`, `morph
   serve`, and the MCP `morph_inspect_run` tool all exist
   specifically so a reviewer can ask the trace questions during
   code review.

## See also

- [`MORPH-AND-GIT.md`](MORPH-AND-GIT.md) — how morph wraps git in reference mode (the only mode since v0.40.0).
- [`SECURITY.md`](SECURITY.md) — what's in a trace, what crosses the wire when, and the team-sharing model. **Read this before you push to a morph remote.**
- [`EVAL-DRIVEN.md`](EVAL-DRIVEN.md) — the spec-first workflow that turns prompts into acceptance cases.
- [`THEORY.md`](THEORY.md) — the formal model: pipelines as monadic computations, traces as the event log, certificate vectors as the merge contract.
