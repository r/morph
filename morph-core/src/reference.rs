//! Reference mode: Morph sits alongside Git.
//!
//! In reference mode, every git commit is mirrored into a Morph commit
//! with `morph_origin = Some("git-hook")` and `git_origin_sha` set to
//! the source git SHA. The mirror happens via `morph reference-sync`,
//! either invoked manually or triggered by the post-commit hook
//! installed by `morph init --reference`.
//!
//! Mirrored commits start with empty inline `observed_metrics`. Late
//! certification (the unified model from PR 1) attaches evidence after
//! tests run, satisfying policy gates without ever blocking the user's
//! `git commit`.
//!
//! This module owns:
//!   - subprocess wrappers for `git rev-parse`, `git log`
//!   - the sync logic (`sync_to_head`)
//!   - the pending-certification helper used by `morph status`
//!   - the post-commit hook script template

use crate::hash::Hash;
use crate::objects::{Commit, EvalContract, MorphObject};
use crate::store::{MorphError, Store};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// Embedded post-commit hook script. Installed at `.git/hooks/post-commit`
/// by `morph init --reference`. Failure of `morph reference-sync` is
/// swallowed so it never blocks a `git commit` — the user can re-run
/// sync manually or fix the underlying issue without losing their
/// commit.
pub const POST_COMMIT_HOOK_SCRIPT: &str = r#"#!/bin/sh
# Installed by `morph init --reference`. Mirrors every git commit into a
# Morph commit with morph_origin=git-hook, no inline metrics. Late
# certification via `morph certify` attaches evidence afterwards.
exec morph reference-sync >/dev/null 2>&1 || true
"#;

/// True when the path is the working tree of a git repository (a `.git`
/// directory or file exists). We accept both a directory and a file
/// (git worktrees use a file pointing at the real gitdir) so the check
/// is liberal enough for the common cases.
pub fn is_git_working_tree(repo_root: &Path) -> bool {
    let dot_git = repo_root.join(".git");
    dot_git.exists()
}

/// Resolve `git rev-parse HEAD` for the working tree at `repo_root`.
/// Returns `Ok(None)` for an empty repo (no commits yet) so callers
/// can still record `init_at_git_sha = null` cleanly.
pub fn git_head_sha(repo_root: &Path) -> Result<Option<String>, MorphError> {
    let out = Command::new("git")
        .arg("rev-parse")
        .arg("--verify")
        .arg("HEAD")
        .current_dir(repo_root)
        .output()
        .map_err(|e| MorphError::Other(format!("git rev-parse failed to spawn: {}", e)))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("unknown revision") || stderr.contains("Needed a single revision") {
            return Ok(None);
        }
        return Err(MorphError::Other(format!(
            "git rev-parse failed: {}",
            stderr.trim()
        )));
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

/// Read a single git commit's subject + body and author identity in
/// the form `Name <email>` plus the RFC 3339-ish committer timestamp.
pub struct GitCommitInfo {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub timestamp: String,
}

pub fn read_git_commit(repo_root: &Path, sha: &str) -> Result<GitCommitInfo, MorphError> {
    let mut cmd = Command::new("git");
    cmd.arg("log")
        .arg("-1")
        .arg("--format=%H%n%aN <%aE>%n%aI%n%B")
        .arg(sha)
        .current_dir(repo_root);
    let out = cmd
        .output()
        .map_err(|e| MorphError::Other(format!("git log failed to spawn: {}", e)))?;
    if !out.status.success() {
        return Err(MorphError::Other(format!(
            "git log failed for {}: {}",
            sha,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let mut lines = raw.splitn(4, '\n');
    let sha = lines
        .next()
        .ok_or_else(|| MorphError::Other("git log produced no output".into()))?
        .trim()
        .to_string();
    let author = lines
        .next()
        .ok_or_else(|| MorphError::Other("git log: missing author line".into()))?
        .trim()
        .to_string();
    let timestamp = lines
        .next()
        .ok_or_else(|| MorphError::Other("git log: missing timestamp line".into()))?
        .trim()
        .to_string();
    let message = lines.next().unwrap_or("").trim_end().to_string();
    Ok(GitCommitInfo {
        sha,
        message,
        author,
        timestamp,
    })
}

/// Outcome of a `sync_to_head` invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Hash of the new Morph commit (`None` when sync was a no-op).
    pub new_commit: Option<Hash>,
    /// Git SHA the sync targeted (HEAD at the moment sync ran). Always
    /// populated even for no-op syncs so callers can report progress.
    pub git_sha: Option<String>,
    /// True when there was nothing to do (Morph already at this git
    /// SHA, or the git repo has no commits yet).
    pub already_synced: bool,
}

/// Mirror the git working tree's HEAD into a Morph commit.
///
/// - Reads `git rev-parse HEAD`.
/// - If the current Morph HEAD already has `git_origin_sha` matching
///   the git SHA, returns `already_synced = true` without writing.
/// - Otherwise creates a new Morph commit with:
///     - `morph_origin = Some("git-hook")`
///     - `git_origin_sha = Some(<git_sha>)`
///     - `tree = None` (file storage stays in git)
///     - `pipeline = identity_pipeline()` (placeholder until the
///       pipeline graph is meaningful in reference mode)
///     - `eval_contract.observed_metrics = {}` (late certification
///       attaches evidence)
///     - parent = previous Morph HEAD (if any).
pub fn sync_to_head(
    store: &dyn Store,
    repo_root: &Path,
    morph_version: Option<&str>,
) -> Result<SyncOutcome, MorphError> {
    let git_sha = match git_head_sha(repo_root)? {
        Some(s) => s,
        None => {
            return Ok(SyncOutcome {
                new_commit: None,
                git_sha: None,
                already_synced: true,
            });
        }
    };

    if let Some(head) = crate::commit::resolve_head(store)? {
        if let MorphObject::Commit(c) = store.get(&head)? {
            if c.git_origin_sha.as_deref() == Some(git_sha.as_str()) {
                return Ok(SyncOutcome {
                    new_commit: None,
                    git_sha: Some(git_sha),
                    already_synced: true,
                });
            }
        }
    }

    let info = read_git_commit(repo_root, &git_sha)?;

    let identity = crate::identity::identity_pipeline();
    let pipeline_hash = store.put(&identity)?;
    let empty_suite = MorphObject::EvalSuite(crate::objects::EvalSuite {
        cases: vec![],
        metrics: vec![],
    });
    let suite_hash = store.put(&empty_suite)?;

    let parent_list: Vec<String> = crate::commit::resolve_head(store)?
        .map(|h| vec![h.to_string()])
        .unwrap_or_default();

    let morph_dir = repo_root.join(".morph");
    let commit = MorphObject::Commit(Commit {
        tree: None,
        pipeline: pipeline_hash.to_string(),
        parents: parent_list,
        message: info.message,
        timestamp: info.timestamp,
        author: info.author,
        contributors: None,
        eval_contract: EvalContract {
            suite: suite_hash.to_string(),
            observed_metrics: BTreeMap::new(),
        },
        env_constraints: None,
        evidence_refs: None,
        morph_version: morph_version.map(String::from),
        morph_instance: crate::agent::read_instance_id(&morph_dir)?,
        morph_origin: Some("git-hook".into()),
        git_origin_sha: Some(info.sha.clone()),
    });

    let hash = store.put(&commit)?;

    let branch = crate::commit::current_branch(store)?
        .unwrap_or_else(|| crate::commit::DEFAULT_BRANCH.to_string());
    store.ref_write(&format!("heads/{}", branch), &hash)?;

    Ok(SyncOutcome {
        new_commit: Some(hash),
        git_sha: Some(info.sha),
        already_synced: false,
    })
}

/// Count git-hook-origin commits reachable from HEAD whose effective
/// metrics (PR 1) are still empty. Used by `morph status` in reference
/// mode to surface the "uncertified git commits" lifecycle state.
///
/// Walks the parent chain from HEAD and stops at the first commit
/// without `morph_origin == "git-hook"` so older Morph-authored
/// commits don't count.
pub fn pending_certifications(
    store: &dyn Store,
    head: &Hash,
) -> Result<Vec<Hash>, MorphError> {
    let mut pending = Vec::new();
    let mut cursor = Some(head.clone());
    while let Some(h) = cursor {
        let obj = store.get(&h)?;
        let c = match obj {
            MorphObject::Commit(c) => c,
            _ => break,
        };
        let is_git_hook = c.morph_origin.as_deref() == Some("git-hook");
        if !is_git_hook {
            break;
        }
        let effective = crate::policy::effective_metrics(store, &h)?;
        if effective.is_empty() {
            pending.push(h.clone());
        }
        cursor = c
            .parents
            .first()
            .and_then(|p| Hash::from_hex(p).ok());
    }
    Ok(pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_repo() -> (tempfile::TempDir, Box<dyn Store>) {
        let dir = tempfile::tempdir().unwrap();
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        (dir, store)
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "tester")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "tester")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .expect("git invoke failed");
        assert!(status.success(), "git {:?} failed", args);
    }

    #[test]
    fn is_git_working_tree_detects_dot_git() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_git_working_tree(dir.path()));
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert!(is_git_working_tree(dir.path()));
    }

    #[test]
    fn git_head_sha_none_for_empty_repo() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        let sha = git_head_sha(dir.path()).unwrap();
        assert_eq!(sha, None);
    }

    #[test]
    fn git_head_sha_resolves_after_commit() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "first"]);
        let sha = git_head_sha(dir.path()).unwrap().expect("sha");
        assert_eq!(sha.len(), 40);
    }

    #[test]
    fn sync_to_head_creates_git_hook_commit() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(
            dir.path(),
            &["commit", "--allow-empty", "-q", "-m", "first git commit"],
        );
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        let outcome = sync_to_head(store.as_ref(), dir.path(), Some("0.24.0")).unwrap();
        assert!(!outcome.already_synced);
        let hash = outcome.new_commit.expect("new commit");
        let obj = store.get(&hash).unwrap();
        match obj {
            MorphObject::Commit(c) => {
                assert_eq!(c.morph_origin.as_deref(), Some("git-hook"));
                assert_eq!(c.git_origin_sha.unwrap().len(), 40);
                assert_eq!(c.message.trim(), "first git commit");
                assert!(c.eval_contract.observed_metrics.is_empty());
            }
            _ => panic!("expected commit"),
        }
    }

    #[test]
    fn sync_to_head_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "first"]);
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        let first = sync_to_head(store.as_ref(), dir.path(), None).unwrap();
        assert!(!first.already_synced);
        let second = sync_to_head(store.as_ref(), dir.path(), None).unwrap();
        assert!(second.already_synced);
        assert_eq!(second.new_commit, None);
    }

    #[test]
    fn sync_to_head_appends_for_new_git_commits() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "first"]);
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        let first = sync_to_head(store.as_ref(), dir.path(), None).unwrap();
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "second"]);
        let second = sync_to_head(store.as_ref(), dir.path(), None).unwrap();
        assert!(!second.already_synced);
        let new = second.new_commit.unwrap();
        assert_ne!(Some(new.clone()), first.new_commit);
        match store.get(&new).unwrap() {
            MorphObject::Commit(c) => {
                assert_eq!(c.parents.len(), 1);
                assert_eq!(c.message.trim(), "second");
            }
            _ => panic!("expected commit"),
        }
    }

    #[test]
    fn pending_certifications_counts_uncertified_git_hook_commits() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "a"]);
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        let first = sync_to_head(store.as_ref(), dir.path(), None)
            .unwrap()
            .new_commit
            .unwrap();
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "b"]);
        let second = sync_to_head(store.as_ref(), dir.path(), None)
            .unwrap()
            .new_commit
            .unwrap();
        let pending = pending_certifications(store.as_ref(), &second).unwrap();
        assert_eq!(pending.len(), 2, "expected 2 uncertified git-hook commits");
        assert!(pending.contains(&first));
        assert!(pending.contains(&second));
    }

    #[test]
    fn pending_certifications_drops_after_certify() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "a"]);
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        let first = sync_to_head(store.as_ref(), dir.path(), None)
            .unwrap()
            .new_commit
            .unwrap();
        let mut metrics = BTreeMap::new();
        metrics.insert("pass_rate".to_string(), 1.0);
        let morph_dir = dir.path().join(".morph");
        let _ = crate::policy::certify_commit(
            store.as_ref(),
            &morph_dir,
            &first,
            &metrics,
            None,
            None,
        )
        .unwrap();
        let pending = pending_certifications(store.as_ref(), &first).unwrap();
        assert!(pending.is_empty(), "certified commit should not be pending");
    }

    #[test]
    fn pending_certifications_stops_at_non_git_hook_parent() {
        let (dir, store) = setup_repo();
        // First commit is morph-authored (no git-hook origin) — should
        // terminate the walk.
        let suite = crate::objects::EvalSuite {
            cases: vec![],
            metrics: vec![],
        };
        let suite_hash = store.put(&MorphObject::EvalSuite(suite)).unwrap();
        let pipe = crate::identity::identity_pipeline();
        let pipe_hash = store.put(&pipe).unwrap();
        let cli_commit = MorphObject::Commit(Commit {
            tree: None,
            pipeline: pipe_hash.to_string(),
            parents: vec![],
            message: "morph cli".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            author: "test".into(),
            contributors: None,
            eval_contract: EvalContract {
                suite: suite_hash.to_string(),
                observed_metrics: BTreeMap::new(),
            },
            env_constraints: None,
            evidence_refs: None,
            morph_version: None,
            morph_instance: None,
            morph_origin: None,
            git_origin_sha: None,
        });
        let cli_hash = store.put(&cli_commit).unwrap();
        let hook_commit = MorphObject::Commit(Commit {
            tree: None,
            pipeline: pipe_hash.to_string(),
            parents: vec![cli_hash.to_string()],
            message: "git mirror".into(),
            timestamp: "2026-01-02T00:00:00Z".into(),
            author: "test".into(),
            contributors: None,
            eval_contract: EvalContract {
                suite: suite_hash.to_string(),
                observed_metrics: BTreeMap::new(),
            },
            env_constraints: None,
            evidence_refs: None,
            morph_version: None,
            morph_instance: None,
            morph_origin: Some("git-hook".into()),
            git_origin_sha: Some("a".repeat(40)),
        });
        let hook_hash = store.put(&hook_commit).unwrap();
        let pending = pending_certifications(store.as_ref(), &hook_hash).unwrap();
        assert_eq!(pending, vec![hook_hash]);
        let _ = dir;
    }
}
