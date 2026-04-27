//! On-disk merge state files for an in-progress 3-way merge.
//!
//! When a `morph merge` (PR 4) starts and produces conflicts, the
//! orchestrator writes the following files under `.morph/` so that the
//! merge can resume across process invocations and survive `--abort`:
//!
//! - **`MERGE_HEAD`** — single-line hex hash of `theirs` (the commit being
//!   merged in). Its presence is the canonical "merge in progress" flag.
//! - **`MERGE_MSG`** — UTF-8 commit-message draft pre-populated by the
//!   orchestrator and editable by the user before `--continue`.
//! - **`ORIG_HEAD`** — single-line hex hash of HEAD before the merge
//!   started, used by `morph merge --abort` to restore the working tree.
//! - **`MERGE_PIPELINE.json`** — serialized [`Pipeline`] from the
//!   structural pipeline merge (PR 2). Edited via `morph merge
//!   resolve-node` during conflict resolution (PR 4).
//! - **`MERGE_SUITE`** — single-line hex hash of the unioned EvalSuite
//!   (PR 1) so `--continue` can stamp the merged commit's `eval_contract`.
//!
//! All readers tolerate missing files by returning `Ok(None)`; writers are
//! atomic-replace so a crashed `morph merge` never leaves a half-written
//! state file. Old morph binaries (pre-PR 3) won't know about these files
//! and will silently ignore them — but `merge_in_progress(...)` exists so
//! the new binary can refuse to do destructive work mid-merge.

use crate::objects::Pipeline;
use crate::store::MorphError;
use crate::Hash;
use serde::{Deserialize, Serialize};
use std::path::Path;

const MERGE_HEAD: &str = "MERGE_HEAD";
const MERGE_MSG: &str = "MERGE_MSG";
const ORIG_HEAD: &str = "ORIG_HEAD";
const MERGE_PIPELINE: &str = "MERGE_PIPELINE.json";
const MERGE_SUITE: &str = "MERGE_SUITE";
const MERGE_RETIRED: &str = "MERGE_RETIRED.json";

const ALL_FILES: &[&str] = &[
    MERGE_HEAD,
    MERGE_MSG,
    ORIG_HEAD,
    MERGE_PIPELINE,
    MERGE_SUITE,
    MERGE_RETIRED,
];

/// Serialized retirement context written by [`start_merge`] and read by
/// [`continue_merge`]. Persists the `--retire <metric>...` and
/// `--retire-reason <text>` arguments so a long-running, conflict-laden
/// merge ends with the same attributed `review` node a single-shot
/// merge would have produced (paper §4.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeRetirement {
    /// Metric names being retired, in user-supplied order.
    pub metrics: Vec<String>,
    /// Optional reason text. `None` causes [`merge::ensure_review_node_for_retirement`]
    /// to fall back to the default placeholder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ── primitives ──────────────────────────────────────────────────────

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), MorphError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|s| s.to_str()).unwrap_or("")
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn read_or_none(path: &Path) -> Result<Option<Vec<u8>>, MorphError> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(std::fs::read(path)?))
}

fn write_hash(morph_dir: &Path, name: &str, hash: &Hash) -> Result<(), MorphError> {
    let mut s = hash.to_string();
    s.push('\n');
    write_atomic(&morph_dir.join(name), s.as_bytes())
}

fn read_hash(morph_dir: &Path, name: &str) -> Result<Option<Hash>, MorphError> {
    let bytes = match read_or_none(&morph_dir.join(name))? {
        Some(b) => b,
        None => return Ok(None),
    };
    let s = std::str::from_utf8(&bytes)
        .map_err(|e| MorphError::Serialization(format!("invalid utf-8 in {}: {}", name, e)))?
        .trim();
    if s.is_empty() {
        return Ok(None);
    }
    Hash::from_hex(s)
        .map(Some)
        .map_err(|_| MorphError::InvalidHash(s.to_string()))
}

// ── public API ───────────────────────────────────────────────────────

pub fn write_merge_head(morph_dir: &Path, hash: &Hash) -> Result<(), MorphError> {
    write_hash(morph_dir, MERGE_HEAD, hash)
}
pub fn read_merge_head(morph_dir: &Path) -> Result<Option<Hash>, MorphError> {
    read_hash(morph_dir, MERGE_HEAD)
}

pub fn write_merge_msg(morph_dir: &Path, msg: &str) -> Result<(), MorphError> {
    write_atomic(&morph_dir.join(MERGE_MSG), msg.as_bytes())
}
pub fn read_merge_msg(morph_dir: &Path) -> Result<Option<String>, MorphError> {
    let bytes = match read_or_none(&morph_dir.join(MERGE_MSG))? {
        Some(b) => b,
        None => return Ok(None),
    };
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|e| MorphError::Serialization(format!("invalid utf-8 in MERGE_MSG: {}", e)))
}

pub fn write_orig_head(morph_dir: &Path, hash: &Hash) -> Result<(), MorphError> {
    write_hash(morph_dir, ORIG_HEAD, hash)
}
pub fn read_orig_head(morph_dir: &Path) -> Result<Option<Hash>, MorphError> {
    read_hash(morph_dir, ORIG_HEAD)
}

pub fn write_merge_pipeline(morph_dir: &Path, pipeline: &Pipeline) -> Result<(), MorphError> {
    let json = serde_json::to_vec_pretty(pipeline)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    write_atomic(&morph_dir.join(MERGE_PIPELINE), &json)
}
pub fn read_merge_pipeline(morph_dir: &Path) -> Result<Option<Pipeline>, MorphError> {
    let bytes = match read_or_none(&morph_dir.join(MERGE_PIPELINE))? {
        Some(b) => b,
        None => return Ok(None),
    };
    let p: Pipeline =
        serde_json::from_slice(&bytes).map_err(|e| MorphError::Serialization(e.to_string()))?;
    Ok(Some(p))
}

pub fn write_merge_suite(morph_dir: &Path, hash: &Hash) -> Result<(), MorphError> {
    write_hash(morph_dir, MERGE_SUITE, hash)
}
pub fn read_merge_suite(morph_dir: &Path) -> Result<Option<Hash>, MorphError> {
    read_hash(morph_dir, MERGE_SUITE)
}

pub fn write_merge_retirement(
    morph_dir: &Path,
    retirement: &MergeRetirement,
) -> Result<(), MorphError> {
    let bytes = serde_json::to_vec_pretty(retirement)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    write_atomic(&morph_dir.join(MERGE_RETIRED), &bytes)
}

pub fn read_merge_retirement(
    morph_dir: &Path,
) -> Result<Option<MergeRetirement>, MorphError> {
    let bytes = match read_or_none(&morph_dir.join(MERGE_RETIRED))? {
        Some(b) => b,
        None => return Ok(None),
    };
    let r: MergeRetirement = serde_json::from_slice(&bytes)
        .map_err(|e| MorphError::Serialization(e.to_string()))?;
    Ok(Some(r))
}

/// Remove every merge-state file. Tolerates missing files. Used by
/// `morph merge --abort` and `morph merge --continue` (after the merge
/// commit is created).
pub fn clear_merge_state(morph_dir: &Path) -> Result<(), MorphError> {
    for name in ALL_FILES {
        let path = morph_dir.join(name);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// Returns `true` when `MERGE_HEAD` exists. Treated as the canonical
/// in-progress signal — other files may or may not be present.
pub fn merge_in_progress(morph_dir: &Path) -> bool {
    morph_dir.join(MERGE_HEAD).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{Pipeline, PipelineGraph, PipelineNode};
    use std::collections::BTreeMap;

    fn dummy_hash(byte: u8) -> Hash {
        Hash::from_hex(&format!("{:02x}", byte).repeat(32)).unwrap()
    }

    fn make_morph_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".morph")).unwrap();
        dir
    }

    #[test]
    fn merge_state_head_msg_orig_roundtrip() {
        let dir = make_morph_dir();
        let m = dir.path().join(".morph");

        // missing → None
        assert!(read_merge_head(&m).unwrap().is_none());
        assert!(read_merge_msg(&m).unwrap().is_none());
        assert!(read_orig_head(&m).unwrap().is_none());

        let h_head = dummy_hash(0xab);
        let h_orig = dummy_hash(0xcd);
        write_merge_head(&m, &h_head).unwrap();
        write_merge_msg(&m, "Merge branch 'feature'").unwrap();
        write_orig_head(&m, &h_orig).unwrap();

        assert_eq!(read_merge_head(&m).unwrap(), Some(h_head));
        assert_eq!(
            read_merge_msg(&m).unwrap().as_deref(),
            Some("Merge branch 'feature'")
        );
        assert_eq!(read_orig_head(&m).unwrap(), Some(h_orig));
    }

    #[test]
    fn merge_state_pipeline_json_roundtrip() {
        let dir = make_morph_dir();
        let m = dir.path().join(".morph");

        let pipeline = Pipeline {
            graph: PipelineGraph {
                nodes: vec![PipelineNode {
                    id: "a".into(),
                    kind: "prompt_call".into(),
                    ref_: None,
                    params: BTreeMap::new(),
                    env: None,
                }],
                edges: vec![],
            },
            prompts: vec!["p1".into()],
            eval_suite: None,
            attribution: None,
            provenance: None,
        };

        assert!(read_merge_pipeline(&m).unwrap().is_none());
        write_merge_pipeline(&m, &pipeline).unwrap();
        let loaded = read_merge_pipeline(&m).unwrap().unwrap();
        assert_eq!(loaded, pipeline);
    }

    #[test]
    fn merge_state_suite_hash_roundtrip() {
        let dir = make_morph_dir();
        let m = dir.path().join(".morph");

        assert!(read_merge_suite(&m).unwrap().is_none());
        let h = dummy_hash(0x77);
        write_merge_suite(&m, &h).unwrap();
        assert_eq!(read_merge_suite(&m).unwrap(), Some(h));
    }

    #[test]
    fn merge_state_retirement_breadcrumb_roundtrip() {
        let dir = make_morph_dir();
        let m = dir.path().join(".morph");

        assert!(read_merge_retirement(&m).unwrap().is_none());

        let r = MergeRetirement {
            metrics: vec!["old_metric".into(), "stale".into()],
            reason: Some("switching retrieval strategy".into()),
        };
        write_merge_retirement(&m, &r).unwrap();
        assert_eq!(read_merge_retirement(&m).unwrap(), Some(r));

        let r_no_reason = MergeRetirement {
            metrics: vec!["only_one".into()],
            reason: None,
        };
        write_merge_retirement(&m, &r_no_reason).unwrap();
        assert_eq!(read_merge_retirement(&m).unwrap(), Some(r_no_reason));
    }

    #[test]
    fn clear_merge_state_removes_all_files_and_in_progress_flag() {
        let dir = make_morph_dir();
        let m = dir.path().join(".morph");

        let h = dummy_hash(0x01);
        write_merge_head(&m, &h).unwrap();
        write_merge_msg(&m, "x").unwrap();
        write_orig_head(&m, &h).unwrap();
        write_merge_suite(&m, &h).unwrap();
        write_merge_pipeline(
            &m,
            &Pipeline {
                graph: PipelineGraph { nodes: vec![], edges: vec![] },
                prompts: vec![],
                eval_suite: None,
                attribution: None,
                provenance: None,
            },
        )
        .unwrap();
        write_merge_retirement(
            &m,
            &MergeRetirement {
                metrics: vec!["old".into()],
                reason: Some("reason".into()),
            },
        )
        .unwrap();

        assert!(merge_in_progress(&m), "MERGE_HEAD must signal in-progress");
        clear_merge_state(&m).unwrap();
        assert!(!merge_in_progress(&m), "in-progress flag must clear");
        assert!(read_merge_head(&m).unwrap().is_none());
        assert!(read_merge_msg(&m).unwrap().is_none());
        assert!(read_orig_head(&m).unwrap().is_none());
        assert!(read_merge_suite(&m).unwrap().is_none());
        assert!(read_merge_pipeline(&m).unwrap().is_none());
        assert!(
            read_merge_retirement(&m).unwrap().is_none(),
            "MERGE_RETIRED must be cleared by clear_merge_state"
        );

        // Calling clear again on a clean repo must be a no-op.
        clear_merge_state(&m).unwrap();
    }
}
