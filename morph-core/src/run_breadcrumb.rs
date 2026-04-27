//! `LAST_RUN.json` breadcrumb: a small file under `.morph/` that
//! `morph eval run` (and `morph eval from-output --record`) writes
//! after persisting a metric-bearing Run, so a follow-up
//! `morph commit` can auto-attach the run's metrics + provenance
//! without the user having to copy the run hash.
//!
//! The breadcrumb is *not* a versioned object — it is purely a CLI
//! ergonomics helper. The merge gate never reads it; it only sees the
//! Run that the breadcrumb points at, after the commit handler has
//! resolved it to a regular `--from-run` provenance.
//!
//! Two staleness checks make the breadcrumb safe to read implicitly:
//!
//! 1. `head` must match the current HEAD commit (or both must be
//!    `None` for a pre-root state).
//! 2. `index_fingerprint` must match the current staging index'sfingerprint.
//!
//! On any mismatch the breadcrumb is ignored. The commit handler is
//! responsible for clearing it after a successful commit so the same
//! run is not silently re-attached to a follow-up commit.

use crate::commit::resolve_head;
use crate::index::{read_index, StagingIndex};
use crate::store::{MorphError, Store};
use crate::Hash;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

const LAST_RUN_FILE: &str = "LAST_RUN.json";

/// Breadcrumb left by `morph eval run` so a follow-up `morph commit`
/// can pick up the run's metrics and evidence_refs automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRun {
    /// Hex hash of the Run object that was just persisted.
    pub run: String,
    /// Hex hash of HEAD at record time, or `None` when the repo had
    /// no commits yet. The commit handler compares against the
    /// current HEAD to detect HEAD-mismatch staleness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Deterministic fingerprint of the staging index at record time.
    /// The commit handler compares against the current fingerprint
    /// to detect worktree staleness (the user staged or unstaged
    /// files since the run was recorded).
    pub index_fingerprint: String,
    /// RFC3339 timestamp of when the breadcrumb was written. Carried
    /// through purely for human-friendly stderr messages — never
    /// part of the staleness decision.
    pub recorded_at: String,
}

/// Compute a stable fingerprint of the staging index.
///
/// We hash the canonical JSON of the index entries (sorted by path,
/// per `BTreeMap`'s contract). Unmerged entries are folded in too so
/// a mid-merge breadcrumb doesn't get reused after the merge resolves.
pub fn fingerprint_index(idx: &StagingIndex) -> String {
    let json = serde_json::to_string(idx).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    let digest = hasher.finalize();
    hex::encode(digest)
}

/// Write (or atomically replace) the breadcrumb at `<morph_dir>/LAST_RUN.json`.
pub fn write_last_run(morph_dir: &Path, last: &LastRun) -> Result<(), MorphError> {
    let path = morph_dir.join(LAST_RUN_FILE);
    let tmp = morph_dir.join(format!("{LAST_RUN_FILE}.tmp"));
    let json = serde_json::to_string_pretty(last)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Read the breadcrumb if present. Returns `Ok(None)` when the file
/// is missing; returns an error only on I/O or malformed JSON. The
/// commit handler treats a malformed breadcrumb as "ignore + warn"
/// rather than a hard failure to keep the auto-pickup safe.
pub fn read_last_run(morph_dir: &Path) -> Result<Option<LastRun>, MorphError> {
    let path = morph_dir.join(LAST_RUN_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&path)?;
    let last: LastRun = serde_json::from_str(&data)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    Ok(Some(last))
}

/// Remove the breadcrumb. Idempotent — missing file is not an error.
pub fn clear_last_run(morph_dir: &Path) -> Result<(), MorphError> {
    let path = morph_dir.join(LAST_RUN_FILE);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// One-shot helper that captures the current HEAD + index
/// fingerprint and writes the breadcrumb pointing at `run_hash`.
///
/// Designed for the `morph eval run` and
/// `morph eval from-output --record` code paths (and the matching
/// MCP tools) so the breadcrumb is written identically regardless
/// of the entry point. Callers can ignore the result and surface a
/// soft warning instead of failing the eval flow itself.
pub fn record_last_run(
    store: &dyn Store,
    morph_dir: &Path,
    run_hash: &Hash,
) -> Result<LastRun, MorphError> {
    let head = resolve_head(store)?.map(|h| h.to_string());
    let index = read_index(morph_dir)?;
    let last = LastRun {
        run: run_hash.to_string(),
        head,
        index_fingerprint: fingerprint_index(&index),
        recorded_at: chrono::Utc::now().to_rfc3339(),
    };
    write_last_run(morph_dir, &last)?;
    Ok(last)
}

/// Resolve the breadcrumb against the current HEAD and staging
/// index. Returns the `LastRun` only when the breadcrumb is fresh
/// (HEAD matches and the staging index fingerprint matches);
/// otherwise returns `Ok(None)` along with a `staleness` reason
/// the caller can surface as a stderr nudge.
///
/// `Ok(None, None)` — no breadcrumb on disk.
/// `Ok(None, Some(reason))` — breadcrumb exists but is stale.
/// `Ok(Some(last), None)` — breadcrumb is fresh and ready to use.
pub fn resolve_fresh_last_run(
    store: &dyn Store,
    morph_dir: &Path,
) -> Result<(Option<LastRun>, Option<StaleReason>), MorphError> {
    let Some(last) = read_last_run(morph_dir)? else {
        return Ok((None, None));
    };
    let head = resolve_head(store)?.map(|h| h.to_string());
    if head != last.head {
        return Ok((None, Some(StaleReason::HeadChanged)));
    }
    let index = read_index(morph_dir)?;
    if fingerprint_index(&index) != last.index_fingerprint {
        return Ok((None, Some(StaleReason::IndexChanged)));
    }
    Ok((Some(last), None))
}

/// Why a breadcrumb was rejected. Surfaces in the commit handler's
/// stderr nudge so users can tell the difference between "I forgot
/// to run tests" and "I added more files since the run".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleReason {
    /// HEAD moved since the run was recorded.
    HeadChanged,
    /// The staging index changed since the run was recorded
    /// (added/removed/changed files).
    IndexChanged,
}

impl StaleReason {
    pub fn as_human(&self) -> &'static str {
        match self {
            StaleReason::HeadChanged => "HEAD changed since recording",
            StaleReason::IndexChanged => "worktree changed since recording",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{update_index, write_index};

    fn fixture(run: &str, head: Option<&str>, fp: &str) -> LastRun {
        LastRun {
            run: run.into(),
            head: head.map(|s| s.into()),
            index_fingerprint: fp.into(),
            recorded_at: "2026-04-27T10:00:00Z".into(),
        }
    }

    #[test]
    fn write_then_read_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let last = fixture(&"a".repeat(64), Some(&"b".repeat(64)), "fp");
        write_last_run(dir.path(), &last).unwrap();
        let loaded = read_last_run(dir.path()).unwrap().expect("breadcrumb present");
        assert_eq!(loaded, last);
    }

    #[test]
    fn read_returns_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_last_run(dir.path()).unwrap(), None);
    }

    #[test]
    fn read_errors_on_malformed_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(LAST_RUN_FILE), "{not json").unwrap();
        assert!(read_last_run(dir.path()).is_err());
    }

    #[test]
    fn clear_removes_file_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        clear_last_run(dir.path()).unwrap(); // missing → ok
        let last = fixture(&"a".repeat(64), None, "fp");
        write_last_run(dir.path(), &last).unwrap();
        assert!(dir.path().join(LAST_RUN_FILE).exists());
        clear_last_run(dir.path()).unwrap();
        assert!(!dir.path().join(LAST_RUN_FILE).exists());
        clear_last_run(dir.path()).unwrap(); // again → still ok
    }

    #[test]
    fn record_last_run_captures_head_and_index_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        update_index(&morph_dir, "f.txt", &"a".repeat(64)).unwrap();
        let run_hash = Hash::from_hex(&"d".repeat(64)).unwrap();
        let last = record_last_run(&store, &morph_dir, &run_hash).unwrap();
        assert_eq!(last.run, run_hash.to_string());
        assert_eq!(last.head, None, "no HEAD yet → None");
        assert!(!last.index_fingerprint.is_empty());

        let from_disk = read_last_run(&morph_dir).unwrap().expect("present");
        assert_eq!(from_disk, last);
    }

    #[test]
    fn resolve_fresh_returns_breadcrumb_when_head_and_index_match() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        update_index(&morph_dir, "f.txt", &"a".repeat(64)).unwrap();
        let run_hash = Hash::from_hex(&"d".repeat(64)).unwrap();
        record_last_run(&store, &morph_dir, &run_hash).unwrap();
        let (got, stale) = resolve_fresh_last_run(&store, &morph_dir).unwrap();
        assert!(got.is_some());
        assert_eq!(stale, None);
        assert_eq!(got.unwrap().run, run_hash.to_string());
    }

    #[test]
    fn resolve_fresh_marks_index_change_as_stale() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        update_index(&morph_dir, "f.txt", &"a".repeat(64)).unwrap();
        let run_hash = Hash::from_hex(&"d".repeat(64)).unwrap();
        record_last_run(&store, &morph_dir, &run_hash).unwrap();
        update_index(&morph_dir, "g.txt", &"b".repeat(64)).unwrap();
        let (got, stale) = resolve_fresh_last_run(&store, &morph_dir).unwrap();
        assert!(got.is_none());
        assert_eq!(stale, Some(StaleReason::IndexChanged));
    }

    #[test]
    fn resolve_fresh_marks_head_change_as_stale() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        let fake = LastRun {
            run: "a".repeat(64),
            head: Some("c".repeat(64)),
            index_fingerprint: fingerprint_index(&read_index(&morph_dir).unwrap()),
            recorded_at: "2026-01-01T00:00:00Z".into(),
        };
        write_last_run(&morph_dir, &fake).unwrap();
        let (got, stale) = resolve_fresh_last_run(&store, &morph_dir).unwrap();
        assert!(got.is_none());
        assert_eq!(stale, Some(StaleReason::HeadChanged));
    }

    #[test]
    fn resolve_fresh_returns_none_when_no_breadcrumb() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        let (got, stale) = resolve_fresh_last_run(&store, &morph_dir).unwrap();
        assert!(got.is_none());
        assert_eq!(stale, None);
    }

    #[test]
    fn fingerprint_index_is_deterministic_and_distinguishes_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = StagingIndex::new();
        a.entries.insert("foo.txt".into(), "a".repeat(64));
        a.entries.insert("bar.txt".into(), "b".repeat(64));
        write_index(dir.path(), &a).unwrap();

        let mut b = a.clone();
        // Same content → same fingerprint.
        let fp1 = fingerprint_index(&a);
        let fp2 = fingerprint_index(&b);
        assert_eq!(fp1, fp2);

        // Add a new entry → fingerprint changes.
        b.entries.insert("baz.txt".into(), "c".repeat(64));
        let fp3 = fingerprint_index(&b);
        assert_ne!(fp1, fp3);
    }
}
