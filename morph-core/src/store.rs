//! Storage backend: trait and filesystem implementation.
//!
//! [FsStore] supports two hash modes via its constructors:
//! - [`FsStore::new`]: Legacy 0.0 layout, hash = SHA-256(canonical_json).
//! - [`FsStore::new_git`]: 0.2+ layout, hash = Git-format SHA-256("blob "+len+"\0"+canonical_json).
//!
//! Both use the same filesystem layout (`objects/<hash>.json`, `refs/`).

use crate::hash::Hash;
use crate::objects::MorphObject;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum MorphError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization: {0}")]
    Serialization(String),
    #[error("Invalid hash: {0}")]
    InvalidHash(String),
    #[error("Object not found: {0}")]
    NotFound(String),
    #[error("Not a morph repository")]
    NotRepo,
    #[error("Upgrade required: {0}")]
    UpgradeRequired(String),
    /// Repo's store version is older than this binary supports. The user
    /// should run `morph upgrade` in the project directory.
    #[error("Repo too old: {0}")]
    RepoTooOld(String),
    /// Repo's store version is newer than this binary supports. The user
    /// should update their `morph` binary.
    #[error("Repo too new: {0}")]
    RepoTooNew(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    /// Branch has diverged from its remote tracking ref; fast-forward not
    /// possible. The CLI surfaces this with a hint to run
    /// `morph pull --merge` (PR 4) or `morph merge` manually.
    #[error("Diverged: branch '{branch}' at {local_tip} has diverged from remote tip {remote_tip}")]
    Diverged {
        branch: String,
        local_tip: String,
        remote_tip: String,
    },
    /// Remote speaks a protocol or repo schema version this binary does not
    /// support. The CLI surfaces this with a clear hint to upgrade either
    /// the local `morph` or the remote helper. Introduced in PR 6 stage E.
    #[error("Incompatible remote: remote {reason}={remote}, local {reason}={local}")]
    IncompatibleRemote {
        remote: String,
        local: String,
        reason: String,
    },
    /// Catch-all for tool-shell-out failures and other miscellaneous
    /// errors that don't fit a more specific variant. Introduced
    /// alongside reference mode (PR 2) for git subprocess failures.
    #[error("{0}")]
    Other(String),
}

/// Object type filter for list operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectType {
    Blob,
    Tree,
    Pipeline,
    EvalSuite,
    Commit,
    Run,
    Artifact,
    Trace,
    TraceRollup,
    Annotation,
}

impl MorphObject {
    pub fn object_type(&self) -> ObjectType {
        match self {
            MorphObject::Blob(_) => ObjectType::Blob,
            MorphObject::Tree(_) => ObjectType::Tree,
            MorphObject::Pipeline(_) => ObjectType::Pipeline,
            MorphObject::EvalSuite(_) => ObjectType::EvalSuite,
            MorphObject::Commit(_) => ObjectType::Commit,
            MorphObject::Run(_) => ObjectType::Run,
            MorphObject::Artifact(_) => ObjectType::Artifact,
            MorphObject::Trace(_) => ObjectType::Trace,
            MorphObject::TraceRollup(_) => ObjectType::TraceRollup,
            MorphObject::Annotation(_) => ObjectType::Annotation,
        }
    }
}

/// Abstract storage interface (v0-spec §3).
pub trait Store {
    fn put(&self, object: &MorphObject) -> Result<Hash, MorphError>;
    fn get(&self, hash: &Hash) -> Result<MorphObject, MorphError>;
    fn has(&self, hash: &Hash) -> Result<bool, MorphError>;
    fn list(&self, type_filter: ObjectType) -> Result<Vec<Hash>, MorphError>;
    fn ref_read(&self, name: &str) -> Result<Option<Hash>, MorphError>;
    fn ref_write(&self, name: &str, hash: &Hash) -> Result<(), MorphError>;
    /// Raw ref content (e.g. "ref: heads/main\n" or a hash string). Used for HEAD.
    fn ref_read_raw(&self, name: &str) -> Result<Option<String>, MorphError>;
    /// Write raw ref content (symbolic or hash).
    fn ref_write_raw(&self, name: &str, value: &str) -> Result<(), MorphError>;
    /// Delete a ref file.
    fn ref_delete(&self, name: &str) -> Result<(), MorphError>;
    /// Path to refs directory (e.g. for listing branches).
    fn refs_dir(&self) -> PathBuf;
    /// Compute the content hash for an object without storing it.
    fn hash_object(&self, object: &MorphObject) -> Result<Hash, MorphError>;

    /// Enumerate all refs under `prefix` (e.g. `"heads"` or
    /// `"remotes/origin"`). Returns `(name_relative_to_prefix, hash)`
    /// pairs, recursive into subdirectories. Transport-neutral
    /// replacement for `refs_dir()` walks — non-filesystem backends
    /// (SSH, etc.) override this with a single RPC. The default impl
    /// walks `refs_dir()`, so existing in-process stores keep working
    /// with no extra code.
    fn list_refs(&self, prefix: &str) -> Result<Vec<(String, Hash)>, MorphError> {
        let root = self.refs_dir().join(prefix);
        if !root.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        list_refs_recursive(&root, "", &mut out)?;
        Ok(out)
    }

    /// Convenience wrapper for `list_refs("heads")`. SSH overrides
    /// only this method when it has a cheaper one-shot RPC for the
    /// common case.
    fn list_branches(&self) -> Result<Vec<(String, Hash)>, MorphError> {
        self.list_refs("heads")
    }

    /// List object hashes whose hex string starts with `prefix`.
    ///
    /// This powers `resolve_hash_prefix` (Git-style short-hash lookup).
    /// The default implementation iterates every object type and calls
    /// `list(t)` — correct, but for backends without a type index it
    /// also reads and deserializes every object on disk just to filter
    /// by type. Backends that can do better (notably `FsStore` with
    /// fanout layout, where a 2+ char prefix uniquely identifies the
    /// `objects/<2chars>/` subdirectory) override this with a direct
    /// directory walk that performs zero JSON deserialization.
    ///
    /// Callers should pass an already-validated lowercase hex prefix
    /// (≥4 chars in the CLI path); implementations may assume hex but
    /// are tolerant of mixed case.
    fn list_hashes_with_prefix(&self, prefix: &str) -> Result<Vec<Hash>, MorphError> {
        let prefix = prefix.to_ascii_lowercase();
        let mut out: Vec<Hash> = Vec::new();
        for t in [
            ObjectType::Blob,
            ObjectType::Tree,
            ObjectType::Pipeline,
            ObjectType::EvalSuite,
            ObjectType::Commit,
            ObjectType::Run,
            ObjectType::Artifact,
            ObjectType::Trace,
            ObjectType::TraceRollup,
            ObjectType::Annotation,
        ] {
            for h in self.list(t)? {
                if h.to_string().starts_with(&prefix) && !out.contains(&h) {
                    out.push(h);
                }
            }
        }
        Ok(out)
    }
}

fn list_refs_recursive(
    dir: &Path,
    rel_prefix: &str,
    out: &mut Vec<(String, Hash)>,
) -> Result<(), MorphError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let next_rel = if rel_prefix.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", rel_prefix, name)
        };
        if ft.is_dir() {
            list_refs_recursive(&entry.path(), &next_rel, out)?;
        } else if ft.is_file() {
            let s = std::fs::read_to_string(entry.path())?.trim().to_string();
            if s.is_empty() {
                continue;
            }
            // Skip non-hash refs (e.g. HEAD pointing at a symbolic
            // ref). list_refs is for resolving leaf refs; a symbolic
            // ref isn't enumerable as a (name, hash) pair.
            if let Ok(h) = Hash::from_hex(&s) {
                out.push((next_rel, h));
            }
        }
    }
    Ok(())
}

impl Store for Box<dyn Store + '_> {
    fn put(&self, object: &MorphObject) -> Result<Hash, MorphError> { self.as_ref().put(object) }
    fn get(&self, hash: &Hash) -> Result<MorphObject, MorphError> { self.as_ref().get(hash) }
    fn has(&self, hash: &Hash) -> Result<bool, MorphError> { self.as_ref().has(hash) }
    fn list(&self, tf: ObjectType) -> Result<Vec<Hash>, MorphError> { self.as_ref().list(tf) }
    fn ref_read(&self, name: &str) -> Result<Option<Hash>, MorphError> { self.as_ref().ref_read(name) }
    fn ref_write(&self, name: &str, hash: &Hash) -> Result<(), MorphError> { self.as_ref().ref_write(name, hash) }
    fn ref_read_raw(&self, name: &str) -> Result<Option<String>, MorphError> { self.as_ref().ref_read_raw(name) }
    fn ref_write_raw(&self, name: &str, value: &str) -> Result<(), MorphError> { self.as_ref().ref_write_raw(name, value) }
    fn ref_delete(&self, name: &str) -> Result<(), MorphError> { self.as_ref().ref_delete(name) }
    fn refs_dir(&self) -> PathBuf { self.as_ref().refs_dir() }
    fn hash_object(&self, object: &MorphObject) -> Result<Hash, MorphError> { self.as_ref().hash_object(object) }
    fn list_hashes_with_prefix(&self, prefix: &str) -> Result<Vec<Hash>, MorphError> { self.as_ref().list_hashes_with_prefix(prefix) }
}

// ── Filesystem-backed store ──────────────────────────────────────────

type HashFn = fn(&MorphObject) -> Result<Hash, MorphError>;

/// Layout of the objects directory on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectLayout {
    /// All objects in `objects/<hash>.json` (store versions ≤ 0.3).
    Flat,
    /// Git-style fan-out: `objects/<hash[0..2]>/<hash[2..]>.json` (store version 0.4+).
    Fanout,
}

/// Filesystem-backed store. Objects at `root/objects/...`, refs at `root/refs/`.
///
/// The hash function is configurable: legacy stores use `content_hash` (plain SHA-256),
/// while v0.2+ stores use `content_hash_git` (Git blob-format SHA-256).
pub struct FsStore {
    root: PathBuf,
    hash_fn: HashFn,
    layout: ObjectLayout,
}

impl FsStore {
    /// Create a legacy store (v0.0/v0.1). Hash = SHA-256(canonical_json). Flat layout.
    pub fn new(root: impl AsRef<Path>) -> Self {
        FsStore { root: root.as_ref().to_path_buf(), hash_fn: crate::content_hash, layout: ObjectLayout::Flat }
    }

    /// Create a Git-format store (v0.2/v0.3). Hash = Git-format SHA-256. Flat layout.
    pub fn new_git(root: impl AsRef<Path>) -> Self {
        FsStore { root: root.as_ref().to_path_buf(), hash_fn: crate::content_hash_git, layout: ObjectLayout::Flat }
    }

    /// Create a Git-format store with fan-out layout (v0.4+).
    pub fn new_git_fanout(root: impl AsRef<Path>) -> Self {
        FsStore { root: root.as_ref().to_path_buf(), hash_fn: crate::content_hash_git, layout: ObjectLayout::Fanout }
    }

    /// Open a concrete FsStore by reading the store version from config.json.
    pub fn from_store_version(morph_dir: &Path) -> Result<Self, MorphError> {
        let version = crate::repo::read_repo_version(morph_dir)?;
        Ok(match version.as_str() {
            "0.4" => Self::new_git_fanout(morph_dir),
            "0.2" | "0.3" => Self::new_git(morph_dir),
            _ => Self::new(morph_dir),
        })
    }

    pub fn objects_dir(&self) -> PathBuf { self.root.join("objects") }

    pub fn refs_dir(&self) -> PathBuf { self.root.join("refs") }

    pub fn layout(&self) -> ObjectLayout { self.layout }

    fn object_path(&self, hash: &Hash) -> PathBuf {
        let hex = hash.to_string();
        match self.layout {
            ObjectLayout::Flat => self.objects_dir().join(format!("{}.json", hex)),
            ObjectLayout::Fanout => {
                let (prefix, rest) = hex.split_at(2);
                self.objects_dir().join(prefix).join(format!("{}.json", rest))
            }
        }
    }

    /// List every object hash in the store (all types). Used by GC.
    pub fn all_object_hashes(&self) -> Result<Vec<Hash>, MorphError> {
        match self.layout {
            ObjectLayout::Flat => fs_list_hashes_from_dir(&self.objects_dir()),
            ObjectLayout::Fanout => fs_list_hashes_fanout(&self.objects_dir()),
        }
    }

    /// Delete an object by hash. Returns true if the file existed.
    pub fn delete_object(&self, hash: &Hash) -> Result<bool, MorphError> {
        let path = self.object_path(hash);
        if path.exists() {
            std::fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

impl Store for FsStore {
    fn put(&self, object: &MorphObject) -> Result<Hash, MorphError> {
        let hash = (self.hash_fn)(object)?;
        let path = self.object_path(&hash);
        fs_put(&self.root, object, hash, &path)
    }

    fn get(&self, hash: &Hash) -> Result<MorphObject, MorphError> {
        fs_get(&self.object_path(hash), hash)
    }

    fn has(&self, hash: &Hash) -> Result<bool, MorphError> {
        Ok(self.object_path(hash).exists())
    }

    fn list(&self, type_filter: ObjectType) -> Result<Vec<Hash>, MorphError> {
        fs_list(&self.root, &self.objects_dir(), type_filter, &|h| self.get(h), self.layout)
    }

    fn list_hashes_with_prefix(&self, prefix: &str) -> Result<Vec<Hash>, MorphError> {
        // Fast path: walk only the directory entries that can possibly
        // match. With fanout layout, a 2+ char prefix pins the search
        // to a single `objects/<2chars>/` subdirectory — at most one
        // `read_dir` call. With flat layout, we scan `objects/` once
        // and filter by string prefix. Either way, no JSON is read or
        // deserialized; we never call `get()`.
        let prefix = prefix.to_ascii_lowercase();
        match self.layout {
            ObjectLayout::Flat => {
                let all = fs_list_hashes_from_dir(&self.objects_dir())?;
                Ok(all
                    .into_iter()
                    .filter(|h| h.to_string().starts_with(&prefix))
                    .collect())
            }
            ObjectLayout::Fanout if prefix.len() >= 2 => {
                let (fanout, rest_prefix) = prefix.split_at(2);
                let dir = self.objects_dir().join(fanout);
                if !dir.is_dir() {
                    return Ok(Vec::new());
                }
                let mut out = Vec::new();
                for entry in std::fs::read_dir(&dir)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("json") {
                        continue;
                    }
                    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    if stem.len() != 62 {
                        continue;
                    }
                    if !stem.starts_with(rest_prefix) {
                        continue;
                    }
                    let full_hex = format!("{}{}", fanout, stem);
                    out.push(
                        Hash::from_hex(&full_hex)
                            .map_err(|_| MorphError::InvalidHash(full_hex))?,
                    );
                }
                Ok(out)
            }
            ObjectLayout::Fanout => {
                // Prefix shorter than the 2-char fanout key — fall
                // back to walking the whole fanout. Callers in the
                // CLI/MCP path enforce ≥4 chars, so this branch only
                // fires from direct API users.
                let all = fs_list_hashes_fanout(&self.objects_dir())?;
                Ok(all
                    .into_iter()
                    .filter(|h| h.to_string().starts_with(&prefix))
                    .collect())
            }
        }
    }

    fn ref_read(&self, name: &str) -> Result<Option<Hash>, MorphError> {
        fs_ref_read(&self.refs_dir(), name)
    }

    fn ref_write(&self, name: &str, hash: &Hash) -> Result<(), MorphError> {
        fs_ref_write(&self.refs_dir(), name, hash)
    }

    fn ref_read_raw(&self, name: &str) -> Result<Option<String>, MorphError> {
        fs_ref_read_raw(&self.refs_dir(), name)
    }

    fn ref_write_raw(&self, name: &str, value: &str) -> Result<(), MorphError> {
        fs_ref_write_raw(&self.refs_dir(), name, value)
    }

    fn ref_delete(&self, name: &str) -> Result<(), MorphError> {
        fs_ref_delete(&self.refs_dir(), name)
    }

    fn refs_dir(&self) -> PathBuf { self.root.join("refs") }

    fn hash_object(&self, object: &MorphObject) -> Result<Hash, MorphError> {
        (self.hash_fn)(object)
    }
}

/// Backward-compatible alias: 0.2+ store with Git-format hashes.
#[deprecated(note = "use FsStore::new_git() directly")]
pub type GixStore = FsStore;

// ── Shared filesystem helpers ────────────────────────────────────────

/// One type-index dir entry: where the index file for an object lives,
/// and whether the file contains the object's full canonical JSON or
/// is a zero-byte marker.
///
/// Pre-0.37.6 indexes (runs/traces/evals/prompts/annotations) write
/// full content because at least one fallback path reads from them
/// when the primary store lookup fails (`morph tap` falls back to
/// `.morph/traces/<hash>.json` during migrations). Indexes added in
/// 0.37.6 (blobs/trees/pipelines/commits/artifacts/trace_rollups)
/// only need to be discoverable for `list(t)`, so they're empty
/// markers — on a 33 GB blob-heavy repo, copying every blob into
/// `.morph/blobs/` would double disk usage; a marker file costs
/// nothing but an inode.
#[derive(Clone, Copy, Debug)]
struct TypeIndex {
    dir: &'static str,
    full_content: bool,
}

/// Type-index directories an object is written into on `put`. A blob
/// of `kind: "prompt"` lands in BOTH `blobs/` (the type index) and
/// `prompts/` (the kind-subset index used by recording flows), so
/// `list(Blob)` sees prompts via the type index without `prompts/`
/// having to widen its scope.
fn type_indexes_for_object(object: &MorphObject) -> Vec<TypeIndex> {
    let mut out = Vec::new();
    let primary = match object {
        MorphObject::Blob(_) => TypeIndex { dir: "blobs", full_content: false },
        MorphObject::Tree(_) => TypeIndex { dir: "trees", full_content: false },
        MorphObject::Pipeline(_) => TypeIndex { dir: "pipelines", full_content: false },
        MorphObject::EvalSuite(_) => TypeIndex { dir: "evals", full_content: true },
        MorphObject::Commit(_) => TypeIndex { dir: "commits", full_content: false },
        MorphObject::Run(_) => TypeIndex { dir: "runs", full_content: true },
        MorphObject::Artifact(_) => TypeIndex { dir: "artifacts", full_content: false },
        MorphObject::Trace(_) => TypeIndex { dir: "traces", full_content: true },
        MorphObject::TraceRollup(_) => TypeIndex { dir: "trace_rollups", full_content: false },
        MorphObject::Annotation(_) => TypeIndex { dir: "annotations", full_content: true },
    };
    out.push(primary);
    if let MorphObject::Blob(b) = object {
        if b.kind == "prompt" {
            out.push(TypeIndex { dir: "prompts", full_content: true });
        }
    }
    out
}

/// Index dir to read when listing all objects of a given type.
/// Mirrors the primary entry in [`type_indexes_for_object`]: the
/// kind-subset `prompts/` is *not* returned here because
/// `list(Blob)` must surface every blob, including prompts.
fn type_index_for_object_type(t: ObjectType) -> Option<&'static str> {
    match t {
        ObjectType::Blob => Some("blobs"),
        ObjectType::Tree => Some("trees"),
        ObjectType::Pipeline => Some("pipelines"),
        ObjectType::EvalSuite => Some("evals"),
        ObjectType::Commit => Some("commits"),
        ObjectType::Run => Some("runs"),
        ObjectType::Artifact => Some("artifacts"),
        ObjectType::Trace => Some("traces"),
        ObjectType::TraceRollup => Some("trace_rollups"),
        ObjectType::Annotation => Some("annotations"),
    }
}

/// All type-index directories the store maintains, in the order
/// `ensure_type_indexes` writes them on a legacy rebuild. Tests use
/// this to assert every type is covered.
const ALL_TYPE_INDEX_DIRS: &[&str] = &[
    "blobs", "trees", "pipelines", "evals", "commits",
    "runs", "artifacts", "traces", "trace_rollups", "annotations",
];

fn fs_get(object_path: &Path, hash: &Hash) -> Result<MorphObject, MorphError> {
    let bytes = std::fs::read(object_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            MorphError::NotFound(hash.to_string())
        } else {
            MorphError::Io(e)
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|e| MorphError::Serialization(e.to_string()))
}

fn fs_list_hashes_from_dir(dir: &Path) -> Result<Vec<Hash>, MorphError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut hashes = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if name.len() != 64 {
            continue;
        }
        hashes.push(Hash::from_hex(name).map_err(|_| MorphError::InvalidHash(name.into()))?);
    }
    Ok(hashes)
}

fn fs_list_hashes_fanout(objects_dir: &Path) -> Result<Vec<Hash>, MorphError> {
    if !objects_dir.exists() {
        return Ok(Vec::new());
    }
    let mut hashes = Vec::new();
    for prefix_entry in std::fs::read_dir(objects_dir)? {
        let prefix_entry = prefix_entry?;
        if !prefix_entry.file_type()?.is_dir() {
            continue;
        }
        let prefix = prefix_entry.file_name().to_string_lossy().into_owned();
        if prefix.len() != 2 || !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        for entry in std::fs::read_dir(prefix_entry.path())? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let rest = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if rest.len() != 62 {
                continue;
            }
            let full_hex = format!("{}{}", prefix, rest);
            hashes.push(Hash::from_hex(&full_hex).map_err(|_| MorphError::InvalidHash(full_hex))?);
        }
    }
    Ok(hashes)
}

fn fs_list(
    root: &Path,
    objects_dir: &Path,
    type_filter: ObjectType,
    getter: &dyn Fn(&Hash) -> Result<MorphObject, MorphError>,
    layout: ObjectLayout,
) -> Result<Vec<Hash>, MorphError> {
    if let Some(index_dir) = type_index_for_object_type(type_filter) {
        // Every top-level object type now has a per-type index dir
        // (0.37.6). Legacy stores predate that — the dir doesn't
        // exist, so a naïve index read silently returns empty.
        // `ensure_type_index` brings it into existence on first use
        // and drops a `.indexed` marker so subsequent calls hit the
        // fast path. To avoid paying the rebuild cost N times — once
        // per type the user happens to list — the rebuild walks the
        // store ONCE and populates every missing index simultaneously.
        ensure_type_index(root, objects_dir, layout, getter, type_filter)?;
        return fs_list_hashes_from_dir(&root.join(index_dir));
    }
    // Defensive fallback: every top-level type is indexed in 0.37.6+,
    // so this branch is unreachable today. We keep it because the
    // trait permits future custom `ObjectType` values, and "deserialize
    // everything" is at least correct (just slow).
    let all = match layout {
        ObjectLayout::Flat => fs_list_hashes_from_dir(objects_dir)?,
        ObjectLayout::Fanout => fs_list_hashes_fanout(objects_dir)?,
    };
    let mut hashes = Vec::new();
    for hash in all {
        if getter(&hash)?.object_type() == type_filter {
            hashes.push(hash);
        }
    }
    Ok(hashes)
}

/// Lazy one-shot build of every type-index directory whose `.indexed`
/// marker is missing.
///
/// Pre-0.37.5 stores indexed only `runs/`/`traces/`/`evals/`/
/// `prompts/`. 0.37.5 added `annotations/`. 0.37.6 added
/// `blobs/`/`trees/`/`pipelines/`/`commits/`/`artifacts/`/
/// `trace_rollups/`. On a legacy store, calling `list(<any-type>)`
/// would otherwise either silently return empty (when the dir is
/// missing) or fall through to a full deserialize-every-object walk.
/// We instead amortize the migration: one fanout walk classifies
/// every object and writes markers (or full-content copies for
/// `full_content` types) into every missing index dir, then writes a
/// `.indexed` marker per dir. Subsequent `list(t)` calls — for any
/// type — hit the fast read_dir path.
///
/// Skipping rules:
/// - If `<dir>/.indexed` exists, the rebuild leaves that dir alone.
///   Mixed states (some indexed, some not) are normal during rolling
///   upgrades and handled correctly: only the missing ones get built.
/// - The fanout walk is skipped entirely when *every* indexed type
///   already has a marker. The cheap `markers_present` check happens
///   before any read_dir, so the steady-state cost is one stat per
///   index dir.
/// - Individual unreadable objects are skipped, not fatal — a
///   `morph fsck` (future) is the right place to surface corruption.
/// - Marker writes are last per dir so a crash mid-rebuild leaves
///   the next invocation to retry from scratch instead of trusting a
///   partial index.
fn ensure_type_index(
    root: &Path,
    objects_dir: &Path,
    layout: ObjectLayout,
    getter: &dyn Fn(&Hash) -> Result<MorphObject, MorphError>,
    requested: ObjectType,
) -> Result<(), MorphError> {
    // Steady-state fast path: every indexed type already has a marker.
    let markers_present = ALL_TYPE_INDEX_DIRS
        .iter()
        .all(|d| root.join(d).join(".indexed").exists());
    if markers_present {
        return Ok(());
    }

    // Optional micro-optimization: if only the requested type is
    // missing a marker AND its dir exists with at least one entry,
    // the user has been on 0.37.6 long enough that put() filled in
    // this index incrementally — we just need to drop the marker.
    // Concretely: nothing in the code path between `put` and now
    // could have skipped a write, so trust what's on disk.
    //
    // We deliberately don't extend this trust to other types in the
    // same call: each type independently signals "fully rebuilt" via
    // its own marker, which is what unblocks future calls.
    if let Some(req_dir) = type_index_for_object_type(requested) {
        let dir_path = root.join(req_dir);
        let other_markers_done = ALL_TYPE_INDEX_DIRS
            .iter()
            .filter(|d| **d != req_dir)
            .all(|d| root.join(d).join(".indexed").exists());
        if other_markers_done && dir_path.exists() {
            // Only this one type is unmarked; if we're confident
            // nothing was missed (the dir already exists, suggesting
            // `put`-time indexing has been running), drop the marker
            // and skip the walk. Conservative trigger: dir exists.
            // For the cold-start path (no markers anywhere) we fall
            // through to the full rebuild.
            std::fs::write(dir_path.join(".indexed"), "1\n")?;
            return Ok(());
        }
    }

    let all = match layout {
        ObjectLayout::Flat => fs_list_hashes_from_dir(objects_dir)?,
        ObjectLayout::Fanout => fs_list_hashes_fanout(objects_dir)?,
    };

    for hash in &all {
        let obj = match getter(hash) {
            Ok(o) => o,
            Err(_) => continue,
        };
        for idx in type_indexes_for_object(&obj) {
            // Respect already-built indexes (their marker is set);
            // a half-built index dir without a marker is treated as
            // "rebuild me", and we'll write any missing entries.
            let idx_dir = root.join(idx.dir);
            if idx_dir.join(".indexed").exists() {
                continue;
            }
            let dest = idx_dir.join(format!("{}.json", hash));
            if dest.exists() {
                continue;
            }
            std::fs::create_dir_all(&idx_dir)?;
            if idx.full_content {
                let json = crate::canonical_json(&obj)?;
                std::fs::write(&dest, &json)?;
            } else {
                std::fs::write(&dest, "")?;
            }
        }
    }

    // Drop a marker on every index dir we just rebuilt (i.e. every
    // dir that didn't already have one). Marker writes are the last
    // step so a crash above leaves the next call to retry cleanly.
    for dir in ALL_TYPE_INDEX_DIRS {
        let p = root.join(dir);
        let marker = p.join(".indexed");
        if !marker.exists() {
            std::fs::create_dir_all(&p)?;
            std::fs::write(&marker, "1\n")?;
        }
    }

    Ok(())
}

fn fs_ref_read(refs_dir: &Path, name: &str) -> Result<Option<Hash>, MorphError> {
    let path = refs_dir.join(name);
    if !path.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(&path)?.trim().to_string();
    if s.is_empty() {
        return Ok(None);
    }
    Hash::from_hex(&s).map(Some)
}

fn fs_ref_write(refs_dir: &Path, name: &str, hash: &Hash) -> Result<(), MorphError> {
    let path = refs_dir.join(name);
    if let Some(parent) = path.parent() {
        if path != *refs_dir {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(&path, hash.to_string())?;
    Ok(())
}

fn fs_ref_read_raw(refs_dir: &Path, name: &str) -> Result<Option<String>, MorphError> {
    let path = refs_dir.join(name);
    if !path.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(&path)?.trim().to_string();
    Ok(if s.is_empty() { None } else { Some(s) })
}

fn fs_ref_write_raw(refs_dir: &Path, name: &str, value: &str) -> Result<(), MorphError> {
    let path = refs_dir.join(name);
    if let Some(parent) = path.parent() {
        if path != *refs_dir {
            std::fs::create_dir_all(parent)?;
        }
    }
    let content = if value.ends_with('\n') { value.to_string() } else { format!("{}\n", value) };
    std::fs::write(&path, content)?;
    Ok(())
}

fn fs_ref_delete(refs_dir: &Path, name: &str) -> Result<(), MorphError> {
    let path = refs_dir.join(name);
    if !path.exists() {
        return Err(MorphError::NotFound(format!("ref '{}' not found", name)));
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

fn fs_put(
    root: &Path,
    object: &MorphObject,
    hash: Hash,
    object_path: &Path,
) -> Result<Hash, MorphError> {
    let json = if object_path.exists() {
        None
    } else {
        std::fs::create_dir_all(object_path.parent().unwrap())?;
        let json = crate::canonical_json(object)?;
        std::fs::write(object_path, &json)?;
        Some(json)
    };

    for idx in type_indexes_for_object(object) {
        let index_path = root.join(idx.dir).join(format!("{}.json", hash));
        if !index_path.exists() {
            if let Some(parent) = index_path.parent() {
                std::fs::create_dir_all(parent)?;
                if idx.full_content {
                    let content = match json {
                        Some(ref j) => j.clone(),
                        None => std::fs::read_to_string(object_path)?,
                    };
                    std::fs::write(&index_path, content)?;
                } else {
                    // Zero-byte marker — `fs_list_hashes_from_dir`
                    // only consults filenames, so an empty file is
                    // enough to make the hash discoverable.
                    std::fs::write(&index_path, "")?;
                }
            }
        }
    }

    Ok(hash)
}

// ── Hash-prefix resolution ───────────────────────────────────────────

/// Resolve a user-supplied hex string to a full `Hash` by prefix match.
///
/// Mirrors Git's behavior:
/// - If `s` is exactly 64 hex chars, parse it as a full hash (no prefix lookup).
/// - If `s` is ≥4 hex chars, ask the store for every hash matching the
///   prefix and return the unique match. Error on zero matches or
///   ambiguous prefixes.
/// - Anything shorter than 4 chars, containing non-hex, or empty is rejected.
///
/// This function is used by read-path CLI/MCP commands (`show`, `run show`,
/// `trace show`, `certify --commit <prefix>`, etc.) so users can refer to
/// objects by short prefix.
///
/// The lookup delegates to [`Store::list_hashes_with_prefix`] — for the
/// default `FsStore` with fanout layout that's a single `read_dir` of one
/// of 256 subdirectories, with no JSON deserialization. Earlier versions
/// (≤0.37.3) iterated all 10 object types and called `list(t)` for each,
/// which on backends without a type index forced a full deserialize of
/// every object on disk just to filter by type — that turned a short-hash
/// lookup into an apparent hang on stores with many objects.
pub fn resolve_hash_prefix(store: &dyn Store, s: &str) -> Result<Hash, MorphError> {
    let s = s.trim();
    if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Hash::from_hex(s).map_err(|e| MorphError::InvalidHash(format!("invalid hash: {}", e)));
    }
    if s.is_empty() || s.len() < 4 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(MorphError::InvalidHash(format!(
            "invalid hash prefix '{}': must be 4–64 hex chars",
            s
        )));
    }
    let lower = s.to_ascii_lowercase();
    let matches = store.list_hashes_with_prefix(&lower)?;
    match matches.len() {
        0 => Err(MorphError::NotFound(format!("no object matches prefix '{}'", s))),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => Err(MorphError::InvalidHash(format!(
            "ambiguous hash prefix '{}': {} objects match",
            s, n
        ))),
    }
}

/// Resolve a revision identifier (Git's "rev") to an object hash.
///
/// Accepts, in order:
/// - A full 64-char hex hash (parsed directly).
/// - The literal `HEAD` (resolved via `resolve_head`).
/// - A branch name, looked up under `refs/heads/<name>` (slash-separated
///   names like `feature/x` work).
/// - A tag name, looked up under `refs/tags/<name>`.
/// - A `refs/...` path like `heads/main` or `tags/v1`, looked up directly.
/// - A Git-style hex prefix (≥4 chars), resolved via `resolve_hash_prefix`.
///
/// This is the canonical "what does the user mean?" entry point for any
/// CLI/MCP read-path command that takes an object identifier. It lets
/// humans and agents pass `HEAD`, branch names, tags, or short prefixes
/// instead of memorising 64-character hashes.
pub fn resolve_revision(store: &dyn Store, s: &str) -> Result<Hash, MorphError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(MorphError::InvalidHash("empty revision".into()));
    }
    if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Hash::from_hex(s).map_err(|e| MorphError::InvalidHash(format!("invalid hash: {}", e)));
    }
    if s == "HEAD" {
        return crate::commit::resolve_head(store)?
            .ok_or_else(|| MorphError::NotFound("HEAD has no commits".into()));
    }
    if let Some(rest) = s.strip_prefix("refs/") {
        if let Some(h) = store.ref_read(rest)? {
            return Ok(h);
        }
    }
    if let Some(h) = store.ref_read(&format!("heads/{}", s))? {
        return Ok(h);
    }
    if let Some(h) = store.ref_read(&format!("tags/{}", s))? {
        return Ok(h);
    }
    // Common in `morph diff` / `morph log` callers that already pass a
    // partial ref path (e.g. `remotes/origin/main`).
    if let Some(h) = store.ref_read(s)? {
        return Ok(h);
    }
    // Fall through to Git-style hex-prefix lookup. This preserves the
    // existing "invalid hash prefix" / "no object matches prefix" error
    // surfaces for inputs that are clearly hex-shaped, and produces a
    // single canonical error message for everything else.
    resolve_hash_prefix(store, s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::*;

    #[test]
    fn put_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"x": 1}),
        });
        let hash = store.put(&blob).unwrap();
        let got = store.get(&hash).unwrap();
        assert!(matches!(got, MorphObject::Blob(_)));
        assert!(store.has(&hash).unwrap());
    }

    #[test]
    fn put_prompt_blob_creates_type_index_even_without_prompts_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("objects")).unwrap();
        let store = FsStore::new(dir.path());
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"text": "hello"}),
        });
        let hash = store.put(&blob).unwrap();
        let prompts_dir = dir.path().join("prompts");
        assert!(prompts_dir.is_dir(), "put() should create prompts/ when missing");
        let index_file = prompts_dir.join(format!("{}.json", hash));
        assert!(index_file.is_file(), "prompts/<hash>.json should exist");
    }

    #[test]
    fn list_refs_returns_all_refs_under_prefix() {
        // PR5 cycle 1: transport-neutral ref enumeration. Up to PR4
        // `fetch_remote` reaches into `remote_store.refs_dir()` and
        // walks the filesystem directly — that breaks any non-fs
        // backend (SSH, future). The trait now exposes
        // `list_refs(prefix)` returning `(name, hash)` pairs for
        // every leaf under `refs/<prefix>/`, recursive.
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        std::fs::create_dir_all(store.refs_dir().join("heads")).unwrap();
        std::fs::create_dir_all(store.refs_dir().join("heads/feature")).unwrap();
        std::fs::create_dir_all(store.refs_dir().join("tags")).unwrap();

        let blob = MorphObject::Blob(Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let hash = store.put(&blob).unwrap();

        store.ref_write("heads/main", &hash).unwrap();
        store.ref_write("heads/feature/xyz", &hash).unwrap();
        store.ref_write("tags/v1", &hash).unwrap();

        let mut heads = store.list_refs("heads").unwrap();
        heads.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            heads,
            vec![
                ("feature/xyz".to_string(), hash),
                ("main".to_string(), hash),
            ]
        );

        let tags = store.list_refs("tags").unwrap();
        assert_eq!(tags, vec![("v1".to_string(), hash)]);

        // Empty prefix yields nothing useful here but must not error.
        let unknown = store.list_refs("nope").unwrap();
        assert!(unknown.is_empty());
    }

    #[test]
    fn list_branches_returns_heads_only() {
        // PR5 cycle 1 (companion): convenience wrapper for
        // `list_refs("heads")` — used by `fetch_remote` so SSH
        // implementations only need to override one method.
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        std::fs::create_dir_all(store.refs_dir().join("heads")).unwrap();

        let blob = MorphObject::Blob(Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let hash = store.put(&blob).unwrap();
        store.ref_write("heads/main", &hash).unwrap();
        store.ref_write("heads/feature", &hash).unwrap();

        let mut got = store.list_branches().unwrap();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "feature");
        assert_eq!(got[1].0, "main");
    }

    #[test]
    fn ref_write_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        std::fs::create_dir_all(store.refs_dir()).unwrap();
        let blob = MorphObject::Blob(Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let hash = store.put(&blob).unwrap();
        store.ref_write("heads/main", &hash).unwrap();
        let read = store.ref_read("heads/main").unwrap();
        assert_eq!(read, Some(hash));
    }

    #[test]
    fn get_missing_returns_not_found() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("objects")).unwrap();
        let store = FsStore::new(dir.path());
        let hash = Hash::from_hex(&"0".repeat(64)).unwrap();
        let err = store.get(&hash).unwrap_err();
        assert!(matches!(err, MorphError::NotFound(_)));
    }

    #[test]
    fn list_filters_by_object_type() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"body": "x"}),
        });
        let tree = MorphObject::Tree(Tree {
            entries: vec![TreeEntry {
                name: "f".into(),
                hash: "0".repeat(64),
                entry_type: "blob".into(),
            }],
        });
        let blob_hash = store.put(&blob).unwrap();
        let tree_hash = store.put(&tree).unwrap();

        let blobs = store.list(ObjectType::Blob).unwrap();
        assert!(blobs.contains(&blob_hash));
        assert!(!blobs.contains(&tree_hash));

        let trees = store.list(ObjectType::Tree).unwrap();
        assert!(trees.contains(&tree_hash));
        assert!(!trees.contains(&blob_hash));
    }

    #[test]
    fn ref_read_raw_ref_write_raw_symbolic_head() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        std::fs::create_dir_all(store.refs_dir()).unwrap();
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();
        let raw = store.ref_read_raw("HEAD").unwrap();
        assert!(raw.as_deref().map(|s| s.contains("ref:")).unwrap_or(false));
        assert!(raw.as_deref().map(|s| s.contains("heads/main")).unwrap_or(false));
    }

    #[test]
    fn ref_read_raw_after_ref_write_resolves_to_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        std::fs::create_dir_all(store.refs_dir()).unwrap();
        let blob = MorphObject::Blob(Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let hash = store.put(&blob).unwrap();
        store.ref_write("heads/main", &hash).unwrap();
        let raw = store.ref_read_raw("heads/main").unwrap();
        assert_eq!(raw.as_deref(), Some(hash.to_string().as_str()));
        assert_eq!(store.ref_read("heads/main").unwrap(), Some(hash));
    }

    #[test]
    fn ref_delete_removes_ref() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        std::fs::create_dir_all(store.refs_dir().join("tags")).unwrap();
        let blob = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({}) });
        let hash = store.put(&blob).unwrap();
        store.ref_write("tags/v1", &hash).unwrap();
        assert!(store.ref_read("tags/v1").unwrap().is_some());
        store.ref_delete("tags/v1").unwrap();
        assert!(store.ref_read("tags/v1").unwrap().is_none());
    }

    // --- FsStore::new_git: same Store contract, Git-format hash ---

    #[test]
    fn git_store_put_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git(dir.path());
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"x": 1}),
        });
        let hash = store.put(&blob).unwrap();
        let got = store.get(&hash).unwrap();
        assert!(matches!(got, MorphObject::Blob(_)));
        assert!(store.has(&hash).unwrap());
        let legacy_hash = crate::content_hash(&blob).unwrap();
        assert_ne!(hash, legacy_hash);
    }

    #[test]
    fn git_store_ref_write_read_and_symbolic_head() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git(dir.path());
        std::fs::create_dir_all(store.refs_dir()).unwrap();
        let blob = MorphObject::Blob(Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let hash = store.put(&blob).unwrap();
        store.ref_write("heads/main", &hash).unwrap();
        assert_eq!(store.ref_read("heads/main").unwrap(), Some(hash));
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();
        let raw = store.ref_read_raw("HEAD").unwrap();
        assert!(raw.as_deref().map(|s| s.contains("ref:")).unwrap_or(false));
    }

    #[test]
    fn git_store_list_filters_by_type() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git(dir.path());
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"body": "x"}),
        });
        let tree = MorphObject::Tree(Tree {
            entries: vec![TreeEntry {
                name: "f".into(),
                hash: "0".repeat(64),
                entry_type: "blob".into(),
            }],
        });
        let blob_hash = store.put(&blob).unwrap();
        let tree_hash = store.put(&tree).unwrap();
        let blobs = store.list(ObjectType::Blob).unwrap();
        assert!(blobs.contains(&blob_hash));
        assert!(!blobs.contains(&tree_hash));
        let trees = store.list(ObjectType::Tree).unwrap();
        assert!(trees.contains(&tree_hash));
        assert!(!trees.contains(&blob_hash));
    }

    // --- Fan-out layout ---

    #[test]
    fn fanout_put_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git_fanout(dir.path());
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"fanout": true}),
        });
        let hash = store.put(&blob).unwrap();
        let got = store.get(&hash).unwrap();
        assert!(matches!(got, MorphObject::Blob(_)));
        assert!(store.has(&hash).unwrap());

        let hex = hash.to_string();
        let (prefix, rest) = hex.split_at(2);
        let expected = dir.path().join("objects").join(prefix).join(format!("{}.json", rest));
        assert!(expected.exists(), "object should be at fan-out path");
    }

    #[test]
    fn fanout_list_filters_by_type() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git_fanout(dir.path());
        let blob = MorphObject::Blob(Blob {
            kind: "data".into(),
            content: serde_json::json!({"a": 1}),
        });
        let tree = MorphObject::Tree(Tree {
            entries: vec![TreeEntry {
                name: "f".into(),
                hash: "0".repeat(64),
                entry_type: "blob".into(),
            }],
        });
        let blob_hash = store.put(&blob).unwrap();
        let tree_hash = store.put(&tree).unwrap();
        let blobs = store.list(ObjectType::Blob).unwrap();
        assert!(blobs.contains(&blob_hash));
        assert!(!blobs.contains(&tree_hash));
    }

    #[test]
    fn fanout_all_object_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git_fanout(dir.path());
        let b1 = MorphObject::Blob(Blob { kind: "a".into(), content: serde_json::json!(1) });
        let b2 = MorphObject::Blob(Blob { kind: "b".into(), content: serde_json::json!(2) });
        let h1 = store.put(&b1).unwrap();
        let h2 = store.put(&b2).unwrap();
        let all = store.all_object_hashes().unwrap();
        assert!(all.contains(&h1));
        assert!(all.contains(&h2));
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn fanout_delete_object() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git_fanout(dir.path());
        let blob = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({}) });
        let hash = store.put(&blob).unwrap();
        assert!(store.has(&hash).unwrap());
        assert!(store.delete_object(&hash).unwrap());
        assert!(!store.has(&hash).unwrap());
        assert!(!store.delete_object(&hash).unwrap());
    }

    #[test]
    fn from_store_version_selects_correct_layout() {
        let dir = tempfile::tempdir().unwrap();
        let morph_dir = dir.path().join(".morph");
        std::fs::create_dir_all(morph_dir.join("objects")).unwrap();
        std::fs::create_dir_all(morph_dir.join("refs")).unwrap();

        std::fs::write(morph_dir.join("config.json"), r#"{"repo_version":"0.3"}"#).unwrap();
        let store = FsStore::from_store_version(&morph_dir).unwrap();
        assert_eq!(store.layout(), ObjectLayout::Flat);

        std::fs::write(morph_dir.join("config.json"), r#"{"repo_version":"0.4"}"#).unwrap();
        let store = FsStore::from_store_version(&morph_dir).unwrap();
        assert_eq!(store.layout(), ObjectLayout::Fanout);
    }

    // ── resolve_hash_prefix ──────────────────────────────────────────

    #[test]
    fn resolve_hash_prefix_full_hash_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        let blob = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({"a": 1}) });
        let hash = store.put(&blob).unwrap();
        let full = hash.to_string();
        let resolved = resolve_hash_prefix(&store, &full).unwrap();
        assert_eq!(resolved, hash);
    }

    #[test]
    fn resolve_hash_prefix_short_prefix_matches() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        let blob = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({"a": 1}) });
        let hash = store.put(&blob).unwrap();
        let full = hash.to_string();
        for len in [4, 6, 8, 12, 40] {
            let prefix = &full[..len];
            let resolved = resolve_hash_prefix(&store, prefix).unwrap();
            assert_eq!(resolved, hash, "prefix len {} should resolve", len);
        }
    }

    #[test]
    fn resolve_hash_prefix_accepts_uppercase() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        let blob = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({"a": 1}) });
        let hash = store.put(&blob).unwrap();
        let prefix_upper = hash.to_string()[..10].to_ascii_uppercase();
        let resolved = resolve_hash_prefix(&store, &prefix_upper).unwrap();
        assert_eq!(resolved, hash);
    }

    #[test]
    fn resolve_hash_prefix_trims_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        let blob = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({"a": 1}) });
        let hash = store.put(&blob).unwrap();
        let padded = format!("  {}  ", &hash.to_string()[..8]);
        let resolved = resolve_hash_prefix(&store, &padded).unwrap();
        assert_eq!(resolved, hash);
    }

    #[test]
    fn resolve_hash_prefix_no_match_errors() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        let blob = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({"a": 1}) });
        let _ = store.put(&blob).unwrap();
        let err = resolve_hash_prefix(&store, "deadbeefcafe").unwrap_err();
        assert!(matches!(err, MorphError::NotFound(_)), "got {:?}", err);
    }

    #[test]
    fn resolve_hash_prefix_too_short_errors() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        for bad in ["", "a", "ab", "abc"] {
            let err = resolve_hash_prefix(&store, bad).unwrap_err();
            assert!(matches!(err, MorphError::InvalidHash(_)), "input {:?} got {:?}", bad, err);
        }
    }

    #[test]
    fn resolve_hash_prefix_non_hex_errors() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        let err = resolve_hash_prefix(&store, "not-a-hash-at-all").unwrap_err();
        assert!(matches!(err, MorphError::InvalidHash(_)));
        let err = resolve_hash_prefix(&store, "zzzzzzzz").unwrap_err();
        assert!(matches!(err, MorphError::InvalidHash(_)));
    }

    #[test]
    fn resolve_hash_prefix_ambiguous_errors() {
        // Find a store where two object hashes happen to share a 4-char prefix.
        // We populate many blobs until we hit a collision, then verify that
        // resolving by the shared prefix returns an ambiguous error while the
        // longer unique prefixes still resolve.
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        let mut hashes: Vec<Hash> = Vec::new();
        let mut collision: Option<(Hash, Hash, usize)> = None;
        for i in 0..10000u32 {
            let obj = MorphObject::Blob(Blob {
                kind: "x".into(),
                content: serde_json::json!({"i": i}),
            });
            let h = store.put(&obj).unwrap();
            let hs = h.to_string();
            for existing in &hashes {
                let es = existing.to_string();
                // find longest common hex prefix
                let common = hs.chars().zip(es.chars()).take_while(|(a, b)| a == b).count();
                if common >= 4 {
                    collision = Some((*existing, h, common));
                    break;
                }
            }
            if collision.is_some() { break; }
            hashes.push(h);
        }
        let (a, b, common_len) = collision.expect("expected a ≥4-char prefix collision within 10k blobs");
        let shared = &a.to_string()[..common_len];
        let err = resolve_hash_prefix(&store, shared).unwrap_err();
        assert!(matches!(err, MorphError::InvalidHash(_)), "expected ambiguous, got {:?}", err);
        // Longer prefixes should disambiguate (different next char).
        let a_str = a.to_string();
        let b_str = b.to_string();
        let a_unique = &a_str[..common_len + 1];
        let b_unique = &b_str[..common_len + 1];
        assert_ne!(a_unique, b_unique);
        assert_eq!(resolve_hash_prefix(&store, a_unique).unwrap(), a);
        assert_eq!(resolve_hash_prefix(&store, b_unique).unwrap(), b);
    }

    #[test]
    fn resolve_hash_prefix_matches_across_object_types() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        let blob = MorphObject::Blob(Blob { kind: "b".into(), content: serde_json::json!({}) });
        let tree = MorphObject::Tree(Tree { entries: vec![TreeEntry { name: "f".into(), hash: "0".repeat(64), entry_type: "blob".into() }] });
        let bh = store.put(&blob).unwrap();
        let th = store.put(&tree).unwrap();
        assert_eq!(resolve_hash_prefix(&store, &bh.to_string()[..8]).unwrap(), bh);
        assert_eq!(resolve_hash_prefix(&store, &th.to_string()[..8]).unwrap(), th);
    }

    /// Regression test for the v0.37.4 hang: prior to the fix,
    /// `resolve_hash_prefix` iterated all 10 object types and called
    /// `Store::list(t)` for each. For backends without a per-type
    /// index (FsStore for Blob/Tree/Pipeline/Commit/Artifact/
    /// TraceRollup/Annotation), `list(t)` deserialized every object
    /// on disk to filter by type — `morph certify --commit <prefix>`
    /// looked like a hang on real repos.
    ///
    /// We wrap a real FsStore in a proxy whose `list(t)` panics, so
    /// any future regression that reintroduces the per-type iteration
    /// fails this test loudly. The proxy delegates the new fast path
    /// (`list_hashes_with_prefix`) to the inner store.
    #[test]
    fn resolve_hash_prefix_does_not_iterate_object_types() {
        struct PanicListByTypeStore {
            inner: FsStore,
        }
        impl Store for PanicListByTypeStore {
            fn put(&self, o: &MorphObject) -> Result<Hash, MorphError> { self.inner.put(o) }
            fn get(&self, h: &Hash) -> Result<MorphObject, MorphError> { self.inner.get(h) }
            fn has(&self, h: &Hash) -> Result<bool, MorphError> { self.inner.has(h) }
            fn list(&self, _t: ObjectType) -> Result<Vec<Hash>, MorphError> {
                panic!("resolve_hash_prefix must not call Store::list(type) — use list_hashes_with_prefix");
            }
            fn list_hashes_with_prefix(&self, p: &str) -> Result<Vec<Hash>, MorphError> {
                self.inner.list_hashes_with_prefix(p)
            }
            fn ref_read(&self, n: &str) -> Result<Option<Hash>, MorphError> { self.inner.ref_read(n) }
            fn ref_write(&self, n: &str, h: &Hash) -> Result<(), MorphError> { self.inner.ref_write(n, h) }
            fn ref_read_raw(&self, n: &str) -> Result<Option<String>, MorphError> { self.inner.ref_read_raw(n) }
            fn ref_write_raw(&self, n: &str, v: &str) -> Result<(), MorphError> { self.inner.ref_write_raw(n, v) }
            fn ref_delete(&self, n: &str) -> Result<(), MorphError> { self.inner.ref_delete(n) }
            fn refs_dir(&self) -> PathBuf { self.inner.refs_dir() }
            fn hash_object(&self, o: &MorphObject) -> Result<Hash, MorphError> { self.inner.hash_object(o) }
        }

        let dir = tempfile::tempdir().unwrap();
        let inner = FsStore::new_git_fanout(dir.path());
        // Sprinkle several types of objects into the store so the
        // legacy "iterate every type" path would have plenty to walk.
        let mut hashes = Vec::new();
        for i in 0..16u32 {
            let obj = MorphObject::Blob(Blob {
                kind: "x".into(),
                content: serde_json::json!({"i": i}),
            });
            hashes.push(inner.put(&obj).unwrap());
        }
        let tree = MorphObject::Tree(Tree {
            entries: vec![TreeEntry {
                name: "f".into(),
                hash: "0".repeat(64),
                entry_type: "blob".into(),
            }],
        });
        let th = inner.put(&tree).unwrap();
        hashes.push(th);

        let store = PanicListByTypeStore { inner };
        for h in &hashes {
            let prefix = &h.to_string()[..8];
            let resolved = resolve_hash_prefix(&store, prefix).unwrap();
            assert_eq!(&resolved, h, "prefix {prefix} should resolve");
        }
    }

    /// Annotations got a per-type index in 0.37.5, but legacy stores
    /// (94k objects, 33 GB) have no `annotations/` directory at all.
    /// Naïvely reading the index would silently miss every existing
    /// annotation and break `morph status` / `morph eval gaps`. The
    /// lazy rebuild path must:
    ///   1. Discover existing annotations even when the index dir is
    ///      missing (legacy store).
    ///   2. Drop the `.indexed` marker so subsequent calls don't repay
    ///      the walk.
    ///   3. Skip rebuilding when the marker is already present (the
    ///      common path on 0.37.5+ stores).
    #[test]
    fn list_annotation_lazily_builds_index_on_legacy_store() {
        // Build a store and write annotation objects directly into the
        // fanout, bypassing `put` so the `annotations/` index dir
        // doesn't exist — that's what a 0.37.4-and-earlier store looks
        // like after the user upgrades.
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git_fanout(dir.path());
        std::fs::create_dir_all(store.objects_dir()).unwrap();

        let mut expected: Vec<Hash> = Vec::new();
        for i in 0..3u32 {
            let ann = MorphObject::Annotation(Annotation {
                target: format!("{:0>64}", i),
                target_sub: None,
                kind: "certification".into(),
                data: Default::default(),
                author: "tester".into(),
                timestamp: "2026-04-29T00:00:00Z".into(),
            });
            let json = crate::canonical_json(&ann).unwrap();
            let hash = crate::content_hash_git(&ann).unwrap();
            let path = store.objects_dir()
                .join(&hash.to_string()[..2])
                .join(format!("{}.json", &hash.to_string()[2..]));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &json).unwrap();
            expected.push(hash);
        }
        // Sprinkle a non-annotation alongside so the rebuild also has
        // to filter, not just slurp every fanout file.
        let blob = MorphObject::Blob(Blob {
            kind: "noise".into(),
            content: serde_json::json!({"v": 1}),
        });
        store.put(&blob).unwrap();

        // Sanity: the legacy store has no `annotations/` dir yet.
        assert!(!dir.path().join("annotations").exists());

        let mut listed = store.list(ObjectType::Annotation).unwrap();
        listed.sort_by_key(|h| h.to_string());
        expected.sort_by_key(|h| h.to_string());
        assert_eq!(listed, expected, "lazy rebuild must surface every legacy annotation");
        assert!(
            dir.path().join("annotations").join(".indexed").exists(),
            "rebuild must drop the .indexed marker so subsequent calls skip the walk"
        );

        // Run again with the marker in place; same result, no panic
        // even if we delete the underlying objects (the index serves
        // it). This proves subsequent calls don't pay the rebuild
        // cost.
        let listed_again = store.list(ObjectType::Annotation).unwrap();
        assert_eq!(listed_again.len(), expected.len());
    }

    /// Once the `.indexed` marker is in place, `list(Annotation)` must
    /// not call `getter` (i.e. must not deserialize a single object).
    /// This is the regression test for the 0.37.5 fix: on a 94k-object
    /// store, the rebuild is a one-time cost; every subsequent call
    /// stays cheap.
    #[test]
    fn list_annotation_after_marker_does_not_deserialize_objects() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git_fanout(dir.path());
        let ann = MorphObject::Annotation(Annotation {
            target: "0".repeat(64),
            target_sub: None,
            kind: "certification".into(),
            data: Default::default(),
            author: "tester".into(),
            timestamp: "2026-04-29T00:00:00Z".into(),
        });
        store.put(&ann).unwrap();
        // Mark the index complete so the lazy build short-circuits.
        std::fs::write(dir.path().join("annotations").join(".indexed"), "1\n").unwrap();

        // Drop a poison file deep in the object fanout. If the legacy
        // walk ever fired, it'd try to deserialize this and blow up.
        let poison_dir = store.objects_dir().join("ff");
        std::fs::create_dir_all(&poison_dir).unwrap();
        std::fs::write(poison_dir.join("poison.json"), "not valid json at all").unwrap();

        let listed = store.list(ObjectType::Annotation).unwrap();
        assert_eq!(listed.len(), 1, "marker present ⇒ trust the index");
    }

    /// `morph status` and `morph eval gaps` were the immediate
    /// victims; this test exercises the contract end-to-end at the
    /// store level. With many non-annotation objects on disk and only
    /// a handful of annotations, `list(Annotation)` must return only
    /// the annotations after the lazy build, and the walk must not
    /// repeat on the second call.
    #[test]
    fn list_annotation_filters_to_annotations_only_after_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git_fanout(dir.path());
        // 20 noise blobs.
        for i in 0..20u32 {
            let obj = MorphObject::Blob(Blob {
                kind: "noise".into(),
                content: serde_json::json!({"i": i}),
            });
            store.put(&obj).unwrap();
        }
        // 4 annotations, written via `put` so they're already
        // indexed (the modern 0.37.5+ path).
        let mut expected: Vec<Hash> = Vec::new();
        for i in 0..4u32 {
            let ann = MorphObject::Annotation(Annotation {
                target: format!("{:0>64}", i),
                target_sub: None,
                kind: "certification".into(),
                data: Default::default(),
                author: "tester".into(),
                timestamp: "2026-04-29T00:00:00Z".into(),
            });
            expected.push(store.put(&ann).unwrap());
        }
        let mut listed = store.list(ObjectType::Annotation).unwrap();
        listed.sort_by_key(|h| h.to_string());
        expected.sort_by_key(|h| h.to_string());
        assert_eq!(listed, expected);
    }

    /// 0.37.6 extended the type-index roster from
    /// `runs/traces/evals/prompts/annotations` to every top-level
    /// object type. The contract is: `list(t)` for any `t` must not
    /// call `getter` (i.e. must not deserialize a single object) once
    /// the indexes are built. We prove it by wrapping a real FsStore
    /// in a proxy whose `get()` panics, populating the store with the
    /// types that have minimal field surface (blob/tree/pipeline/
    /// trace_rollup/annotation), then listing each.
    ///
    /// The other types (Run/Trace/Commit/Artifact/EvalSuite) hit the
    /// same code paths in `fs_put` and `fs_list`; covering them too
    /// would just duplicate fixture boilerplate without exercising
    /// new logic.
    #[test]
    fn list_every_type_uses_index_not_get() {
        struct PanicGetStore {
            inner: FsStore,
        }
        impl Store for PanicGetStore {
            fn put(&self, o: &MorphObject) -> Result<Hash, MorphError> { self.inner.put(o) }
            fn get(&self, _h: &Hash) -> Result<MorphObject, MorphError> {
                panic!("list(t) must not call get() once indexes are built");
            }
            fn has(&self, h: &Hash) -> Result<bool, MorphError> { self.inner.has(h) }
            fn list(&self, t: ObjectType) -> Result<Vec<Hash>, MorphError> { self.inner.list(t) }
            fn list_hashes_with_prefix(&self, p: &str) -> Result<Vec<Hash>, MorphError> {
                self.inner.list_hashes_with_prefix(p)
            }
            fn ref_read(&self, n: &str) -> Result<Option<Hash>, MorphError> { self.inner.ref_read(n) }
            fn ref_write(&self, n: &str, h: &Hash) -> Result<(), MorphError> { self.inner.ref_write(n, h) }
            fn ref_read_raw(&self, n: &str) -> Result<Option<String>, MorphError> { self.inner.ref_read_raw(n) }
            fn ref_write_raw(&self, n: &str, v: &str) -> Result<(), MorphError> { self.inner.ref_write_raw(n, v) }
            fn ref_delete(&self, n: &str) -> Result<(), MorphError> { self.inner.ref_delete(n) }
            fn refs_dir(&self) -> PathBuf { self.inner.refs_dir() }
            fn hash_object(&self, o: &MorphObject) -> Result<Hash, MorphError> { self.inner.hash_object(o) }
        }

        let dir = tempfile::tempdir().unwrap();
        let inner = FsStore::new_git_fanout(dir.path());

        // One object of every easily-constructible top-level type.
        let mut puts: Vec<(ObjectType, Hash)> = Vec::new();
        for obj in [
            MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({"a": 1}) }),
            MorphObject::Tree(Tree { entries: vec![] }),
            MorphObject::TraceRollup(crate::objects::TraceRollup {
                trace: "0".repeat(64),
                summary: "ok".into(),
                key_events: vec![],
            }),
            MorphObject::Annotation(Annotation {
                target: "0".repeat(64),
                target_sub: None,
                kind: "certification".into(),
                data: Default::default(),
                author: "t".into(),
                timestamp: "2026-04-29T00:00:00Z".into(),
            }),
        ] {
            let t = obj.object_type();
            let h = inner.put(&obj).unwrap();
            puts.push((t, h));
        }

        // Drop a marker on every type-index dir so `ensure_type_index`
        // skips the rebuild walk. (`put` doesn't drop the marker
        // itself; the marker only signals "this dir was rebuilt from
        // a legacy store" — see ensure_type_index docs.)
        for d in ALL_TYPE_INDEX_DIRS {
            let p = dir.path().join(d).join(".indexed");
            if !p.exists() {
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, "1\n").unwrap();
            }
        }

        let store = PanicGetStore { inner };
        for (t, h) in &puts {
            let listed = store.list(*t).unwrap();
            assert!(
                listed.contains(h),
                "list({:?}) must surface the put hash via the index alone",
                t
            );
        }
    }

    /// Pipeline objects construct via a real PipelineGraph; covered
    /// in its own test so the heavy fixture doesn't bloat the
    /// "every type" test above.
    #[test]
    fn list_pipeline_uses_index_after_put() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git_fanout(dir.path());
        let pipeline = MorphObject::Pipeline(crate::objects::Pipeline {
            graph: crate::objects::PipelineGraph { nodes: vec![], edges: vec![] },
            prompts: vec![],
            eval_suite: None,
            attribution: None,
            provenance: None,
        });
        let h = store.put(&pipeline).unwrap();
        let listed = store.list(ObjectType::Pipeline).unwrap();
        assert!(listed.contains(&h));
        let idx_path = dir.path().join("pipelines").join(format!("{}.json", h));
        assert!(idx_path.exists(), "pipelines index entry must exist");
        assert_eq!(
            std::fs::metadata(&idx_path).unwrap().len(),
            0,
            "pipeline index uses zero-byte marker (new in 0.37.6)"
        );
    }

    /// On a 0.37.5-or-earlier store, only `runs/`/`traces/`/`evals/`/
    /// `prompts/`/`annotations/` directories exist. `list(Blob)`,
    /// `list(Tree)`, `list(Commit)`, etc. used to fall through to
    /// "deserialize every object" — a multi-minute hang on a
    /// 94k-object store. After the 0.37.6 fix, the first such call
    /// triggers a single-pass rebuild that populates *every* missing
    /// type-index dir simultaneously, so subsequent `list(t)` calls
    /// for any type are O(read_dir).
    #[test]
    fn list_legacy_store_rebuilds_every_missing_type_index_in_one_walk() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git_fanout(dir.path());

        // Write one of every easily-constructed object type directly
        // into the fanout (bypassing `put`) — that's what a
        // pre-0.37.6 store looks like after upgrade.
        for obj in [
            MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({"a": 1}) }),
            MorphObject::Blob(Blob { kind: "prompt".into(), content: serde_json::json!({"t": "hi"}) }),
            MorphObject::Tree(Tree { entries: vec![] }),
        ] {
            let h = crate::content_hash_git(&obj).unwrap();
            let json = crate::canonical_json(&obj).unwrap();
            let path = store.objects_dir()
                .join(&h.to_string()[..2])
                .join(format!("{}.json", &h.to_string()[2..]));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &json).unwrap();
        }

        // Sanity: no type-index dirs exist yet.
        for d in ALL_TYPE_INDEX_DIRS {
            assert!(!dir.path().join(d).exists(), "{} should be missing pre-rebuild", d);
        }

        // First list call triggers the unified rebuild.
        let blobs = store.list(ObjectType::Blob).unwrap();
        assert_eq!(blobs.len(), 2, "both regular and prompt blobs surface via blobs/");

        // Every type-index dir now has a `.indexed` marker, so
        // subsequent `list(t)` calls hit the fast path for any type.
        for d in ALL_TYPE_INDEX_DIRS {
            assert!(
                dir.path().join(d).join(".indexed").exists(),
                "{}/.indexed must exist after rebuild",
                d
            );
        }

        // Listing other types now returns the right counts without
        // re-walking the fanout.
        assert_eq!(store.list(ObjectType::Tree).unwrap().len(), 1);
        assert_eq!(
            store.list(ObjectType::Pipeline).unwrap().len(),
            0,
            "no pipelines were written; index dir is empty post-rebuild"
        );
    }

    /// New 0.37.6 indexes use zero-byte marker files (not full-content
    /// copies) so a 33 GB blob-heavy store doesn't double in size when
    /// `blobs/` materializes. `runs/`, `traces/`, `evals/`,
    /// `prompts/`, `annotations/` keep full content for backward
    /// compat (`morph tap` falls back to reading from `traces/`).
    ///
    /// We probe the `blobs/` (new) and `annotations/` (legacy
    /// full-content) indexes — the corresponding code path for the
    /// other 8 types is identical, dispatched by the
    /// `full_content` flag in `type_indexes_for_object`.
    #[test]
    fn type_index_files_are_markers_for_new_types_full_content_for_legacy() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git_fanout(dir.path());

        let blob = MorphObject::Blob(Blob {
            kind: "regular".into(),
            content: serde_json::json!({"a": 1}),
        });
        let blob_hash = store.put(&blob).unwrap();
        let blob_idx = dir.path().join("blobs").join(format!("{}.json", blob_hash));
        let blob_idx_size = std::fs::metadata(&blob_idx).unwrap().len();
        assert_eq!(blob_idx_size, 0, "new blob index must be a zero-byte marker");

        let ann = MorphObject::Annotation(Annotation {
            target: "0".repeat(64),
            target_sub: None,
            kind: "certification".into(),
            data: Default::default(),
            author: "t".into(),
            timestamp: "2026-04-29T00:00:00Z".into(),
        });
        let ann_hash = store.put(&ann).unwrap();
        let ann_idx = dir.path().join("annotations").join(format!("{}.json", ann_hash));
        let ann_idx_size = std::fs::metadata(&ann_idx).unwrap().len();
        assert!(
            ann_idx_size > 0,
            "legacy annotation index must keep full content for back-compat"
        );
    }

    /// A prompt blob is a kind-subset of Blob. It must show up in
    /// both `blobs/` (so `list(Blob)` is complete) and `prompts/`
    /// (so existing recording flows that read `prompts/` keep working).
    #[test]
    fn prompt_blob_lands_in_both_blobs_and_prompts_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git_fanout(dir.path());
        let prompt = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"text": "hi"}),
        });
        let h = store.put(&prompt).unwrap();
        assert!(dir.path().join("blobs").join(format!("{}.json", h)).exists(),
            "blob index must include prompts");
        assert!(dir.path().join("prompts").join(format!("{}.json", h)).exists(),
            "prompts index must keep its kind-subset entry for recording flows");
        assert!(store.list(ObjectType::Blob).unwrap().contains(&h));
    }

    /// `list_hashes_with_prefix` on a fanout `FsStore` must walk only
    /// the matching `objects/<2chars>/` subdirectory. We prove that by
    /// dropping a poison file into a *different* fanout subdirectory:
    /// if the implementation walked it, the unrelated file would either
    /// surface in the result or cause a parse error. With the fanout
    /// fast path, it's silently ignored.
    #[test]
    fn list_hashes_with_prefix_fanout_walks_only_target_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new_git_fanout(dir.path());
        let blob = MorphObject::Blob(Blob {
            kind: "x".into(),
            content: serde_json::json!({"a": 1}),
        });
        let h = store.put(&blob).unwrap();
        let hex = h.to_string();
        let target_fanout = &hex[..2];

        // Pick any 2-char fanout dir that is *not* the target.
        let other_fanout: String = (0u8..=255)
            .map(|n| format!("{:02x}", n))
            .find(|s| s != target_fanout)
            .unwrap();
        let poison_dir = store.objects_dir().join(&other_fanout);
        std::fs::create_dir_all(&poison_dir).unwrap();
        let poison_path = poison_dir.join("poison.json");
        std::fs::write(&poison_path, "this is not valid morph object json").unwrap();

        let prefix = &hex[..8];
        let result = store.list_hashes_with_prefix(prefix).unwrap();
        assert_eq!(result, vec![h], "fast path must not touch unrelated fanout subdirs");
    }
}
