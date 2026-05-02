//! `morph forget` (v0.41.0): permanent retirement of `Run`,
//! `Trace`, or prompt `Blob` objects.
//!
//! A forget operation:
//! 1. Validates that the named object is a `Run`, `Trace`, or
//!    prompt `Blob` (other kinds are refused; trying to forget a
//!    `Commit` would tear holes in the version-control DAG).
//! 2. Optionally walks every `Commit` in the store and refuses if
//!    the named hash appears in any commit's `evidence_refs`
//!    unless `--force` is set. (The intent is to make accidental
//!    forgets loud; a deliberate operator can opt past the check.)
//! 3. Removes the object's primary `objects/<hash>.json` plus its
//!    entry in every type-index dir.
//! 4. Writes a `Tombstone` object recording the actor / reason /
//!    timestamp, and a `forgotten/<original_hash>.txt` marker
//!    pointing at the tombstone hash. Subsequent
//!    `FsStore::is_forgotten()` returns `true`.
//!
//! Replays from a remote (`apply_tombstone`) follow the same
//! steps but skip the kind/reference validation — the originating
//! repo already enforced those, and the receiving repo wants the
//! deletion to be authoritative.
//!
//! What forget does **not** cover:
//! - **Already-fetched copies on other laptops.** A teammate
//!   who pulled the trace before the `morph forget --remote` push
//!   still has it. The next `morph fetch` from the remote will
//!   apply the tombstone, but data already on disk is the
//!   teammate's choice to delete.
//! - **Commits, blobs (other than prompts), trees, pipelines,
//!   eval suites, artifacts, trace rollups, annotations.** These
//!   all hold structural meaning that the version-control DAG
//!   depends on — a forgotten commit would silently break parent
//!   chains, a forgotten tree would break checkouts.
//! - **Partial redaction.** `morph forget` is whole-object only.
//!   Editing a trace to remove a single secret is not safe — the
//!   resulting "trace" would have a different hash and would be
//!   indistinguishable from a fabrication.

use crate::hash::Hash;
use crate::objects::{MorphObject, Tombstone};
use crate::store::{FsStore, MorphError, ObjectType};
use crate::time::now_rfc3339_utc;

/// Identifies the result of a forget operation. Used by the CLI to
/// surface a one-line confirmation to the operator.
#[derive(Clone, Debug)]
pub struct ForgetReport {
    /// The hash of the (now retired) original object.
    pub original_hash: Hash,
    /// The kind of the retired object as the tombstone records it.
    pub original_kind: String,
    /// The hash of the new `Tombstone` object recording the
    /// retirement.
    pub tombstone_hash: Hash,
    /// Commits whose `evidence_refs` named this hash. Recorded so
    /// the CLI can warn the operator that "N commits previously
    /// pointed at this evidence; they now read as no-claim."
    pub referencing_commits: Vec<Hash>,
}

/// True when an object of this kind is allowed to be forgotten.
/// `Run`, `Trace`, and prompt-`Blob` carry private session data;
/// every other kind is structural and refused.
pub fn kind_is_forgettable(obj: &MorphObject) -> bool {
    match obj {
        MorphObject::Run(_) => true,
        MorphObject::Trace(_) => true,
        MorphObject::Blob(b) => b.kind == "prompt",
        _ => false,
    }
}

/// Human-readable string label for the forgettable kind. Returns
/// `None` when the kind is not allowed (the caller should refuse).
pub fn forgettable_kind_label(obj: &MorphObject) -> Option<&'static str> {
    match obj {
        MorphObject::Run(_) => Some("run"),
        MorphObject::Trace(_) => Some("trace"),
        MorphObject::Blob(b) if b.kind == "prompt" => Some("prompt"),
        _ => None,
    }
}

/// Walk every commit in the store and return those whose
/// `evidence_refs` contain `target`. Used by `forget_local` to
/// surface the impact before the deletion lands. Linear in the
/// number of commits — cheap on small repos, acceptable on
/// large ones because forget is a rare operation.
pub fn commits_referencing(
    store: &FsStore,
    target: &Hash,
) -> Result<Vec<Hash>, MorphError> {
    use crate::store::Store;
    let mut out = Vec::new();
    let target_hex = target.to_string();
    for commit_hash in store.list(ObjectType::Commit)? {
        if let Ok(MorphObject::Commit(c)) = store.get(&commit_hash) {
            if let Some(refs) = &c.evidence_refs {
                if refs.iter().any(|s| s == &target_hex) {
                    out.push(commit_hash);
                }
            }
        }
    }
    Ok(out)
}

/// Remove the named object from the local store and record a
/// tombstone for it.
///
/// Validation:
/// - `target` must resolve to a `Run`, `Trace`, or prompt `Blob`.
///   Anything else returns `MorphError::Other("refused: …")`.
/// - When `force` is `false`, refuses if any `Commit` in the store
///   has `target` in its `evidence_refs`. The error message lists
///   up to three referencing commits so the operator can audit.
/// - When the target is *already* tombstoned, returns
///   `MorphError::AlreadyExists` so callers can surface an
///   idempotent no-op cleanly.
///
/// On success returns a `ForgetReport` that the CLI uses to print
/// "forgot <kind> <hash>; tombstone <hash>; N commit(s) now read as no-claim".
pub fn forget_local(
    store: &FsStore,
    target: &Hash,
    actor: &str,
    reason: Option<&str>,
    force: bool,
) -> Result<ForgetReport, MorphError> {
    use crate::store::Store;

    if store.is_forgotten(target)? {
        return Err(MorphError::AlreadyExists(format!(
            "{} is already forgotten",
            target
        )));
    }

    let obj = store.get(target)?;
    let kind = forgettable_kind_label(&obj).ok_or_else(|| {
        MorphError::Other(format!(
            "refused: {} is a {}; morph forget only retires runs, traces, or prompt blobs",
            target,
            obj.kind_str()
        ))
    })?;

    let referencing_commits = commits_referencing(store, target)?;
    if !referencing_commits.is_empty() && !force {
        let preview: Vec<String> = referencing_commits
            .iter()
            .take(3)
            .map(|h| h.to_string()[..12].to_string())
            .collect();
        let extra = if referencing_commits.len() > 3 {
            format!(" (+{} more)", referencing_commits.len() - 3)
        } else {
            String::new()
        };
        return Err(MorphError::Other(format!(
            "refused: {} is named in evidence_refs of {} commit(s): {}{}. \
             Pass --force to forget anyway (the merge gate will treat \
             those references as 'no claim').",
            target,
            referencing_commits.len(),
            preview.join(", "),
            extra
        )));
    }

    let tombstone = Tombstone {
        original_hash: target.to_string(),
        original_kind: kind.to_string(),
        forgotten_at: now_rfc3339_utc(),
        actor: actor.to_string(),
        reason: reason.map(|s| s.to_string()),
    };

    let tombstone_hash = store.write_tombstone(&tombstone)?;

    Ok(ForgetReport {
        original_hash: *target,
        original_kind: kind.to_string(),
        tombstone_hash,
        referencing_commits,
    })
}

/// Apply a tombstone received from a remote. Idempotent: if the
/// store already has the same tombstone, this is a no-op. If the
/// store still has the original object, the object's bytes and
/// type-index entries are scrubbed.
///
/// Receivers don't validate `kind_is_forgettable` — the originating
/// repo already enforced that, and refusing replay would mean an
/// infinite re-forget loop on every fetch.
pub fn apply_tombstone(
    store: &FsStore,
    tombstone: &Tombstone,
) -> Result<bool, MorphError> {
    let original_hash = Hash::from_hex(&tombstone.original_hash)
        .map_err(|_| MorphError::InvalidHash(tombstone.original_hash.clone()))?;

    if store.is_forgotten(&original_hash)? {
        return Ok(false);
    }

    store.write_tombstone(tombstone)?;
    Ok(true)
}

/// Hint string for "I forgot something I shouldn't have."
/// Surfaced by the CLI on successful forget so the operator
/// remembers that the deletion is local + remote-coordinated, not
/// "purges already-fetched copies on teammates' laptops."
pub const RETROACTIVE_NOTE: &str =
    "Note: tombstones do not reach copies that were already fetched \
     before the deletion. If a teammate previously pulled this hash, \
     ask them to fetch from the remote again or delete it by hand.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{AgentInfo, Blob, Run, RunEnvironment};
    use crate::store::Store;
    use std::collections::BTreeMap;

    fn temp_store() -> (tempfile::TempDir, FsStore) {
        let dir = tempfile::tempdir().unwrap();
        let morph_dir = dir.path().to_path_buf();
        std::fs::create_dir_all(morph_dir.join("objects")).unwrap();
        std::fs::create_dir_all(morph_dir.join("refs/heads")).unwrap();
        let store = FsStore::new_git_fanout(&morph_dir);
        (dir, store)
    }

    fn dummy_run() -> MorphObject {
        MorphObject::Run(Run {
            pipeline: "0".repeat(64),
            commit: Some("0".repeat(64)),
            environment: RunEnvironment {
                model: "test-model".into(),
                version: "v0".into(),
                parameters: BTreeMap::new(),
                toolchain: BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: BTreeMap::new(),
            trace: "0".repeat(64),
            agent: AgentInfo {
                id: "test".into(),
                version: "1".into(),
                instance_id: None,
                policy: None,
            },
            contributors: None,
            morph_version: None,
        })
    }

    #[test]
    fn forget_run_writes_tombstone_and_deletes_object() {
        let (_d, store) = temp_store();
        let run_hash = store.put(&dummy_run()).unwrap();
        assert!(store.has(&run_hash).unwrap());

        let report = forget_local(
            &store,
            &run_hash,
            "raffi@example.com",
            Some("test"),
            false,
        )
        .expect("forget should succeed");

        assert_eq!(report.original_kind, "run");
        assert!(!store.has(&run_hash).unwrap(), "object should be gone");
        assert!(store.is_forgotten(&run_hash).unwrap(), "marker should exist");

        let tombstone = store
            .read_tombstone(&run_hash)
            .unwrap()
            .expect("tombstone should be readable");
        assert_eq!(tombstone.original_hash, run_hash.to_string());
        assert_eq!(tombstone.original_kind, "run");
        assert_eq!(tombstone.actor, "raffi@example.com");
        assert_eq!(tombstone.reason.as_deref(), Some("test"));
    }

    #[test]
    fn forget_refuses_commit() {
        use crate::objects::{Commit, EvalContract};
        let (_d, store) = temp_store();
        let commit = MorphObject::Commit(Commit {
            tree: None,
            pipeline: "0".repeat(64),
            parents: vec![],
            message: "x".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            author: "a".into(),
            contributors: None,
            eval_contract: EvalContract {
                suite: "0".repeat(64),
                observed_metrics: BTreeMap::new(),
            },
            env_constraints: None,
            evidence_refs: None,
            morph_version: None,
            morph_instance: None,
            morph_origin: None,
            git_origin_sha: None,
            human_edits: None,
        });
        let h = store.put(&commit).unwrap();
        let err = forget_local(&store, &h, "a", None, false).unwrap_err();
        assert!(err.to_string().contains("refused"), "{}", err);
    }

    #[test]
    fn forget_refuses_blob_unless_prompt_kind() {
        let (_d, store) = temp_store();
        let blob = MorphObject::Blob(Blob {
            kind: "data".into(),
            content: serde_json::json!({"foo": "bar"}),
        });
        let h = store.put(&blob).unwrap();
        let err = forget_local(&store, &h, "a", None, false).unwrap_err();
        assert!(err.to_string().contains("refused"), "{}", err);

        let prompt = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"text": "hi"}),
        });
        let p = store.put(&prompt).unwrap();
        let report = forget_local(&store, &p, "a", None, false).unwrap();
        assert_eq!(report.original_kind, "prompt");
    }

    #[test]
    fn forget_refuses_with_referencing_commit_unless_force() {
        use crate::objects::{Commit, EvalContract};
        let (_d, store) = temp_store();
        let run_hash = store.put(&dummy_run()).unwrap();
        let commit = MorphObject::Commit(Commit {
            tree: None,
            pipeline: "0".repeat(64),
            parents: vec![],
            message: "x".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            author: "a".into(),
            contributors: None,
            eval_contract: EvalContract {
                suite: "0".repeat(64),
                observed_metrics: BTreeMap::new(),
            },
            env_constraints: None,
            evidence_refs: Some(vec![run_hash.to_string()]),
            morph_version: None,
            morph_instance: None,
            morph_origin: None,
            git_origin_sha: None,
            human_edits: None,
        });
        store.put(&commit).unwrap();

        let err = forget_local(&store, &run_hash, "a", None, false).unwrap_err();
        assert!(err.to_string().contains("commit(s)"), "{}", err);

        let report = forget_local(&store, &run_hash, "a", None, true).unwrap();
        assert_eq!(report.referencing_commits.len(), 1);
    }

    #[test]
    fn forget_is_idempotent_via_already_exists() {
        let (_d, store) = temp_store();
        let run_hash = store.put(&dummy_run()).unwrap();
        forget_local(&store, &run_hash, "a", None, false).unwrap();
        let err = forget_local(&store, &run_hash, "a", None, false).unwrap_err();
        assert!(matches!(err, MorphError::AlreadyExists(_)));
    }

    #[test]
    fn apply_tombstone_round_trips_from_remote() {
        let (_a, source) = temp_store();
        let run_hash = source.put(&dummy_run()).unwrap();
        let report = forget_local(&source, &run_hash, "a", Some("leak"), false).unwrap();
        let tombstone = source
            .read_tombstone(&run_hash)
            .unwrap()
            .expect("source has tombstone");

        let (_b, dest) = temp_store();
        let dest_run_hash = dest.put(&dummy_run()).unwrap();
        assert_eq!(dest_run_hash, run_hash, "deterministic hash");
        assert!(dest.has(&run_hash).unwrap());

        let applied = apply_tombstone(&dest, &tombstone).unwrap();
        assert!(applied);
        assert!(!dest.has(&run_hash).unwrap());
        assert!(dest.is_forgotten(&run_hash).unwrap());

        let again = apply_tombstone(&dest, &tombstone).unwrap();
        assert!(!again, "second apply is a no-op");

        // Same-shaped tombstone hash on both sides
        let _ = report.tombstone_hash;
    }
}
