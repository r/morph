//! Wire protocol for the SSH-driven `Store` transport.
//!
//! The `morph remote-helper` CLI subcommand (server side) and
//! `SshStore` (client side, PR5 Stage D) share one set of types so
//! that protocol changes ripple to both at the type-checker level
//! instead of via stringly-typed JSON.
//!
//! Each request and response is a single JSON object on its own
//! line. Successful responses always set `ok: true`; errors carry a
//! typed `error_kind` that round-trips back to a `MorphError`
//! variant on the client.

use crate::objects::MorphObject;
use crate::store::MorphError;
use crate::Hash;
use serde::{Deserialize, Serialize};

/// PR 6 stage E: pinned wire protocol version. Increment whenever
/// `Request` / `Response` shapes change in a way an older client
/// cannot ignore. Pre-PR6 helpers don't advertise this field at
/// all, which the client treats as protocol 0 (legacy).
pub const MORPH_PROTOCOL_VERSION: u32 = 1;

/// Client → server request.
///
/// `Put` carries a full `MorphObject` so it's much larger than the
/// other variants; boxing it would force every code path to allocate
/// on the heap even though `Request` values exist only briefly on the
/// stack between serialization steps.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op")]
#[allow(clippy::large_enum_variant)]
pub enum Request {
    #[serde(rename = "hello")]
    Hello,
    #[serde(rename = "list-branches")]
    ListBranches,
    #[serde(rename = "list-refs")]
    ListRefs { prefix: String },
    #[serde(rename = "ref-read")]
    RefRead { name: String },
    #[serde(rename = "ref-write")]
    RefWrite { name: String, hash: String },
    #[serde(rename = "has")]
    Has { hash: String },
    #[serde(rename = "get")]
    Get { hash: String },
    #[serde(rename = "put")]
    Put { object: MorphObject },
}

/// Pair returned by `list-branches` / `list-refs`.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct RefEntry {
    pub name: String,
    pub hash: String,
}

/// Server → client response. Untagged so each variant drops its
/// fields straight at the top level (matches what the v0 helper
/// already emits, makes responses pleasant to read in logs).
///
/// `OkResponse` carries optional payload fields (objects, ref lists)
/// and is naturally larger than `ErrResponse`; boxing the success
/// path would penalize the common case for marginal stack savings.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum Response {
    Ok(OkResponse),
    Err(ErrResponse),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OkResponse {
    pub ok: bool, // always true; serde forces us to keep it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub morph_version: Option<String>,
    /// PR 6 stage E: wire protocol version the helper speaks. None
    /// means a legacy (pre-PR6) helper that doesn't advertise it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u32>,
    /// PR 6 stage E: repo schema version of the remote store.
    /// Lets clients refuse to push to a repo on an incompatible
    /// schema instead of writing incoherent objects into it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branches: Option<Vec<RefEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs: Option<Vec<RefEntry>>,
    /// Wire-level: `null` for "ref does not exist" (ref-read) and a
    /// hex string for "got a hash" (ref-read existing / put). The
    /// double-Option dance via `Option<Option<String>>` doesn't
    /// round-trip through serde without custom hooks, so we keep
    /// this as a flat `Option<String>` and rely on the request kind
    /// to disambiguate "missing field" from "ref absent".
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<MorphObject>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrResponse {
    pub ok: bool, // always false
    pub error: String,
    pub error_kind: ErrorKind,
    /// Optional structured fields used by `Diverged`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_tip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_tip: Option<String>,
    /// PR 6 stage E: structured fields for `IncompatibleRemote`. Kept
    /// flat so older clients can still read the `error` string and
    /// older servers don't break the schema by emitting them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Stable taxonomy of error kinds. Keep `snake_case` strings in sync
/// with `MorphError` variants; new variants must be added to both
/// `from_morph_error` and `to_morph_error`.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotFound,
    InvalidHash,
    AlreadyExists,
    Diverged,
    Io,
    Serialization,
    UnknownOp,
    /// PR 6 stage E: remote/local protocol or repo schema mismatch.
    IncompatibleRemote,
    /// Reserved for non-classified failures so older clients can
    /// still pretty-print a message.
    Other,
}

/// Map a server-side error to its wire form.
pub fn from_morph_error(e: &MorphError) -> ErrResponse {
    let mut r = ErrResponse {
        ok: false,
        error: e.to_string(),
        error_kind: ErrorKind::Other,
        branch: None,
        local_tip: None,
        remote_tip: None,
        remote_version: None,
        local_version: None,
        reason: None,
    };
    match e {
        MorphError::NotFound(_) => r.error_kind = ErrorKind::NotFound,
        MorphError::InvalidHash(_) => r.error_kind = ErrorKind::InvalidHash,
        MorphError::AlreadyExists(_) => r.error_kind = ErrorKind::AlreadyExists,
        MorphError::Io(_) => r.error_kind = ErrorKind::Io,
        MorphError::Serialization(_) => r.error_kind = ErrorKind::Serialization,
        MorphError::Diverged { branch, local_tip, remote_tip } => {
            r.error_kind = ErrorKind::Diverged;
            r.branch = Some(branch.clone());
            r.local_tip = Some(local_tip.clone());
            r.remote_tip = Some(remote_tip.clone());
        }
        MorphError::IncompatibleRemote { remote, local, reason } => {
            r.error_kind = ErrorKind::IncompatibleRemote;
            r.remote_version = Some(remote.clone());
            r.local_version = Some(local.clone());
            r.reason = Some(reason.clone());
        }
        _ => r.error_kind = ErrorKind::Other,
    }
    r
}

/// Map a wire error back into a typed `MorphError` on the client
/// side. Loses no semantic information for the variants we care
/// about (NotFound, Diverged, …).
pub fn to_morph_error(r: &ErrResponse) -> MorphError {
    match r.error_kind {
        ErrorKind::NotFound => MorphError::NotFound(r.error.clone()),
        ErrorKind::InvalidHash => MorphError::InvalidHash(r.error.clone()),
        ErrorKind::AlreadyExists => MorphError::AlreadyExists(r.error.clone()),
        ErrorKind::Io => MorphError::Serialization(format!("remote io: {}", r.error)),
        ErrorKind::Serialization => MorphError::Serialization(r.error.clone()),
        ErrorKind::UnknownOp => {
            MorphError::Serialization(format!("remote: {}", r.error))
        }
        ErrorKind::Diverged => MorphError::Diverged {
            branch: r.branch.clone().unwrap_or_default(),
            local_tip: r.local_tip.clone().unwrap_or_default(),
            remote_tip: r.remote_tip.clone().unwrap_or_default(),
        },
        ErrorKind::IncompatibleRemote => MorphError::IncompatibleRemote {
            remote: r.remote_version.clone().unwrap_or_default(),
            local: r.local_version.clone().unwrap_or_default(),
            reason: r.reason.clone().unwrap_or_default(),
        },
        ErrorKind::Other => MorphError::Serialization(r.error.clone()),
    }
}

/// Build a successful "hello" response for the helper.
///
/// PR 6 stage E: callers now also advertise the wire protocol
/// version (so clients can refuse incompatible servers) and the
/// repo schema version (so clients can refuse to push to a repo on
/// an unknown schema).
pub fn hello_ok(version: &str, protocol: u32, repo_version: &str) -> OkResponse {
    OkResponse {
        ok: true,
        morph_version: Some(version.to_string()),
        protocol_version: Some(protocol),
        repo_version: Some(repo_version.to_string()),
        ..default_ok()
    }
}

/// Build a successful list-branches response.
pub fn list_refs_ok(refs: Vec<(String, Hash)>, kind: ListRefsKind) -> OkResponse {
    let entries: Vec<RefEntry> = refs
        .into_iter()
        .map(|(name, h)| RefEntry { name, hash: h.to_string() })
        .collect();
    let mut ok = default_ok();
    match kind {
        ListRefsKind::Branches => ok.branches = Some(entries),
        ListRefsKind::Refs => ok.refs = Some(entries),
    }
    ok
}

pub enum ListRefsKind {
    Branches,
    Refs,
}

pub fn ref_read_ok(h: Option<Hash>) -> OkResponse {
    OkResponse {
        hash: h.map(|x| x.to_string()),
        ..default_ok()
    }
}

pub fn ref_write_ok() -> OkResponse {
    default_ok()
}

pub fn has_ok(present: bool) -> OkResponse {
    OkResponse {
        has: Some(present),
        ..default_ok()
    }
}

pub fn get_ok(obj: MorphObject) -> OkResponse {
    OkResponse {
        object: Some(obj),
        ..default_ok()
    }
}

pub fn put_ok(h: Hash) -> OkResponse {
    OkResponse {
        hash: Some(h.to_string()),
        ..default_ok()
    }
}

pub fn unknown_op_err(op_text: &str) -> ErrResponse {
    ErrResponse {
        ok: false,
        error: format!("unknown op: {}", op_text),
        error_kind: ErrorKind::UnknownOp,
        branch: None,
        local_tip: None,
        remote_tip: None,
        remote_version: None,
        local_version: None,
        reason: None,
    }
}

fn default_ok() -> OkResponse {
    OkResponse {
        ok: true,
        morph_version: None,
        protocol_version: None,
        repo_version: None,
        branches: None,
        refs: None,
        hash: None,
        has: None,
        object: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_through_json() {
        let cases = vec![
            Request::Hello,
            Request::ListBranches,
            Request::ListRefs { prefix: "heads".into() },
            Request::RefRead { name: "heads/main".into() },
            Request::RefWrite {
                name: "heads/main".into(),
                hash: "0".repeat(64),
            },
            Request::Has { hash: "0".repeat(64) },
            Request::Get { hash: "0".repeat(64) },
        ];
        for r in cases {
            let s = serde_json::to_string(&r).unwrap();
            let parsed: Request = serde_json::from_str(&s).unwrap();
            assert_eq!(r, parsed, "round-trip failed for: {}", s);
        }
    }

    #[test]
    fn diverged_round_trips_with_typed_fields() {
        // PR5 cycle 13 RED→GREEN: a `Diverged` error must reach
        // the client with branch / local_tip / remote_tip fields
        // intact so the CLI can suggest `morph pull --merge`.
        let original = MorphError::Diverged {
            branch: "main".into(),
            local_tip: "a".repeat(64),
            remote_tip: "b".repeat(64),
        };
        let wire = from_morph_error(&original);
        assert_eq!(wire.error_kind, ErrorKind::Diverged);
        assert_eq!(wire.branch.as_deref(), Some("main"));

        let s = serde_json::to_string(&wire).unwrap();
        let parsed: ErrResponse = serde_json::from_str(&s).unwrap();
        let restored = to_morph_error(&parsed);
        match restored {
            MorphError::Diverged { branch, local_tip, remote_tip } => {
                assert_eq!(branch, "main");
                assert_eq!(local_tip, "a".repeat(64));
                assert_eq!(remote_tip, "b".repeat(64));
            }
            other => panic!("expected Diverged, got: {:?}", other),
        }
    }

    #[test]
    fn not_found_round_trips() {
        // PR5 cycle 14 RED→GREEN: NotFound is the most common error
        // a remote will produce (missing object during fetch). The
        // typed round-trip lets `transfer_objects` distinguish
        // it from generic transport errors.
        let original = MorphError::NotFound("abc".into());
        let wire = from_morph_error(&original);
        assert_eq!(wire.error_kind, ErrorKind::NotFound);

        let restored = to_morph_error(&wire);
        assert!(matches!(restored, MorphError::NotFound(_)));
    }

    #[test]
    fn unknown_op_is_a_distinct_kind() {
        // PR5 cycle 12 RED→GREEN: the helper returns this when the
        // request can't be parsed at all. The client surfaces it as
        // a `Serialization` error with a clear "remote: ..." prefix.
        let err = unknown_op_err("frob");
        assert_eq!(err.error_kind, ErrorKind::UnknownOp);
        let restored = to_morph_error(&err);
        match restored {
            MorphError::Serialization(s) => {
                assert!(s.contains("remote"), "got: {}", s);
                assert!(s.contains("frob"), "got: {}", s);
            }
            other => panic!("expected Serialization, got: {:?}", other),
        }
    }

    #[test]
    fn incompatible_remote_round_trips() {
        // PR 6 stage E cycle 20 RED→GREEN: when the remote speaks a
        // protocol version we don't understand, the client raises
        // `IncompatibleRemote` and the CLI explains "your local
        // morph is too old/new for that server". The wire form keeps
        // the typed remote/local version pair so the message is
        // actionable.
        let original = MorphError::IncompatibleRemote {
            remote: "2".into(),
            local: "1".into(),
            reason: "protocol_version".into(),
        };
        let wire = from_morph_error(&original);
        assert_eq!(wire.error_kind, ErrorKind::IncompatibleRemote);
        assert_eq!(wire.remote_version.as_deref(), Some("2"));
        assert_eq!(wire.local_version.as_deref(), Some("1"));
        assert_eq!(wire.reason.as_deref(), Some("protocol_version"));

        let s = serde_json::to_string(&wire).unwrap();
        let parsed: ErrResponse = serde_json::from_str(&s).unwrap();
        let restored = to_morph_error(&parsed);
        match restored {
            MorphError::IncompatibleRemote { remote, local, reason } => {
                assert_eq!(remote, "2");
                assert_eq!(local, "1");
                assert_eq!(reason, "protocol_version");
            }
            other => panic!("expected IncompatibleRemote, got: {:?}", other),
        }
    }

    #[test]
    fn ok_response_skips_none_fields() {
        // We don't want list-branches responses to contain `"has":
        // null, "object": null, ...` — the wire stays clean.
        let ok = hello_ok("0.9.0", MORPH_PROTOCOL_VERSION, "0.5");
        let s = serde_json::to_string(&ok).unwrap();
        assert!(!s.contains("\"has\""), "got: {}", s);
        assert!(!s.contains("\"object\""), "got: {}", s);
        assert!(s.contains("\"morph_version\":\"0.9.0\""), "got: {}", s);
    }

    #[test]
    fn hello_response_advertises_protocol_and_repo_version() {
        // PR 6 stage E cycle 21 RED→GREEN: Hello tells the client
        // both what wire protocol the helper speaks and what
        // repo schema the bare/working repo is on. The client
        // uses these to derive `IncompatibleRemote`.
        let ok = hello_ok("0.11.0", 1, "0.5");
        assert_eq!(ok.protocol_version, Some(1));
        assert_eq!(ok.repo_version.as_deref(), Some("0.5"));

        let s = serde_json::to_string(&ok).unwrap();
        let parsed: OkResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed.protocol_version, Some(1));
        assert_eq!(parsed.repo_version.as_deref(), Some("0.5"));
        assert_eq!(parsed.morph_version.as_deref(), Some("0.11.0"));
    }

    #[test]
    fn hello_response_omits_protocol_fields_for_legacy_servers() {
        // A pre-PR6 helper doesn't emit `protocol_version` or
        // `repo_version`. We must still parse such a hello so
        // forwards and backwards compat hold for one release cycle.
        let legacy = "{\"ok\":true,\"morph_version\":\"0.10.0\"}";
        let parsed: OkResponse = serde_json::from_str(legacy).unwrap();
        assert!(parsed.ok);
        assert_eq!(parsed.morph_version.as_deref(), Some("0.10.0"));
        assert_eq!(parsed.protocol_version, None);
        assert_eq!(parsed.repo_version, None);
    }

    #[test]
    fn protocol_version_constant_is_one_in_pr6() {
        // Pin the protocol version. Bump this constant deliberately
        // when the wire format changes.
        assert_eq!(MORPH_PROTOCOL_VERSION, 1);
    }
}
