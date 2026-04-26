//! Text 3-way merge primitive that delegates to `git merge-file`.
//!
//! Used as the leaf for [`crate::treemerge`] when both branches modify the
//! same path's content differently. Shelling out to `git merge-file` is the
//! shortest path to a battle-tested merge algorithm; if `git` is missing on
//! `PATH` we return a structured error so the CLI layer can render a clear
//! "install git" message.

use crate::store::MorphError;
use std::process::{Command, Stdio};

/// Outcome of a 3-way text merge.
#[derive(Clone, Debug)]
pub enum TextMergeResult {
    /// Merge produced clean output with no markers.
    Clean(Vec<u8>),
    /// Merge had at least one conflict region; output contains git-style
    /// `<<<<<<<` / `=======` / `>>>>>>>` markers.
    Conflict { content_with_markers: Vec<u8> },
}

/// Labels embedded into conflict markers so users can identify each side.
#[derive(Clone, Debug)]
pub struct TextMergeLabels {
    pub base: String,
    pub ours: String,
    pub theirs: String,
}

impl Default for TextMergeLabels {
    fn default() -> Self {
        Self {
            base: "base".to_string(),
            ours: "HEAD".to_string(),
            theirs: "MERGE_HEAD".to_string(),
        }
    }
}

/// Run `git merge-file -p -L ours -L base -L theirs ours base theirs`
/// using temp files. `base = None` is treated as an empty file (used for
/// add/add merges where both sides introduce the same path independently).
pub fn merge_text(
    base: Option<&[u8]>,
    ours: &[u8],
    theirs: &[u8],
    labels: TextMergeLabels,
) -> Result<TextMergeResult, MorphError> {
    let dir = tempfile::tempdir()?;
    let base_path = dir.path().join("base");
    let ours_path = dir.path().join("ours");
    let theirs_path = dir.path().join("theirs");

    std::fs::write(&base_path, base.unwrap_or(&[]))?;
    std::fs::write(&ours_path, ours)?;
    std::fs::write(&theirs_path, theirs)?;

    let output = Command::new("git")
        .arg("merge-file")
        .arg("-p")
        .arg("-L")
        .arg(&labels.ours)
        .arg("-L")
        .arg(&labels.base)
        .arg("-L")
        .arg(&labels.theirs)
        .arg(&ours_path)
        .arg(&base_path)
        .arg(&theirs_path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                MorphError::Serialization(
                    "git binary not found on PATH; install git to enable text merge".to_string(),
                )
            } else {
                MorphError::Io(e)
            }
        })?;

    // git merge-file exit code:
    //   0  → clean merge
    //   >0 → number of conflict regions
    //   <0 → error
    let exit = output.status.code().unwrap_or(-1);
    if exit < 0 {
        return Err(MorphError::Serialization(format!(
            "git merge-file failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    if exit == 0 {
        Ok(TextMergeResult::Clean(output.stdout))
    } else {
        Ok(TextMergeResult::Conflict {
            content_with_markers: output.stdout,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unwrap_clean(r: TextMergeResult) -> Vec<u8> {
        match r {
            TextMergeResult::Clean(b) => b,
            TextMergeResult::Conflict { content_with_markers } => panic!(
                "expected clean merge, got conflict:\n{}",
                String::from_utf8_lossy(&content_with_markers)
            ),
        }
    }

    fn unwrap_conflict(r: TextMergeResult) -> Vec<u8> {
        match r {
            TextMergeResult::Conflict { content_with_markers } => content_with_markers,
            TextMergeResult::Clean(b) => panic!(
                "expected conflict, got clean merge:\n{}",
                String::from_utf8_lossy(&b)
            ),
        }
    }

    #[test]
    fn merge_text_no_conflict_returns_clean() {
        // ours edits line 1, theirs edits line 3 — non-overlapping → clean.
        let base = b"line1\nline2\nline3\n";
        let ours = b"LINE1\nline2\nline3\n";
        let theirs = b"line1\nline2\nLINE3\n";
        let result = merge_text(Some(base), ours, theirs, TextMergeLabels::default()).unwrap();
        let bytes = unwrap_clean(result);
        assert_eq!(String::from_utf8_lossy(&bytes), "LINE1\nline2\nLINE3\n");
    }

    #[test]
    fn merge_text_with_conflict_returns_markers() {
        // Both sides edit line 2 differently → conflict.
        let base = b"line1\nline2\nline3\n";
        let ours = b"line1\nOURS_LINE2\nline3\n";
        let theirs = b"line1\nTHEIRS_LINE2\nline3\n";
        let result = merge_text(Some(base), ours, theirs, TextMergeLabels::default()).unwrap();
        let bytes = unwrap_conflict(result);
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("<<<<<<< HEAD"), "missing ours marker:\n{}", s);
        assert!(s.contains("======="), "missing separator:\n{}", s);
        assert!(s.contains(">>>>>>> MERGE_HEAD"), "missing theirs marker:\n{}", s);
        assert!(s.contains("OURS_LINE2"), "ours content missing:\n{}", s);
        assert!(s.contains("THEIRS_LINE2"), "theirs content missing:\n{}", s);
    }

    #[test]
    fn merge_text_identical_inputs_returns_unchanged() {
        let content = b"line1\nline2\nline3\n";
        let result = merge_text(
            Some(content),
            content,
            content,
            TextMergeLabels::default(),
        )
        .unwrap();
        let bytes = unwrap_clean(result);
        assert_eq!(bytes, content);
    }

    #[test]
    fn merge_text_handles_missing_base_for_add_add() {
        // Both sides "add" the same path with disjoint content. With no
        // common base, git merge-file uses an empty file as base. If the
        // contents share nothing, it produces a conflict. If they're
        // identical we'd get a clean merge of identical inputs.
        let ours = b"line1\nline2\n";
        let theirs = b"line1\nline2\n";
        let result = merge_text(None, ours, theirs, TextMergeLabels::default()).unwrap();
        let bytes = unwrap_clean(result);
        assert_eq!(bytes, ours, "identical add/add must merge clean");
    }
}
