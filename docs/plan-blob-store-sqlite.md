# Plan: Content-addressed blob store + SQLite index

Goal: Replace the flat `objects/` directory with (1) a content-addressed blob store for payloads and (2) a SQLite index for fast type/ref lookups and metadata, without changing the public `Store` trait. All work is done TDD (tests first, then implementation). Repo versioning is introduced so we can migrate between storage layouts safely.

---

## Nomenclature

- **v0**: The first thing we release publicly. The spec lives in `v0-spec.md`; we keep that name even as we iterate. “v0” is the product/spec line, not a store version.
- **Store version**: We version the **store layout** so we can migrate between formats. Store versions use **0.0** and **0.1** (and later 0.2, …) so we can iterate within the same public “v0” release.

---

## Repo / store versioning

We version the morph repo **store layout** so future storage changes can be migrated cleanly.

- **Where**: Version lives in `.morph/config.json` as `repo_version` (or `store_version`). Use **string values `"0.0"` and `"0.1"`**. If missing, treat as `"0.0"`.
- **Semantics**:
  - **0.0 (current)**: FsStore. Flat `objects/<hash>.json`, type-index dirs (`runs/`, `traces/`, `prompts/`, `evals/`), refs as files under `refs/`.
  - **0.1**: SQLite store + blob store. `.morph/store.db` (index + refs), payloads in blob store (e.g. sharded `objects/` or DB). No duplicate type-index dirs.
- **0.2**: Gix-backed store (Git object format, SHA-256). See [plan-gix-store-option-b.md](plan-gix-store-option-b.md). Migration from 0.0/0.1 rewrites all objects to new hashes.
- **Migration 0.0 → 0.1**: Read every JSON blob from the 0.0 `objects/` directory (all `objects/<hash>.json` files), deserialize to `MorphObject`, and `put` each into the new store so they exist in SQLite + blob store. Copy all refs (HEAD and `refs/heads/*`) into the new refs table. Then set `repo_version` to `"0.1"` and optionally rename old `objects/` to `objects.old` (or leave for rollback). Yes — we migrate **all** JSON blobs over; the new store becomes the single source of truth.
- **Opening a repo**: Read `repo_version` (default `"0.0"`). If `"0.0"`, use FsStore; if `"0.1"`, use SqliteStore + BlobStore. Reject unknown versions with a clear error (e.g. "Unsupported repo_version …; upgrade morph or migrate manually").

---

## Current state

- **Store trait**: `put`, `get`, `has`, `list(type)`, `ref_read`, `ref_write`.
- **FsStore**: Objects in `objects/<hash>.json`; type-index copies in `runs/`, `traces/`, `prompts/`, `evals/`; refs in `refs/` (files). Commit/HEAD logic in `commit.rs` uses `FsStore` directly for `refs_dir()` and raw HEAD content (`ref: heads/main`).
- **Hash**: SHA-256 of canonical JSON; must remain the same so existing hashes stay valid.

---

## Target architecture

1. **Blob store**  
   - Holds one immutable blob per object: key = content hash (same as today).  
   - Optional: compression (e.g. zstd) before store; decompress on read. Hash is still over canonical JSON (so we hash before compress, store compressed).  
   - Implementation options: sharded directory `objects/aa/bb...json[.zst]`, or a single SQLite table `blobs(hash, blob)`.

2. **SQLite index**  
   - **objects**: `(hash PRIMARY KEY, type TEXT NOT NULL)` — no payload, just “this hash exists and has this type.”  
   - **refs**: `(name TEXT PRIMARY KEY, value TEXT)` — ref names (e.g. `HEAD`, `heads/main`) and values (hash or `ref: heads/main`).  
   - **Optional later**: FTS table over blob content for search; extra columns (e.g. `created_at`) for analytics.

3. **Single store implementation**  
   - One backend that implements `Store`: on `put`, write canonical JSON → hash → store blob (blob store) + insert/ignore row in SQLite `objects` and update type indices if needed; on `get`, read from blob store; on `list(type)`, query SQLite; refs from SQLite.  
   - This replaces FsStore for “new” repos or when enabled by config; FsStore can remain for backward compatibility and migration.

4. **Ref handling**  
   - Today: `commit.rs` uses `FsStore::refs_dir()` and file paths for HEAD. To support the new backend we need ref operations to go through the trait.  
   - Extend usage so that symbolic ref (e.g. `ref: heads/main`) and hash resolution use `ref_read` only, or add `ref_read_raw(name) -> Option<String>` to the trait and implement for both FsStore (read file) and the new store (read from SQLite `refs` table). Then `commit.rs` and CLI use the trait instead of `refs_dir()`.

---

## TDD discipline

Every phase follows **Red → Green → Refactor**:

1. **Red**: Write tests that define the desired behavior. Run them; they must fail (or be unimplemented).
2. **Green**: Implement the minimum code to make the tests pass.
3. **Refactor**: Clean up without changing behavior; tests stay green.

No production code for a feature without a failing test first. Shared behavior (e.g. `Store` contract) is covered by tests that run against **all** store implementations (FsStore now; FsStore + SqliteStore later).

---

## Phases

### Phase 0: Repo / store versioning (TDD)

**Red — write failing tests**

- **repo_version in config**: Test that `init_repo` writes `config.json` containing `repo_version: "0.0"`. Test that reading config from an existing `.morph` returns `repo_version` ("0.0" if key missing).
- **version reader**: Introduce `read_repo_version(morph_dir: &Path) -> Result<String, MorphError>` (or a small version type); test that it returns "0.0" for current init, and defaults to "0.0" for missing config.
- **version on init**: Test that a newly inited repo has `repo_version: "0.0"` in config.

**Green — implement**

- Add `repo_version: "0.0"` to the config written in `init_repo`.
- Add `read_repo_version()` that reads `.morph/config.json` and returns `repo_version` (default "0.0" if missing). Reject unknown versions in a later phase when we have 0.1.

**Refactor**

- Optional: small config module (read/write config, typed struct) if it keeps repo.rs clear.

**Exit criterion**: Tests pass; every new repo has `repo_version: "0.0"` in config; we can read it.

---

### Phase 1: Trait and ref abstraction (TDD)

**Red — write failing tests**

- **ref_read_raw**: Test that `store.ref_read_raw("HEAD")` returns `Some("ref: heads/main\n")` (or trimmed) for an inited FsStore repo. Test that `ref_read_raw("heads/main")` returns the hash after a commit. Test that `ref_read_raw("nonexistent")` returns `None`. Add trait method `ref_read_raw` to `Store`; FsStore impl can be stubbed to fail first.
- **ref_write_raw**: Test that after `store.ref_write_raw("HEAD", "ref: heads/main\n")`, `ref_read_raw("HEAD")` returns that value. Test that after `store.ref_write_raw("heads/main", "<hash>")`, `ref_read` resolves to that hash. Add `ref_write_raw(name, value: &str)` to trait.
- **commit uses trait only**: Integration test: create commit via public API (create_commit with `dyn Store`); resolve_head and current_branch work. Then change `commit::resolve_head` and friends to take `dyn Store` (or a trait that has `ref_read_raw`/`ref_read`) and remove use of `refs_dir()`. Write test that uses only the trait and a FsStore; test fails until ref resolution is trait-based.
- **ref_write for hash refs**: Ensure existing `ref_write(name, hash)` keeps working; document that ref value is opaque (hash or symbolic). Test: ref_write then ref_read returns the hash.

**Green — implement**

- Add `ref_read_raw` and `ref_write_raw` to `Store`; implement for FsStore (read/write file under refs_dir).
- In `commit.rs`, replace direct `FsStore` ref file access with `store.ref_read_raw` / `store.ref_write_raw` (and keep `ref_write` for hash refs). Add a helper that resolves symbolic refs via repeated ref_read_raw so `resolve_head` and `current_branch` work through the trait.
- Update call sites in morph-cli and morph-mcp to use the trait for ref access.

**Refactor**

- Extract symbolic-ref resolution into a small function used by both FsStore and (later) SqliteStore.

**Exit criterion**: All ref access goes through the trait; existing FsStore and commit tests pass; new ref_read_raw/ref_write_raw tests pass.

---

### Phase 2: SQLite index only — no blob store yet (TDD)

**Red — write failing tests**

- **Store contract for SqliteStore**: Run the same unit tests as for FsStore (put_get_roundtrip, ref_write_read, get_missing_returns_not_found, list_filters_by_object_type, put_prompt_blob_creates_type_index if still applicable) against SqliteStore. Either parameterize tests with a store factory or add a second test module that builds SqliteStore and runs identical behavior. Tests fail until SqliteStore exists and implements Store.
- **ref_read_raw / ref_write_raw for SqliteStore**: Test that SqliteStore returns the same ref_read_raw/ref_write_raw behavior as FsStore (HEAD symbolic, heads/main hash).
- **SqliteStore init**: Test that opening a path creates `.morph/store.db` and tables `objects` and `refs`; test that put/get/has/list/ref_* work. Test that init_repo (store 0.0) does not create store.db.

**Green — implement**

- Add `rusqlite` to morph-core/Cargo.toml.
- New module (e.g. `store_sqlite.rs`): struct SqliteStore, open or create `.morph/store.db`, schema `objects(hash TEXT PRIMARY KEY, type TEXT NOT NULL, json TEXT NOT NULL)` and `refs(name TEXT PRIMARY KEY, value TEXT NOT NULL)`.
- Implement Store for SqliteStore: put → canonical_json, hash, insert into objects (skip type-index dirs); get → read json from objects; has/list/ref_read/ref_write/ref_read_raw/ref_write_raw. Symbolic ref resolution: ref_read_raw returns value; resolve_head logic follows "ref: " to next name.
- Run all Store tests against SqliteStore; fix until green.

**Refactor**

- Shared test harness: one function that takes a Store (or factory) and runs the full Store contract test list for both FsStore and SqliteStore.

**Exit criterion**: All Store tests pass for both FsStore and SqliteStore; refs and commit behavior work with SqliteStore.

---

### Phase 3: Blob store + index split (TDD)

**Red** — Write failing tests: BlobStore put_bytes/get_bytes/has and sharded path; SqliteStore with BlobStore (no json column); idempotent put. **Green** — BlobStore trait + ShardedBlobStore; SqliteStore delegates payload to BlobStore; objects table (hash, type) only. **Refactor** — Optional small-object inlining later. **Exit**: SqliteStore uses BlobStore; all tests pass.

*(The following legacy bullets are superseded by the TDD phases above and the Phase 4 migration tests.)* (and anywhere else that uses `refs_dir()` or raw HEAD):
  - Replace direct `FsStore` ref access with `store.ref_read_raw` / `store.ref_write` (or a small helper that uses the trait). Keep `ref_write` for hash refs; for symbolic HEAD we need to write the string `ref: heads/main` — so either:
    - `ref_write(name, value)` where value is either a hash or the literal string for symbolic refs, or
    - Keep `ref_write(name, hash)` for hash refs and add `ref_write_raw(name, content)` for symbolic refs.
- Decide and document: ref value in trait is “opaque string” (hash or `ref: ...`) so that both files and SQLite can store it the same way. Implement `ref_read_raw` and `ref_write_raw` (or generalize `ref_write` to accept a string) and use them everywhere refs are read/written.
### Phase 2: SQLite index only (no blob store yet) — see Phase 2 TDD above

- Add dependency: `rusqlite` (and optionally `bundled` for portability).
- New module: `morph_core::store::sqlite` (or `store_sqlite.rs`).
- **SqliteStore** (or similar name):
  - Open/create `.morph/store.db` under repo root.
  - Schema:  
    - `objects(hash TEXT PRIMARY KEY, type TEXT NOT NULL)`  
    - `refs(name TEXT PRIMARY KEY, value TEXT NOT NULL)`
  - For now, store the **full JSON in SQLite** as well: add column `objects.json BLOB` (or `TEXT`). So: one table `objects(hash, type, json)`, one table `refs(name, value)`.
  - Implement `Store`: put → insert into `objects` and update refs; get → read `objects.json`; has → exists in `objects`; list(type) → `SELECT hash FROM objects WHERE type = ?`; ref_read → resolve via `refs` (symbolic ref: value is `ref: heads/main`, so read again by name `heads/main` until you get a hash); ref_write/ref_write_raw → update `refs`.
  - Implement `ref_read_raw` and symbolic ref handling so HEAD and branch refs work like FsStore.
- **Init**: Either extend `init_repo` to create `store.db` and not create `objects/` when a config flag is set, or add `init_repo_v2` / config key `store.backend = "sqlite"` that creates the DB and the two tables. For Phase 2, still store JSON in the DB (no separate blob store).
- **Exit criterion**: New repo (or test) can use SqliteStore; all `Store` tests pass with SqliteStore; refs and commit/checkout behavior match FsStore.

### Phase 3: Blob store + index split

- **Blob storage**: Introduce a small abstraction (e.g. `BlobStore` trait or module): `put_bytes(hash, bytes)`, `get_bytes(hash) -> Option<Vec<u8>>`, `has(hash)`. First implementation: **sharded directory** under `.morph/objects/` (e.g. `objects/<first 2 of hash>/<hash>.json` or `.zst`). Hash is still of the canonical JSON; we can store compressed bytes keyed by the same hash (hash is computed before compression).
- **SqliteStore** (Phase 3):
  - Remove `json` column from `objects` (or keep only for small objects; see below).
  - On put: serialize to canonical JSON, compute hash, write bytes to BlobStore (optionally compress before write), insert `(hash, type)` into SQLite `objects`.
  - On get: look up hash in SQLite (to confirm type if desired), then read bytes from BlobStore; decompress if needed; deserialize.
  - Optional: “inline” small objects (e.g. length &lt; 2KB) in SQLite to avoid tiny files; large objects go to blob store only. Defer if you want to keep the first version simple.
- **Compression**: Optional. If enabled: after canonical_json, compress with zstd; store compressed bytes in BlobStore under same hash; on get, read and decompress then deserialize. Hash remains over canonical JSON so compatibility is preserved.
- **Exit criterion**: SqliteStore uses BlobStore for payloads; SQLite holds only index + refs; all tests pass; no duplicate type-index dirs (runs/traces/prompts/evals) for SqliteStore.

### Phase 4: Migration 0.0 → 0.1 and config-driven store

- **Migration**: Given existing `.morph` with store 0.0 (FsStore), read each `objects/<hash>.json`, put each into SqliteStore + BlobStore, copy refs into SQLite `refs`, set `repo_version` to `"0.1"`; optionally rename `objects` to `objects.old`. Run when user opts in (e.g. `morph migrate-store`).
- **Config / open_repo**: Read `repo_version` (default `"0.0"`). If `"0.0"`, use FsStore; if `"0.1"`, use SqliteStore + BlobStore. Reject unknown versions with a clear error.
- **Backward compatibility**: Missing config or `"0.0"` ⇒ FsStore. Document both layouts in v0-spec (see Phase 5).

### Phase 5: Update v0-spec.md (spec catch-up)

Ensure `docs/v0-spec.md` matches the implementation so the spec stays the single source of truth for the public v0 release.

**Checklist (update spec with):**

- **Nomenclature**: v0 = first public release (spec name stays v0-spec.md); store versions 0.0, 0.1, … for layout iteration.
- **Repo / store versioning**: `repo_version` in `.morph/config.json`, string `"0.0"` or `"0.1"`; default `"0.0"`; unknown ⇒ error.
- **Layout 0.0**: FsStore — flat `objects/<hash>.json`, type-index dirs (`runs/`, `traces/`, `prompts/`, `evals/`), refs as files under `refs/`.
- **Layout 0.1**: SqliteStore + BlobStore — `.morph/store.db` (schema: `objects(hash, type[, json])`, `refs(name, value)`); blob store (e.g. sharded `objects/<aa>/<hash>.json`). No duplicate type-index dirs.
- **Store trait**: Document `ref_read_raw`, `ref_write_raw` (opaque ref value: hash or `ref: heads/…`); ref resolution via trait only.
- **Migration 0.0 → 0.1**: All JSON blobs from `objects/` + all refs → new store; then `repo_version: "0.1"`. CLI: `morph migrate-store` (or equivalent).
- **Opening a repo**: How `open_repo` / `find_repo` choose store by `repo_version`.

**When**: After Phase 4 (migration and open_repo) is implemented. Optionally refresh spec again after Phase 3 (blob store) so 0.1 layout is accurate.

**Exit criterion**: v0-spec.md reads as the authoritative description of repo versioning, both layouts, migration, and Store API.

### Phase 6 (optional): Dedup and compression

- **Deduplication**: Today each object is independent. Optional future: for large blobs (e.g. traces), split into chunks, hash chunks, store chunks in BlobStore, object row points to “manifest” (list of chunk hashes). Defer until you have evidence that trace/blob size is a problem.
- **Compression**: Enable zstd in BlobStore for new writes; migration can recompress when copying from FsStore.
- **FTS**: Add SQLite FTS5 table over object content (e.g. prompt text, run metadata) for search; populated on put or in a background pass.

---

## File and dependency changes (summary)

| Phase | Changes |
|-------|--------|
| 0 | `repo.rs`: config with `repo_version: "0.0"`; `read_repo_version(morph_dir)`. |
| 1 | `store.rs`: extend trait `ref_read_raw`, `ref_write_raw`. `commit.rs`, `morph-cli`, `morph-mcp`: use trait for all ref access. |
| 2 | `Cargo.toml`: add `rusqlite`. New `store_sqlite.rs`. SqliteStore implements Store (objects + refs in DB). Shared Store test harness for FsStore and SqliteStore. |
| 3 | New `blob_store.rs` (ShardedBlobStore). SqliteStore delegates payload to BlobStore; `objects` table drops json column. |
| 4 | Migration 0.0→0.1 (all JSON blobs + refs); `open_repo` by `repo_version` ("0.0" / "0.1"); config-driven store selection. |
| 5 | Update `v0-spec.md`: nomenclature (v0 vs store 0.0/0.1), both layouts, Store API (ref_read_raw/ref_write_raw), migration, open_repo. |
| 6 | Optional: zstd, FTS, chunked blobs (TDD). |

---

## Testing strategy (TDD)

- **Per phase**: Red (write failing tests) → Green (implement) → Refactor. No production code without a failing test first.
- **Store contract**: One shared test suite (put/get/has/list/ref_read/ref_write/ref_read_raw/ref_write_raw) run against both FsStore and SqliteStore (parameterized or store factory).
- **Integration**: init repo, record/commit/annotate, read back via same store; compare FsStore vs SqliteStore behavior.
- **Migration**: Create store-0.0 repo with N objects and refs, run migrate to 0.1, assert new store has same N objects and refs; assert idempotent.

---

## Docs

- **Phase 5**: Catch up `docs/v0-spec.md` § Storage backend to describe the “blob store + SQLite index” layout and when it’s used.
- Short note in README or CURSOR-SETUP: repo_version (0.0 / 0.1), store backend, and morph migrate-store (once implemented).
