//! `morph remote-helper` — line-oriented JSON-RPC server that
//! exposes a `Store` to a remote `SshStore` client (PR5 Stage D).
//!
//! Wire definitions live in `morph_core::ssh_proto`; this module is
//! a thin adapter that runs requests against an in-process `Store`.

use anyhow::Result;
use morph_core::objects::MorphObject;
use morph_core::ssh_proto::{
    self, ErrResponse, ListRefsKind, OkResponse, Request, Response,
};
use morph_core::store::Store;
use morph_core::Hash;
use std::io::{BufRead, Write};
use std::path::Path;

/// Entry point invoked from `Command::RemoteHelper`. Returns Ok(())
/// on graceful EOF; any setup error is bubbled up to main and
/// surfaced as a non-zero exit.
pub fn run(repo_root: &Path) -> Result<()> {
    let morph_dir = repo_root.join(".morph");
    if !morph_dir.exists() {
        anyhow::bail!(
            "not a morph repository: {} (no .morph directory)",
            repo_root.display()
        );
    }
    let store = morph_core::open_store(&morph_dir)?;

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
        let resp = handle_line(line, store.as_ref());
        let s = match resp {
            Response::Ok(o) => serde_json::to_string(&o)?,
            Response::Err(e) => serde_json::to_string(&e)?,
        };
        writeln!(out, "{}", s)?;
        out.flush()?;
    }
    Ok(())
}

fn handle_line(line: &str, store: &dyn Store) -> Response {
    let req: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return Response::Err(ssh_proto::unknown_op_err(&format!(
                "could not parse request: {}",
                e
            )));
        }
    };
    match dispatch(req, store) {
        Ok(ok) => Response::Ok(ok),
        Err(err) => Response::Err(err),
    }
}

fn dispatch(req: Request, store: &dyn Store) -> std::result::Result<OkResponse, ErrResponse> {
    match req {
        Request::Hello => Ok(ssh_proto::hello_ok(env!("CARGO_PKG_VERSION"))),
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
