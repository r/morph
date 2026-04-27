//! `morph remote-helper` — line-oriented JSON-RPC server that
//! exposes a `Store` to a remote `SshStore` client (PR5 Stage D).
//!
//! Wire definitions live in `morph_core::ssh_proto`; this module is
//! a thin adapter that runs requests against an in-process `Store`.

use anyhow::Result;
use morph_core::objects::MorphObject;
use morph_core::ssh_proto::{
    self, ErrResponse, ListRefsKind, OkResponse, Request, Response,
    MORPH_PROTOCOL_VERSION,
};
use morph_core::store::Store;
use morph_core::Hash;
use std::io::{BufRead, Write};
use std::path::Path;

/// Entry point invoked from `Command::RemoteHelper`. Returns Ok(())
/// on graceful EOF; any setup error is bubbled up to main and
/// surfaced as a non-zero exit.
pub fn run(repo_root: &Path) -> Result<()> {
    // PR 6 stage D cycle 18: the helper accepts both shapes —
    // working repos at `<root>/.morph` and bare repos directly at
    // `<root>`. `resolve_morph_dir` does the auto-detect and
    // surfaces a `not a morph repository` error if neither is
    // present.
    let morph_dir = morph_core::resolve_morph_dir(repo_root)
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    let store = morph_core::open_store(&morph_dir)?;
    // PR 6 stage E: read the repo schema version once at startup so
    // every Hello can advertise it without reopening config.json on
    // each request.
    let repo_version = morph_core::read_repo_version(&morph_dir)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    // PR 6 stage E cycle 23: testing hook. When set, override the
    // protocol version we advertise. Used only by integration tests
    // to exercise the `IncompatibleRemote` path; production helpers
    // always emit `MORPH_PROTOCOL_VERSION`.
    let protocol_version = std::env::var("MORPH_TEST_PROTOCOL_VERSION_OVERRIDE")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(MORPH_PROTOCOL_VERSION);

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let resp = handle_line(
            line,
            store.as_ref(),
            &morph_dir,
            &repo_version,
            protocol_version,
        );
        let s = match resp {
            Response::Ok(o) => serde_json::to_string(&o)?,
            Response::Err(e) => serde_json::to_string(&e)?,
        };
        writeln!(out, "{}", s)?;
        out.flush()?;
    }
    Ok(())
}

fn handle_line(
    line: &str,
    store: &dyn Store,
    morph_dir: &Path,
    repo_version: &str,
    protocol_version: u32,
) -> Response {
    let req: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return Response::Err(ssh_proto::unknown_op_err(&format!(
                "could not parse request: {}",
                e
            )));
        }
    };
    match dispatch(req, store, morph_dir, repo_version, protocol_version) {
        Ok(ok) => Response::Ok(ok),
        Err(err) => Response::Err(err),
    }
}

// `ErrResponse` carries optional structured error fields and is
// inherently bigger than `OkResponse` for trivial replies; the helper
// uses this `Result` only as control flow inside the loop, so the
// payoff of boxing every error is too small to justify the noise.
#[allow(clippy::result_large_err)]
fn dispatch(
    req: Request,
    store: &dyn Store,
    morph_dir: &Path,
    repo_version: &str,
    protocol_version: u32,
) -> std::result::Result<OkResponse, ErrResponse> {
    match req {
        Request::Hello => Ok(ssh_proto::hello_ok(
            env!("CARGO_PKG_VERSION"),
            protocol_version,
            repo_version,
        )),
        Request::ListBranches => {
            let branches = store
                .list_branches()
                .map_err(|e| ssh_proto::from_morph_error(&e))?;
            Ok(ssh_proto::list_refs_ok(branches, ListRefsKind::Branches))
        }
        Request::ListRefs { prefix } => {
            let refs = store
                .list_refs(&prefix)
                .map_err(|e| ssh_proto::from_morph_error(&e))?;
            Ok(ssh_proto::list_refs_ok(refs, ListRefsKind::Refs))
        }
        Request::RefRead { name } => {
            let h = store
                .ref_read(&name)
                .map_err(|e| ssh_proto::from_morph_error(&e))?;
            Ok(ssh_proto::ref_read_ok(h))
        }
        Request::RefWrite { name, hash } => {
            let h = Hash::from_hex(&hash).map_err(|e| {
                ssh_proto::from_morph_error(
                    &morph_core::store::MorphError::InvalidHash(e.to_string()),
                )
            })?;
            // PR 6 stage F cycle 25: refuse a ref-write whose
            // closure isn't fully present on the server. Without
            // this, a crashed `morph push` could leave a bare repo
            // pointing at objects no client has and break every
            // subsequent fetch.
            morph_core::verify_closure(store, &h)
                .map_err(|e| ssh_proto::from_morph_error(&e))?;
            // PR 6 stage F cycle 28: server-side push gate. If the
            // target branch is listed in `RepoPolicy
            // .push_gated_branches`, run `gate_check` and refuse
            // the write on failure with a clear typed error.
            morph_core::enforce_push_gate(store, morph_dir, &name, &h)
                .map_err(|e| ssh_proto::from_morph_error(&e))?;
            store
                .ref_write(&name, &h)
                .map_err(|e| ssh_proto::from_morph_error(&e))?;
            Ok(ssh_proto::ref_write_ok())
        }
        Request::Has { hash } => {
            let h = Hash::from_hex(&hash).map_err(|e| {
                ssh_proto::from_morph_error(
                    &morph_core::store::MorphError::InvalidHash(e.to_string()),
                )
            })?;
            let has = store.has(&h).map_err(|e| ssh_proto::from_morph_error(&e))?;
            Ok(ssh_proto::has_ok(has))
        }
        Request::Get { hash } => {
            let h = Hash::from_hex(&hash).map_err(|e| {
                ssh_proto::from_morph_error(
                    &morph_core::store::MorphError::InvalidHash(e.to_string()),
                )
            })?;
            let obj = store.get(&h).map_err(|e| ssh_proto::from_morph_error(&e))?;
            Ok(ssh_proto::get_ok(obj))
        }
        Request::Put { object } => {
            let obj: MorphObject = object;
            let h = store.put(&obj).map_err(|e| ssh_proto::from_morph_error(&e))?;
            Ok(ssh_proto::put_ok(h))
        }
    }
}
