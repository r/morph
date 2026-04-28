//! `SshStore` — a `Store` implementation that drives a remote
//! `morph remote-helper` over a pair of pipes.
//!
//! Architecture
//! ------------
//! ```text
//! ┌─ client process ─────────────┐        ┌─ server process ────────────┐
//! │                              │ stdin  │                             │
//! │ SshStore  ──>  Spawn (ssh /  │ ─────> │ morph remote-helper         │
//! │                local fork)   │        │   --repo-root <path>        │
//! │           <──  Response      │ stdout │   (FsStore behind the       │
//! │                              │ <───── │    scenes)                  │
//! └──────────────────────────────┘        └─────────────────────────────┘
//! ```
//!
//! Anything that can produce a (stdin, stdout) pair satisfying the
//! line-oriented JSON wire format is a `Spawn`. The two prod
//! impls land in PR5:
//!
//! - `LocalSpawn` — fork the local `morph` binary against a local
//!   repo. Used by tests today and by `morph push file:///path` once
//!   we expose it from `open_remote_store` (Stage E).
//! - `RemoteSpawn` — `ssh user@host morph remote-helper --repo-root
//!   …`. Built in Stage E.
//!
//! Concurrency
//! -----------
//! A connection is single-threaded (one in-flight request at a
//! time); we wrap it in a `Mutex` so `SshStore` is `Sync` like every
//! other `Store`.

use crate::objects::MorphObject;
use crate::ssh_proto::{self, ErrResponse, OkResponse, Request};
use crate::store::{MorphError, Store};
use crate::Hash;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

// ── Spawn abstraction ────────────────────────────────────────────────

/// Anything that knows how to start a `morph remote-helper`-shaped
/// child process. Production implementations shell out to ssh; tests
/// use `LocalSpawn` to fork the morph binary locally.
pub trait Spawn: Send + Sync {
    fn spawn(&self) -> Result<Connection, MorphError>;
}

/// One running helper process plus the pipes used to talk to it.
pub struct Connection {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Connection {
    /// Send a single request and read a single response line.
    pub fn round_trip(&mut self, req: &Request) -> Result<RawResponse, MorphError> {
        let line = serde_json::to_string(req)
            .map_err(|e| MorphError::Serialization(e.to_string()))?;
        writeln!(self.stdin, "{}", line)
            .map_err(MorphError::Io)?;
        self.stdin.flush().map_err(MorphError::Io)?;

        let mut buf = String::new();
        let n = self.stdout.read_line(&mut buf).map_err(MorphError::Io)?;
        if n == 0 {
            return Err(MorphError::Serialization(
                "remote helper closed stdout before responding".into(),
            ));
        }
        let trimmed = buf.trim();
        // Try parse as Ok first, fall back to Err. The wire's
        // `untagged` Response is fine for tests but here we want
        // explicit dispatch so the caller can react to a typed
        // MorphError.
        match serde_json::from_str::<OkResponse>(trimmed) {
            Ok(ok) if ok.ok => return Ok(RawResponse::Ok(ok)),
            _ => {}
        }
        match serde_json::from_str::<ErrResponse>(trimmed) {
            Ok(err) if !err.ok => return Ok(RawResponse::Err(err)),
            _ => {}
        }
        Err(MorphError::Serialization(format!(
            "unparseable remote response: {}",
            trimmed
        )))
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Closing stdin makes the helper exit on EOF. Best-effort:
        // if the child is already gone, ignore the error.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Raw, parsed-but-not-typed response from the helper. `OkResponse`
/// holds the bulk of the payload (objects, ref lists), so boxing
/// either variant just to balance sizes would penalize the hot path.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum RawResponse {
    Ok(OkResponse),
    Err(ErrResponse),
}

// ── URL parsing ──────────────────────────────────────────────────────

/// A parsed `ssh` remote URL. The two flavors we accept:
///
/// - `ssh://user@host[:port]/abs/path/to/repo`
/// - `user@host:relative/or/abs/path` (scp-style)
///
/// `host` is mandatory; `user` is optional and falls back to the
/// system user when ssh dials out. We deliberately keep this struct
/// minimal so adding GitHub-style `git@github.com:org/repo.git` is a
/// small addition later.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SshUrl {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
}

impl SshUrl {
    /// Parse `ssh://user@host[:port]/path` or `user@host:path`.
    /// Returns `None` for a string that is plainly a local path
    /// (so callers can fall back to the filesystem branch of
    /// `open_remote_store`).
    pub fn parse(s: &str) -> Option<Self> {
        // ssh://user@host:port/path
        if let Some(rest) = s.strip_prefix("ssh://") {
            let (host_part, path) = rest.split_once('/')?;
            let path = format!("/{}", path);
            let (user, host_port) = match host_part.split_once('@') {
                Some((u, h)) => (Some(u.to_string()), h),
                None => (None, host_part),
            };
            let (host, port) = match host_port.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
                None => (host_port.to_string(), None),
            };
            if host.is_empty() {
                return None;
            }
            return Some(SshUrl { user, host, port, path });
        }
        // scp-style: user@host:path  — distinguishable from a path
        // by an `@` BEFORE the first `:`. We refuse Windows drive
        // letters (single-letter "host" before `:`) by requiring at
        // least two characters before the colon.
        if let Some((head, path)) = s.split_once(':') {
            if head.len() < 2 || path.is_empty() {
                return None;
            }
            // Distinguish `./path:foo` (which is a local path) from
            // `host:path`: a `/` in `head` means "local path".
            if head.contains('/') {
                return None;
            }
            let (user, host) = match head.split_once('@') {
                Some((u, h)) => (Some(u.to_string()), h.to_string()),
                None => (None, head.to_string()),
            };
            if host.is_empty() {
                return None;
            }
            return Some(SshUrl {
                user,
                host,
                port: None,
                path: path.to_string(),
            });
        }
        None
    }
}

// ── RemoteSpawn ──────────────────────────────────────────────────────

/// `Spawn` impl that runs `ssh ... morph remote-helper --repo-root
/// <path>` on the other side. The `ssh` command is taken from
/// `$MORPH_SSH` if set (handy for tests that want to swap in a stub),
/// otherwise just `ssh` from `$PATH`.
pub struct RemoteSpawn {
    pub url: SshUrl,
    pub ssh_command: String,
    pub remote_morph_bin: String,
    pub extra_ssh_args: Vec<String>,
}

impl RemoteSpawn {
    pub fn new(url: SshUrl) -> Self {
        let ssh_command =
            std::env::var("MORPH_SSH").unwrap_or_else(|_| "ssh".to_string());
        let remote_morph_bin =
            std::env::var("MORPH_REMOTE_BIN").unwrap_or_else(|_| "morph".to_string());
        Self {
            url,
            ssh_command,
            remote_morph_bin,
            extra_ssh_args: Vec::new(),
        }
    }
}

impl Spawn for RemoteSpawn {
    fn spawn(&self) -> Result<Connection, MorphError> {
        let mut cmd = Command::new(&self.ssh_command);
        if let Some(p) = self.url.port {
            cmd.arg("-p").arg(p.to_string());
        }
        for arg in &self.extra_ssh_args {
            cmd.arg(arg);
        }
        let target = match &self.url.user {
            Some(u) => format!("{}@{}", u, self.url.host),
            None => self.url.host.clone(),
        };
        cmd.arg(target)
            .arg(&self.remote_morph_bin)
            .arg("remote-helper")
            .arg("--repo-root")
            .arg(&self.url.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd.spawn().map_err(|e| {
            MorphError::Serialization(format!(
                "failed to spawn ssh ({}): {} (is ssh on PATH? set MORPH_SSH to override)",
                self.ssh_command, e
            ))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| MorphError::Serialization("missing stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| MorphError::Serialization("missing stdout".into()))?;
        Ok(Connection {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }
}

// ── LocalSpawn ───────────────────────────────────────────────────────

/// Forks the local `morph` binary against a local repository path.
/// Used in tests so the SshStore code path can be exercised without
/// real SSH; `RemoteSpawn` (Stage E) replaces this with an `ssh`
/// command.
pub struct LocalSpawn {
    morph_bin: PathBuf,
    repo_root: PathBuf,
}

impl LocalSpawn {
    pub fn new(morph_bin: impl Into<PathBuf>, repo_root: impl Into<PathBuf>) -> Self {
        Self {
            morph_bin: morph_bin.into(),
            repo_root: repo_root.into(),
        }
    }
}

impl Spawn for LocalSpawn {
    fn spawn(&self) -> Result<Connection, MorphError> {
        let mut child = Command::new(&self.morph_bin)
            .arg("remote-helper")
            .arg("--repo-root")
            .arg(&self.repo_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                MorphError::Serialization(format!(
                    "failed to spawn morph remote-helper: {}",
                    e
                ))
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            MorphError::Serialization("missing stdin from helper".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            MorphError::Serialization("missing stdout from helper".into())
        })?;
        Ok(Connection {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }
}

// ── SshStore ─────────────────────────────────────────────────────────

/// `Store` impl that talks to a remote `morph remote-helper` over a
/// `Spawn`.
pub struct SshStore {
    inner: Mutex<Connection>,
}

/// PR 6 stage E: client side of the schema handshake.
///
/// Map a `hello` `OkResponse` to either Ok (compatible) or
/// `MorphError::IncompatibleRemote`. Pre-PR6 helpers don't
/// advertise `protocol_version`; we accept those silently for one
/// release of overlap. Bump `MORPH_PROTOCOL_VERSION` to tighten.
fn validate_hello(ok: &OkResponse, local_protocol: u32) -> Result<(), MorphError> {
    if let Some(remote_protocol) = ok.protocol_version {
        if remote_protocol != local_protocol {
            return Err(MorphError::IncompatibleRemote {
                remote: remote_protocol.to_string(),
                local: local_protocol.to_string(),
                reason: "protocol_version".into(),
            });
        }
    }
    Ok(())
}

impl SshStore {
    /// Open a new connection by spawning a helper. Sends an initial
    /// `hello` to verify the remote understands the protocol.
    pub fn connect(spawn: &dyn Spawn) -> Result<Self, MorphError> {
        let mut conn = spawn.spawn()?;
        match conn.round_trip(&Request::Hello)? {
            RawResponse::Ok(ok) => {
                validate_hello(&ok, ssh_proto::MORPH_PROTOCOL_VERSION)?;
                Ok(Self { inner: Mutex::new(conn) })
            }
            RawResponse::Err(e) => Err(ssh_proto::to_morph_error(&e)),
        }
    }

    fn call(&self, req: &Request) -> Result<OkResponse, MorphError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| MorphError::Serialization(format!("ssh store mutex poisoned: {}", e)))?;
        match guard.round_trip(req)? {
            RawResponse::Ok(o) => Ok(o),
            RawResponse::Err(e) => Err(ssh_proto::to_morph_error(&e)),
        }
    }
}

impl Store for SshStore {
    fn put(&self, object: &MorphObject) -> Result<Hash, MorphError> {
        let resp = self.call(&Request::Put { object: object.clone() })?;
        let hash = resp
            .hash
            .ok_or_else(|| MorphError::Serialization("put: missing hash field".into()))?;
        Hash::from_hex(&hash).map_err(|_| MorphError::InvalidHash(hash))
    }

    fn get(&self, hash: &Hash) -> Result<MorphObject, MorphError> {
        let resp = self.call(&Request::Get { hash: hash.to_string() })?;
        resp.object
            .ok_or_else(|| MorphError::Serialization("get: missing object field".into()))
    }

    fn has(&self, hash: &Hash) -> Result<bool, MorphError> {
        let resp = self.call(&Request::Has { hash: hash.to_string() })?;
        resp.has
            .ok_or_else(|| MorphError::Serialization("has: missing has field".into()))
    }

    fn list(&self, _type_filter: crate::ObjectType) -> Result<Vec<Hash>, MorphError> {
        // Object enumeration over SSH is not part of the v0 helper
        // protocol — fetches walk reachability from a known commit
        // rather than scanning the store. We surface this loudly so
        // callers can switch to the reachability path.
        Err(MorphError::Serialization(
            "list() is not implemented over SSH; use reachability traversal".into(),
        ))
    }

    fn ref_read(&self, name: &str) -> Result<Option<Hash>, MorphError> {
        let resp = self.call(&Request::RefRead { name: name.to_string() })?;
        // Flat encoding: missing ref ⇒ field is JSON null ⇒
        // deserialized as None. A `put` response always sets it.
        match resp.hash {
            Some(s) => Hash::from_hex(&s).map(Some).map_err(|_| MorphError::InvalidHash(s)),
            None => Ok(None),
        }
    }

    fn ref_write(&self, name: &str, hash: &Hash) -> Result<(), MorphError> {
        self.call(&Request::RefWrite {
            name: name.to_string(),
            hash: hash.to_string(),
        })?;
        Ok(())
    }

    fn ref_read_raw(&self, _name: &str) -> Result<Option<String>, MorphError> {
        // Symbolic refs (e.g. HEAD pointing at heads/main) aren't
        // exposed by the v0 helper. Push/fetch don't need this; if
        // an op ever does, extend the wire format with a dedicated
        // variant rather than overloading ref-read.
        Err(MorphError::Serialization(
            "ref_read_raw is not supported over SSH".into(),
        ))
    }

    fn ref_write_raw(&self, _name: &str, _value: &str) -> Result<(), MorphError> {
        Err(MorphError::Serialization(
            "ref_write_raw is not supported over SSH".into(),
        ))
    }

    fn ref_delete(&self, _name: &str) -> Result<(), MorphError> {
        Err(MorphError::Serialization(
            "ref_delete is not supported over SSH yet".into(),
        ))
    }

    fn refs_dir(&self) -> PathBuf {
        // Returning an obviously bogus path catches anyone reaching
        // into the filesystem. PR5 Stage A removed the FS walk in
        // `fetch_remote`; everything else should go through
        // `list_refs` / `list_branches`.
        PathBuf::from("/var/empty/ssh-store-no-refs-dir")
    }

    fn hash_object(&self, object: &MorphObject) -> Result<Hash, MorphError> {
        // Hashing is a pure function of the object bytes; no remote
        // round-trip required.
        crate::content_hash_git(object)
    }

    fn list_refs(&self, prefix: &str) -> Result<Vec<(String, Hash)>, MorphError> {
        let resp = self.call(&Request::ListRefs {
            prefix: prefix.to_string(),
        })?;
        let entries = resp
            .refs
            .ok_or_else(|| MorphError::Serialization("list-refs: missing refs field".into()))?;
        entries
            .into_iter()
            .map(|e| {
                Hash::from_hex(&e.hash)
                    .map(|h| (e.name, h))
                    .map_err(|_| MorphError::InvalidHash(e.hash))
            })
            .collect()
    }

    fn list_branches(&self) -> Result<Vec<(String, Hash)>, MorphError> {
        let resp = self.call(&Request::ListBranches)?;
        let entries = resp.branches.ok_or_else(|| {
            MorphError::Serialization("list-branches: missing branches field".into())
        })?;
        entries
            .into_iter()
            .map(|e| {
                Hash::from_hex(&e.hash)
                    .map(|h| (e.name, h))
                    .map_err(|_| MorphError::InvalidHash(e.hash))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    //! These tests need a built `morph` binary on disk so they can
    //! be executed via `LocalSpawn`. Cargo conveniently exposes the
    //! sibling binary path via `env!("CARGO_BIN_EXE_morph")` —
    //! except this is a library crate, so we must locate it via
    //! `target/debug/morph` relative to `CARGO_MANIFEST_DIR`. The
    //! tests skip themselves cleanly when the binary is missing
    //! (e.g. `cargo test -p morph-core` run in isolation).

    use super::*;

    fn morph_bin() -> Option<PathBuf> {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
        let workspace = std::path::Path::new(&manifest)
            .parent()
            .map(|p| p.to_path_buf())?;
        // Prefer CARGO_TARGET_DIR if set (cursor sandbox uses one),
        // otherwise default to <workspace>/target.
        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| workspace.join("target"));
        for variant in ["debug", "release"] {
            let candidate = target_dir.join(variant).join("morph");
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }

    fn setup_remote_repo() -> Option<(tempfile::TempDir, LocalSpawn)> {
        let bin = morph_bin()?;
        let dir = tempfile::tempdir().ok()?;
        crate::repo::init_repo(dir.path()).ok()?;
        let spawn = LocalSpawn::new(bin, dir.path());
        Some((dir, spawn))
    }

    // ── URL parsing (no spawn needed) ────────────────────────────

    #[test]
    fn ssh_url_parses_ssh_scheme() {
        // PR5 cycle 23.
        let u = SshUrl::parse("ssh://alice@example.com/srv/morph/repo").unwrap();
        assert_eq!(u.user.as_deref(), Some("alice"));
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, None);
        assert_eq!(u.path, "/srv/morph/repo");
    }

    #[test]
    fn ssh_url_parses_ssh_scheme_with_port() {
        let u = SshUrl::parse("ssh://example.com:2222/repo").unwrap();
        assert_eq!(u.user, None);
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, Some(2222));
        assert_eq!(u.path, "/repo");
    }

    #[test]
    fn ssh_url_parses_scp_style() {
        let u = SshUrl::parse("alice@host.example:morph/repo").unwrap();
        assert_eq!(u.user.as_deref(), Some("alice"));
        assert_eq!(u.host, "host.example");
        assert_eq!(u.path, "morph/repo");
    }

    #[test]
    fn ssh_url_rejects_local_paths() {
        // The disambiguation rule: a `/` before the first `:`
        // indicates a path. This covers absolute paths
        // (`/srv/morph`), relative paths, and Windows-style drive
        // strings won't reach this code yet.
        assert!(SshUrl::parse("/srv/morph/repo").is_none());
        assert!(SshUrl::parse("./relative/repo").is_none());
        assert!(SshUrl::parse("relative/repo").is_none());
        assert!(SshUrl::parse("path/with:colon").is_none());
    }

    // ── Schema handshake (PR 6 stage E, cycle 22) ────────────────

    #[test]
    fn validate_hello_accepts_matching_protocol() {
        // PR 6 stage E cycle 22 RED→GREEN: matching protocol passes
        // through unchanged.
        let ok = ssh_proto::hello_ok("0.11.0", ssh_proto::MORPH_PROTOCOL_VERSION, "0.5");
        validate_hello(&ok, ssh_proto::MORPH_PROTOCOL_VERSION)
            .expect("matching protocol should pass");
    }

    #[test]
    fn validate_hello_rejects_newer_protocol_with_typed_error() {
        // The remote speaks protocol 2; we still understand only 1.
        // This must surface as IncompatibleRemote so the CLI can
        // suggest a local upgrade.
        let ok = ssh_proto::hello_ok("99.0.0", 2, "0.5");
        let err = validate_hello(&ok, 1).expect_err("expected IncompatibleRemote");
        match err {
            MorphError::IncompatibleRemote { remote, local, reason } => {
                assert_eq!(remote, "2");
                assert_eq!(local, "1");
                assert_eq!(reason, "protocol_version");
            }
            other => panic!("expected IncompatibleRemote, got: {:?}", other),
        }
    }

    #[test]
    fn validate_hello_rejects_older_protocol_too() {
        // Symmetric case: a server from the future ran into us and
        // we (newer client) refuse rather than silently send wrong
        // shapes. Either side getting `IncompatibleRemote` is the
        // right outcome.
        let ok = ssh_proto::hello_ok("0.11.0", 1, "0.5");
        let err = validate_hello(&ok, 2).expect_err("expected IncompatibleRemote");
        assert!(matches!(err, MorphError::IncompatibleRemote { .. }));
    }

    #[test]
    fn validate_hello_accepts_legacy_helpers_silently() {
        // Pre-PR6 helpers don't advertise a protocol_version. We
        // still accept them to give the ecosystem a release of
        // overlap. Bump MORPH_PROTOCOL_VERSION from 1→2 to tighten.
        let mut ok = ssh_proto::hello_ok("0.10.0", 1, "0.5");
        ok.protocol_version = None;
        validate_hello(&ok, 1).expect("legacy hello should pass");
    }

    // ── Spawn-driven tests (need built binary) ───────────────────

    #[test]
    fn ssh_store_connects_via_local_spawn() {
        // PR5 cycle 15 RED→GREEN: verify the connection plumbing
        // works end-to-end (spawn helper, send hello, parse
        // response).
        let Some((_dir, spawn)) = setup_remote_repo() else {
            eprintln!("skipping: morph binary not built");
            return;
        };
        let _store = SshStore::connect(&spawn).expect("connect");
    }

    #[test]
    fn ssh_store_has_returns_false_for_missing() {
        // PR5 cycle 16 RED→GREEN.
        let Some((_dir, spawn)) = setup_remote_repo() else { return };
        let store = SshStore::connect(&spawn).unwrap();
        let zeros = Hash::from_hex(&"0".repeat(64)).unwrap();
        assert!(!store.has(&zeros).unwrap());
    }

    #[test]
    fn ssh_ref_write_rejects_missing_closure() {
        // PR 6 stage F cycle 25 RED→GREEN: the server side of `push`
        // must refuse to record a ref pointing at an object the
        // helper doesn't have. Without this, a bare repo could end
        // up with a `heads/main` whose closure was never uploaded.
        let Some((_dir, spawn)) = setup_remote_repo() else { return };
        let store = SshStore::connect(&spawn).unwrap();
        let bogus = Hash::from_hex(&"a".repeat(64)).unwrap();
        let err = store
            .ref_write("heads/main", &bogus)
            .expect_err("ref_write to missing object should fail");
        assert!(
            matches!(err, MorphError::NotFound(_)),
            "expected NotFound, got: {:?}",
            err
        );
    }

    #[test]
    fn ssh_ref_write_rejects_partial_closure() {
        // The mid-flight crash case: client uploaded the commit but
        // not all of its dependencies, then issued ref-write. Server
        // walks the closure and refuses.
        let Some(bin) = morph_bin() else { return };
        let dir = tempfile::tempdir().unwrap();
        crate::repo::init_repo(dir.path()).unwrap();
        let local_store =
            crate::repo::open_store(&dir.path().join(".morph")).unwrap();

        // Build a commit locally — its closure includes a tree, a
        // pipeline, an eval suite, and one blob.
        let blob = MorphObject::Blob(crate::objects::Blob {
            kind: "data".into(),
            content: serde_json::json!({"v": 1}),
        });
        let blob_h = local_store.put(&blob).unwrap();

        let tree = MorphObject::Tree(crate::objects::Tree {
            entries: vec![crate::objects::TreeEntry {
                name: "x".into(),
                hash: blob_h.to_string(),
                entry_type: "blob".into(),
            }],
        });
        let tree_h = local_store.put(&tree).unwrap();

        let suite = MorphObject::EvalSuite(crate::objects::EvalSuite {
            cases: vec![],
            metrics: vec![],
        });
        let suite_h = local_store.put(&suite).unwrap();

        let pipe = MorphObject::Pipeline(crate::objects::Pipeline {
            graph: crate::objects::PipelineGraph {
                nodes: vec![],
                edges: vec![],
            },
            prompts: vec![],
            eval_suite: Some(suite_h.to_string()),
            attribution: None,
            provenance: None,
        });
        let pipe_h = local_store.put(&pipe).unwrap();

        let commit = MorphObject::Commit(crate::objects::Commit {
            parents: vec![],
            tree: Some(tree_h.to_string()),
            pipeline: pipe_h.to_string(),
            eval_contract: crate::objects::EvalContract {
                suite: suite_h.to_string(),
                observed_metrics: Default::default(),
            },
            message: "hello".into(),
            timestamp: "2026-04-26T00:00:00Z".into(),
            author: "morph".into(),
            contributors: None,
            env_constraints: None,
            evidence_refs: None,
            morph_version: None,
            morph_instance: None,
            morph_origin: None,
            git_origin_sha: None,
            human_edits: None,
        });
        let commit_h = local_store.put(&commit).unwrap();

        // Spin up a fresh remote bare repo and connect.
        let remote_dir = tempfile::tempdir().unwrap();
        crate::repo::init_repo(remote_dir.path()).unwrap();
        let spawn = LocalSpawn::new(bin, remote_dir.path());
        let remote = SshStore::connect(&spawn).unwrap();

        // Upload only the commit, intentionally leaving the tree
        // (and therefore the blob) absent on the remote.
        let _ = remote.put(&commit).unwrap();

        // The ref-write must be rejected because the tree is
        // missing on the server side.
        let err = remote
            .ref_write("heads/main", &commit_h)
            .expect_err("partial closure must be rejected");
        assert!(
            matches!(err, MorphError::NotFound(_)),
            "expected NotFound, got: {:?}",
            err
        );
    }

    #[test]
    fn ssh_ref_write_rejects_failed_push_gate() {
        // PR 6 stage F cycle 28 RED→GREEN: when the bare/working
        // repo configures `push_gated_branches: ["main"]` with a
        // required metric, a ref-write of an uncertified commit
        // must be refused by the helper before reaching the ref
        // store. The error should travel back to the client as a
        // clear "push gate failed" message.
        let Some(bin) = morph_bin() else { return };
        let dir = tempfile::tempdir().unwrap();
        crate::repo::init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");

        // Configure the gate before connecting so the running
        // helper sees it on every RefWrite.
        let policy = crate::policy::RepoPolicy {
            required_metrics: vec!["acc".into()],
            push_gated_branches: vec!["main".into()],
            ..Default::default()
        };
        crate::policy::write_policy(&morph_dir, &policy).unwrap();

        // Build a commit locally that lacks the required metric
        // and is not certified — this is what gate_check will
        // refuse.
        let local_store = crate::repo::open_store(&morph_dir).unwrap();
        std::fs::write(dir.path().join("f.txt"), "data").unwrap();
        crate::add_paths(local_store.as_ref(), dir.path(), &[std::path::PathBuf::from(".")])
            .unwrap();
        let commit_h = crate::create_tree_commit(
            local_store.as_ref(),
            dir.path(),
            None,
            None,
            std::collections::BTreeMap::new(),
            "no metrics".into(),
            None,
            Some("0.3"),
        )
        .unwrap();
        drop(local_store);

        let spawn = LocalSpawn::new(bin, dir.path());
        let store = SshStore::connect(&spawn).unwrap();

        let err = store
            .ref_write("heads/main", &commit_h)
            .expect_err("push gate should reject");
        let msg = format!("{}", err);
        assert!(msg.contains("push gate"), "got: {}", msg);
        assert!(msg.contains("main"), "got: {}", msg);

        // Pushing to a non-gated branch must still succeed,
        // proving the gate is scoped correctly.
        store
            .ref_write("heads/feature", &commit_h)
            .expect("non-gated branch should accept the same commit");
    }

    #[test]
    fn ssh_store_ref_round_trip() {
        // PR5 cycle 17/18 RED→GREEN.
        let Some((_dir, spawn)) = setup_remote_repo() else { return };
        let store = SshStore::connect(&spawn).unwrap();

        let blob = MorphObject::Blob(crate::objects::Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let h = store.put(&blob).unwrap();

        // ref_read on missing -> None
        assert!(store.ref_read("heads/missing").unwrap().is_none());

        store.ref_write("heads/main", &h).unwrap();
        assert_eq!(store.ref_read("heads/main").unwrap(), Some(h));
    }

    #[test]
    fn ssh_store_list_branches_and_refs() {
        // PR5 cycle 19/20 RED→GREEN.
        let Some((_dir, spawn)) = setup_remote_repo() else { return };
        let store = SshStore::connect(&spawn).unwrap();

        let blob = MorphObject::Blob(crate::objects::Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let h = store.put(&blob).unwrap();
        store.ref_write("heads/main", &h).unwrap();
        store.ref_write("heads/feature", &h).unwrap();

        let mut branches = store.list_branches().unwrap();
        branches.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].0, "feature");
        assert_eq!(branches[1].0, "main");

        let refs = store.list_refs("heads").unwrap();
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn ssh_store_get_round_trips_object() {
        // PR5 cycle 21 RED→GREEN.
        let Some((_dir, spawn)) = setup_remote_repo() else { return };
        let store = SshStore::connect(&spawn).unwrap();

        let blob = MorphObject::Blob(crate::objects::Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"text": "hello"}),
        });
        let h = store.put(&blob).unwrap();
        let got = store.get(&h).unwrap();
        match got {
            MorphObject::Blob(b) => {
                assert_eq!(b.kind, "prompt");
                assert_eq!(b.content["text"], "hello");
            }
            other => panic!("expected Blob, got: {:?}", other),
        }
    }

    #[test]
    fn fetch_remote_works_against_ssh_store() {
        // PR5 cycle 22 (capstone): the real point of Stage D — a
        // Store backed by SSH must drive `fetch_remote` end-to-end.
        // The local FsStore receives the closure and the
        // remote-tracking ref, identical to the in-process case.
        let Some(bin) = morph_bin() else { return };

        let local_dir = tempfile::tempdir().unwrap();
        let _ = crate::repo::init_repo(local_dir.path()).unwrap();
        let local = crate::repo::open_store(&local_dir.path().join(".morph")).unwrap();

        let remote_dir = tempfile::tempdir().unwrap();
        let _ = crate::repo::init_repo(remote_dir.path()).unwrap();
        let remote_inproc =
            crate::repo::open_store(&remote_dir.path().join(".morph")).unwrap();
        // Build a real commit on the remote via the in-process
        // Store; we then connect a separate SshStore handle to the
        // same on-disk repo and let `fetch_remote` drive that.
        std::fs::write(remote_dir.path().join("a.txt"), "A").unwrap();
        crate::add_paths(
            remote_inproc.as_ref(),
            remote_dir.path(),
            &[std::path::PathBuf::from(".")],
        )
        .unwrap();
        let commit = crate::create_tree_commit(
            remote_inproc.as_ref(),
            remote_dir.path(),
            None,
            None,
            std::collections::BTreeMap::new(),
            "remote-only".to_string(),
            None,
            Some("0.3"),
        )
        .unwrap();
        drop(remote_inproc);

        let spawn = LocalSpawn::new(bin, remote_dir.path());
        let remote_ssh = SshStore::connect(&spawn).unwrap();

        let updated =
            crate::sync::fetch_remote(local.as_ref(), &remote_ssh, "origin").unwrap();
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].0, "main");
        assert_eq!(updated[0].1, commit);

        let tracking = local.ref_read("remotes/origin/main").unwrap();
        assert_eq!(tracking, Some(commit));
        assert!(local.has(&commit).unwrap());
    }

    #[test]
    fn ssh_store_get_missing_surfaces_typed_not_found() {
        // PR5 cycle 22 RED→GREEN: `MorphError::NotFound` must
        // round-trip end-to-end.
        let Some((_dir, spawn)) = setup_remote_repo() else { return };
        let store = SshStore::connect(&spawn).unwrap();
        let zeros = Hash::from_hex(&"0".repeat(64)).unwrap();
        let err = store.get(&zeros).unwrap_err();
        assert!(
            matches!(err, MorphError::NotFound(_)),
            "expected NotFound, got: {:?}",
            err
        );
    }
}
