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

/// Client → server request.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op")]
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
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Ok(OkResponse),
    Err(ErrResponse),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OkResponse {
    pub ok: bool, // always true; serde forces us to keep it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub morph_version: Option<String>,
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
        ErrorKind::Other => MorphError::Serialization(r.error.clone()),
    }
}

/// Build a successful "hello" response for the helper.
pub fn hello_ok(version: &str) -> OkResponse {
    OkResponse {
        ok: true,
        morph_version: Some(version.to_string()),
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
    }
}

fn default_ok() -> OkResponse {
    OkResponse {
        ok: true,
        morph_version: None,
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
    fn ok_response_skips_none_fields() {
        // We don't want list-branches responses to contain `"has":
        // null, "object": null, ...` — the wire stays clean.
        let ok = hello_ok("0.9.0");
        let s = serde_json::to_string(&ok).unwrap();
        assert!(!s.contains("\"has\""), "got: {}", s);
        assert!(!s.contains("\"object\""), "got: {}", s);
        assert!(s.contains("\"morph_version\":\"0.9.0\""), "got: {}", s);
    }
}
