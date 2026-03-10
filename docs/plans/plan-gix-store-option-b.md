# Plan: Gix-backed store (Option B) and migration

We adopt Git's object format and use **gitoxide (gix)** for the object store. Hashes change; we support a one-time migration from 0.0/0.1 so existing repos can be upgraded.

---

## 1. Hash format (Option B)

- **New hash** = SHA-256 of the Git object representation:  
  `"blob " + decimal_len + "\0" + canonical_json`
- So the same Morph object has a **different** hash than in 0.0/0.1 (where hash was SHA-256 of raw canonical JSON only).
- We do **not** tag each hash with a version. The **repo** has a single `repo_version`; that implies the hash format for every object in that repo. Anything that references a hash (commits, refs, external docs) is valid only in the context of that repo version. After migration, all refs and in-object references point to new hashes.

---

## 2. Store version 0.2

- **0.2**: Gix-backed object store. Objects stored as Git blobs via gix-odb (loose, optional pack). Hash = gix ObjectId (SHA-256 Git format).
- **Refs**: Can stay as files under `.morph/refs/` or move to a small index; either way they hold the new-format commit hashes.
- **Type index**: We still need `list(type)` (e.g. all Runs, all Commits). Options: (a) SQLite table `objects(hash, type)` written on put, or (b) keep type-index dirs keyed by new hash. SQLite fits the existing plan and avoids duplicate copies.

---

## 3. Migration: one-time, hashes change

- **Input**: Repo at 0.0 or 0.1.
- **Output**: Repo at 0.2; every object re-stored with Git-format hash; every reference (inside objects and in refs) updated to the new hashes.
- **User impact**: Any external reference to an old hash (URL, doc, log) will no longer resolve. For a single repo we can just run migration and not worry; document that "after migrating to 0.2, hashes changed."

### 3.1 Migration algorithm

1. **Load** all objects from the old store (0.0: scan `objects/*.json`; 0.1: read from SQLite + blob store). Build a list of `(old_hash, MorphObject)`.
2. **Build mapping** `old_hash → new_hash` by processing objects in **dependency order** so that when we rewrite an object, every hash it references is already in the map.

   **Order (objects that reference others come after their referents):**

   - **No refs:** Blob, EvalSuite, Trace, Artifact  
   - **Tree** (entries reference blob/tree hashes; process in an order where child trees before parents, or iterate until stable)  
   - **Pipeline** (prompts, eval_suite, graph node refs, provenance run/trace)  
   - **Commit** (pipeline, parents, eval_contract.suite)  
   - **Run** (pipeline, commit, trace, output_artifacts, agent.policy)  
   - **TraceRollup** (trace)  
   - **Annotation** (target; data may contain link target)

   For each object in that order:

   - **Rewrite** the object: replace every hash field with `map.get(old_hash).copied().unwrap_or(old_hash)` (or fail if missing and required).
   - **Serialize** to canonical JSON.
   - **Compute new hash** = Git-format SHA-256 of `"blob " + len + "\0" + json`.
   - **Write** to gix store (and type index if used).
   - **Record** `map[old_hash] = new_hash` (use the object’s *own* old hash as key when we’re done, so the next object that references this one gets the new hash).

   Note: the “old hash” we have for each object is the 0.0/0.1 content hash. When we rewrite and re-hash, the new hash is what we store and put in the map.

3. **Update refs**: For HEAD and each `refs/heads/<branch>`, resolve the current commit hash (old), look up `map[old_commit_hash]`, write the ref to the new commit hash. Set `repo_version` to `"0.2"` in config.
4. **Optional**: Rename or remove the old `objects/` (or old store) for rollback safety; document the one-way migration.

### 3.2 Hash fields to rewrite (per object type)

| Type       | Fields to substitute (hash → new hash) |
|-----------|----------------------------------------|
| Tree      | `entries[].hash` |
| Pipeline   | `prompts[]`, `eval_suite`, `graph.nodes[].ref`, `provenance.derived_from_run`, `derived_from_trace` |
| Commit    | `pipeline`, `parents[]`, `eval_contract.suite` |
| Run       | `pipeline`, `commit`, `trace`, `output_artifacts[]`, `agent.policy` |
| TraceRollup | `trace` |
| Annotation | `target`, and `data.target` when kind is link |
| Blob, EvalSuite, Trace, Artifact | none |

---

## 4. Versioning summary

- **Repo-level only:** `repo_version` in `.morph/config.json` is `"0.0"`, `"0.1"`, or `"0.2"`. It defines storage layout and hash format for the entire repo.
- **No per-hash version:** We do not store or pass “hash format version” next to each hash. After migration, the repo only contains 0.2 hashes; anything that “references a hash” (refs, commit graph, annotations) uses that single format.
- **External refs:** Docs or tools that stored an old hash string must be updated or discarded after migration; we don’t support resolving old-format hashes in a 0.2 repo.

---

## 5. Implementation sketch

- **Hash:** New function (e.g. in `hash.rs`) or variant: `content_hash_git(obj) -> Hash` = SHA-256(`"blob " + len.to_string() + "\0" + canonical_json`). In 0.2, this is the only content hash used for storage.
- **Store:** New backend `GixStore` implementing `Store`, using `gix_odb` for put/get and optional SQLite (or files) for refs + type index. `put` serializes to canonical JSON, computes Git-format hash, writes blob via gix `Sink::write_buf(Kind::Blob, bytes)`.
- **Migration:** CLI command e.g. `morph migrate-store` (or `morph upgrade`). Reads `repo_version`; if 0.0 or 0.1, runs the migration above and sets 0.2; if already 0.2, no-op or error.
- **Open repo:** If `repo_version` is 0.2, use GixStore; else use existing FsStore / SqliteStore.

---

## 6. Dependencies

- Add `gix-odb`, `gix-object`, `gix-hash` (with SHA-256). Optionally `gix-repository` if we want refs and discovery from a Git-like directory.

---

## 7. Relation to plan-blob-store-sqlite.md

- **0.1** (SQLite + blob store) can still be implemented as an intermediate step; migration 0.0 → 0.1 and 0.1 → 0.2 both supported.
- **0.2** replaces the “blob store” with gix (so we get Git’s loose/pack layout and tooling) and changes the hash to Git format. The “SQLite index” for type/refs in 0.1 can be reused in 0.2 (refs + `objects(hash, type)`), with payloads in gix only.
