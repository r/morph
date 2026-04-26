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

fn type_index_dir(object: &MorphObject) -> Option<&'static str> {
    match object {
        MorphObject::Run(_) => Some("runs"),
        MorphObject::Trace(_) => Some("traces"),
        MorphObject::EvalSuite(_) => Some("evals"),
        MorphObject::Blob(b) if b.kind == "prompt" => Some("prompts"),
        _ => None,
    }
}

fn type_index_for_object_type(t: ObjectType) -> Option<&'static str> {
    match t {
        ObjectType::Run => Some("runs"),
        ObjectType::Trace => Some("traces"),
        ObjectType::EvalSuite => Some("evals"),
        _ => None,
    }
}

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
        return fs_list_hashes_from_dir(&root.join(index_dir));
    }
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

    if let Some(dir_name) = type_index_dir(object) {
        let index_path = root.join(dir_name).join(format!("{}.json", hash));
        if !index_path.exists() {
            if let Some(parent) = index_path.parent() {
                std::fs::create_dir_all(parent)?;
                let content = match json {
                    Some(ref j) => j.clone(),
                    None => std::fs::read_to_string(object_path)?,
                };
                std::fs::write(&index_path, content)?;
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
/// - If `s` is ≥4 hex chars, scan the store across every object type and
///   return the unique match. Error on zero matches or ambiguous prefixes.
/// - Anything shorter than 4 chars, containing non-hex, or empty is rejected.
///
/// This function is used by read-path CLI/MCP commands (`show`, `run show`,
/// `trace show`, etc.) so users can refer to objects by short prefix.
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
    let mut matches: Vec<Hash> = Vec::new();
    // Scan every object type. The Store trait has no list_all, so we iterate.
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
        for h in store.list(t)? {
            if h.to_string().starts_with(&lower) && !matches.contains(&h) {
                matches.push(h);
                if matches.len() > 1 {
                    // Early exit once we know the prefix is ambiguous; we still
                    // want the final count, but two is enough to report.
                }
            }
        }
    }
    match matches.len() {
        0 => Err(MorphError::NotFound(format!("no object matches prefix '{}'", s))),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => Err(MorphError::InvalidHash(format!(
            "ambiguous hash prefix '{}': {} objects match",
            s, n
        ))),
    }
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
                    collision = Some((existing.clone(), h.clone(), common));
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
}
