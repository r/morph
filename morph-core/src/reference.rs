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
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::Command;

/// Embedded post-commit hook script. Installed at `.git/hooks/post-commit`
/// by `morph init --reference` and `morph install-hooks`. Mirrors every
/// git commit into a Morph commit with `morph_origin = "git-hook"` and
/// empty inline metrics. Late certification via `morph certify`
/// attaches evidence afterwards.
///
/// `MORPH_INTERNAL=1` short-circuits the hook so morph→git CLI
/// wrappers (PR 5) can drive `git commit` without a double-write —
/// the wrapper sets the env var, runs `git commit`, then writes the
/// morph commit itself with the right metadata.
///
/// Failures are swallowed so a hook problem never blocks a `git
/// commit`. The user fixes things up later with `morph reference-sync`.
pub const POST_COMMIT_HOOK_SCRIPT: &str = r#"#!/bin/sh
# Installed by morph (`morph init --reference` / `morph install-hooks`).
# Mirrors every git commit into a Morph commit with morph_origin=git-hook.
[ "$MORPH_INTERNAL" = "1" ] && exit 0
exec morph hook post-commit >/dev/null 2>&1 || true
"#;

/// Embedded post-checkout hook script. Receives three positional args
/// from git: previous HEAD, new HEAD, and a flag (1 = branch
/// checkout, 0 = file checkout). The handler ignores file checkouts
/// and, for branch switches, advances morph HEAD to the morph commit
/// whose `git_origin_sha` matches the new git HEAD — creating the
/// morph branch ref on the fly when git's current branch has no
/// matching morph ref yet.
pub const POST_CHECKOUT_HOOK_SCRIPT: &str = r#"#!/bin/sh
# Installed by morph. Tracks git's HEAD movement so the next morph
# commit lands on the right morph branch.
[ "$MORPH_INTERNAL" = "1" ] && exit 0
exec morph hook post-checkout "$@" >/dev/null 2>&1 || true
"#;

/// Embedded post-rewrite hook script. Receives one positional arg
/// (`amend` or `rebase`) and a stdin stream of `<old-sha> <new-sha>
/// [extra]` lines. The handler mirrors the new git history into morph
/// (so the branch ref advances onto the rewritten tip) and, for every
/// rewritten morph commit, attaches a `kind: "rewritten"` annotation
/// pointing at its successor — that's how stale `morph certify`
/// evidence is surfaced to consumers.
pub const POST_REWRITE_HOOK_SCRIPT: &str = r#"#!/bin/sh
# Installed by morph. Re-mirrors history after `git commit --amend`
# or `git rebase` and flags now-stale certifications via a
# `rewritten` annotation on each old commit.
[ "$MORPH_INTERNAL" = "1" ] && exit 0
exec morph hook post-rewrite "$@" >/dev/null 2>&1 || true
"#;

/// Embedded post-merge hook script. Fires after `git merge` (incl. a
/// non-fast-forward `git pull`) once the merge commit has been
/// recorded. Pure fast-forward pulls fire no git hook at all — that
/// drift is surfaced separately by `morph status` in reference mode.
pub const POST_MERGE_HOOK_SCRIPT: &str = r#"#!/bin/sh
# Installed by morph. Mirrors merge commits into morph history so the
# morph branch ref keeps pace with `git merge` / non-FF `git pull`.
[ "$MORPH_INTERNAL" = "1" ] && exit 0
exec morph hook post-merge "$@" >/dev/null 2>&1 || true
"#;

/// Filename → contents for every reference-mode hook this binary
/// installs. Iterating this list keeps `install_reference_hooks`,
/// `morph init --reference`, and the spec-test assertions in lock-step
/// without copy/paste.
pub fn reference_mode_hooks() -> &'static [(&'static str, &'static str)] {
    &[
        ("post-commit", POST_COMMIT_HOOK_SCRIPT),
        ("post-checkout", POST_CHECKOUT_HOOK_SCRIPT),
        ("post-rewrite", POST_REWRITE_HOOK_SCRIPT),
        ("post-merge", POST_MERGE_HOOK_SCRIPT),
    ]
}

/// Append `.morph/` to `.git/info/exclude` (creating the file when
/// missing) so a stray `git add .` can't pull morph state into the
/// shared repo. This is the Stowaway-mode safety net: the morph user's
/// clone gains a *local* exclude that teammates' clones never receive,
/// so their git workflow is unaffected.
///
/// Idempotent. Returns `true` if a new line was written; `false` when
/// `.morph/` was already present.
pub fn ensure_morph_in_git_info_exclude(repo_root: &Path) -> Result<bool, MorphError> {
    let info_dir = repo_root.join(".git").join("info");
    if !info_dir.exists() {
        std::fs::create_dir_all(&info_dir).map_err(|e| {
            MorphError::Other(format!("create .git/info: {}", e))
        })?;
    }
    let exclude_path = info_dir.join("exclude");
    let existing = if exclude_path.exists() {
        std::fs::read_to_string(&exclude_path)
            .map_err(|e| MorphError::Other(format!("read .git/info/exclude: {}", e)))?
    } else {
        String::new()
    };
    let target = ".morph/";
    let already = existing.lines().any(|l| l.trim() == target);
    if already {
        return Ok(false);
    }
    let mut new_content = existing.clone();
    if !new_content.is_empty() && !new_content.ends_with('\n') {
        new_content.push('\n');
    }
    if existing.is_empty() {
        new_content
            .push_str("# Local-only excludes managed by morph (Stowaway mode):\n");
        new_content
            .push_str("# teammates' clones never see this file, so their git workflow is unaffected.\n");
    }
    new_content.push_str(target);
    new_content.push('\n');
    std::fs::write(&exclude_path, new_content)
        .map_err(|e| MorphError::Other(format!("write .git/info/exclude: {}", e)))?;
    Ok(true)
}

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

/// Read the parent SHAs of a git commit, in the order git stores them
/// (first parent is the mainline). For an octopus merge there will be
/// more than two; for the root commit the vec is empty.
pub fn git_parents(repo_root: &Path, sha: &str) -> Result<Vec<String>, MorphError> {
    let out = Command::new("git")
        .arg("rev-list")
        .arg("--parents")
        .arg("-n")
        .arg("1")
        .arg(sha)
        .current_dir(repo_root)
        .output()
        .map_err(|e| MorphError::Other(format!("git rev-list failed to spawn: {}", e)))?;
    if !out.status.success() {
        return Err(MorphError::Other(format!(
            "git rev-list failed for {}: {}",
            sha,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    parts.next();
    Ok(parts.map(String::from).collect())
}

/// Topologically ordered list of git commit SHAs reachable from `to_sha`
/// but stopping at (and including) `from_sha`. The earliest commit in
/// the returned vec is `from_sha`, the latest is `to_sha`. When
/// `from_sha` is `None`, returns the entire history reachable from
/// `to_sha`. Used by `morph reference-sync --backfill`.
pub fn git_log_range(
    repo_root: &Path,
    from_sha: Option<&str>,
    to_sha: &str,
) -> Result<Vec<String>, MorphError> {
    let mut cmd = Command::new("git");
    cmd.arg("log")
        .arg("--reverse")
        .arg("--topo-order")
        .arg("--format=%H");
    match from_sha {
        Some(from) => {
            cmd.arg(format!("{}^..{}", from, to_sha));
        }
        None => {
            cmd.arg(to_sha);
        }
    }
    let out = cmd
        .current_dir(repo_root)
        .output()
        .map_err(|e| MorphError::Other(format!("git log failed to spawn: {}", e)))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // `<root>^..HEAD` fails when from_sha is the very first commit
        // (no parent of root). Fall back to walking everything from
        // HEAD in that case — harmless because backfill skips already
        // mirrored commits.
        if stderr.contains("unknown revision") || stderr.contains("Needed a single revision") {
            return git_log_range(repo_root, None, to_sha);
        }
        return Err(MorphError::Other(format!(
            "git log range failed: {}",
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect())
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

/// Walk every commit reachable from any branch and build a
/// `git_origin_sha → morph_hash` cache. Used by sync to resolve git
/// parents to the morph commits that mirror them; without this we
/// couldn't reconstruct multi-parent merge commits across branches.
fn build_git_to_morph_cache(store: &dyn Store) -> Result<HashMap<String, Hash>, MorphError> {
    let mut cache: HashMap<String, Hash> = HashMap::new();
    let mut visited: std::collections::HashSet<Hash> = std::collections::HashSet::new();
    let mut frontier: Vec<Hash> = Vec::new();
    for (_name, hash) in store.list_branches()? {
        frontier.push(hash);
    }
    while let Some(h) = frontier.pop() {
        if !visited.insert(h) {
            continue;
        }
        let obj = match store.get(&h) {
            Ok(o) => o,
            Err(_) => continue,
        };
        if let MorphObject::Commit(c) = obj {
            if let Some(git_sha) = c.git_origin_sha.as_ref() {
                cache.entry(git_sha.clone()).or_insert(h);
            }
            for p in &c.parents {
                if let Ok(ph) = Hash::from_hex(p) {
                    frontier.push(ph);
                }
            }
        }
    }
    Ok(cache)
}

/// Mirror a single git commit identified by `git_sha` into a Morph
/// commit. Resolves parents via `cache` (built from existing morph
/// history); falls back to current morph HEAD as a single parent when
/// no git-parent-side mirror is available (the user is partly
/// backfilled).
///
/// Updates `cache` with the new mapping so a subsequent sync in the
/// same backfill loop can find this commit as a parent.
fn sync_one_commit(
    store: &dyn Store,
    repo_root: &Path,
    git_sha: &str,
    origin: &str,
    morph_version: Option<&str>,
    cache: &mut HashMap<String, Hash>,
) -> Result<Hash, MorphError> {
    let info = read_git_commit(repo_root, git_sha)?;
    let parent_shas = git_parents(repo_root, git_sha)?;

    let mut morph_parents: Vec<String> = Vec::new();
    let mut resolved_any = false;
    for p in &parent_shas {
        if let Some(h) = cache.get(p) {
            morph_parents.push(h.to_string());
            resolved_any = true;
        }
    }
    // Fallback: if git lists parents but none resolved (e.g. partial
    // backfill state), peg the new commit to current morph HEAD so
    // history stays connected. Better a slightly inaccurate parent than
    // a free-floating commit.
    if !parent_shas.is_empty() && !resolved_any {
        if let Some(head) = crate::commit::resolve_head(store)? {
            morph_parents.push(head.to_string());
        }
    }

    let identity = crate::identity::identity_pipeline();
    let pipeline_hash = store.put(&identity)?;
    let empty_suite = MorphObject::EvalSuite(crate::objects::EvalSuite {
        cases: vec![],
        metrics: vec![],
    });
    let suite_hash = store.put(&empty_suite)?;

    let morph_dir = repo_root.join(".morph");
    let commit = MorphObject::Commit(Commit {
        tree: None,
        pipeline: pipeline_hash.to_string(),
        parents: morph_parents,
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
        morph_origin: Some(origin.into()),
        git_origin_sha: Some(info.sha.clone()),
    });
    let hash = store.put(&commit)?;
    cache.insert(info.sha, hash);
    Ok(hash)
}

/// Mirror the git working tree's HEAD into a Morph commit.
///
/// - Reads `git rev-parse HEAD`.
/// - If a morph commit already has `git_origin_sha` matching the git
///   SHA, returns `already_synced = true`.
/// - Otherwise creates a new Morph commit. Parents are derived from
///   git's parents (resolved against existing morph history), so a
///   git merge commit becomes a multi-parent morph commit.
/// - Advances the current branch ref to the new commit.
pub fn sync_to_head(
    store: &dyn Store,
    repo_root: &Path,
    morph_version: Option<&str>,
) -> Result<SyncOutcome, MorphError> {
    sync_to_head_with_origin(store, repo_root, "git-hook", morph_version)
}

/// Same as [`sync_to_head`] but lets the caller stamp a non-default
/// `morph_origin` on the new commit. PR 5's `morph commit` wrapper uses
/// `"cli"` so policy + audit can distinguish CLI-driven commits from
/// passive post-commit hook mirrors.
pub fn sync_to_head_with_origin(
    store: &dyn Store,
    repo_root: &Path,
    origin: &str,
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

    let mut cache = build_git_to_morph_cache(store)?;
    if cache.contains_key(&git_sha) {
        return Ok(SyncOutcome {
            new_commit: None,
            git_sha: Some(git_sha),
            already_synced: true,
        });
    }

    let hash = sync_one_commit(store, repo_root, &git_sha, origin, morph_version, &mut cache)?;
    let branch = crate::commit::current_branch(store)?
        .unwrap_or_else(|| crate::commit::DEFAULT_BRANCH.to_string());
    store.ref_write(&format!("heads/{}", branch), &hash)?;

    Ok(SyncOutcome {
        new_commit: Some(hash),
        git_sha: Some(git_sha),
        already_synced: false,
    })
}

/// Run `git commit -m <message>` inside `repo_root` with
/// `MORPH_INTERNAL=1` exported so any installed Morph hooks
/// short-circuit. Used by `morph commit` in reference mode to
/// guarantee atomic git+morph creation: the wrapper runs git, then
/// performs the morph mirror itself with `morph_origin = "cli"`.
///
/// Returns the new git HEAD SHA on success. Bubbles up git's stderr
/// verbatim on failure so `nothing to commit`-style messages reach
/// the user.
///
/// `allow_empty` maps to git's `--allow-empty`. `author`, when
/// provided, maps to `git commit --author <author>`.
pub fn run_git_commit_with_morph_internal(
    repo_root: &Path,
    message: &str,
    allow_empty: bool,
    author: Option<&str>,
) -> Result<String, MorphError> {
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(repo_root)
        .arg("commit")
        .arg("-m")
        .arg(message)
        .env("MORPH_INTERNAL", "1");
    if allow_empty {
        cmd.arg("--allow-empty");
    }
    if let Some(a) = author {
        cmd.arg(format!("--author={}", a));
    }
    let output = cmd
        .output()
        .map_err(|e| MorphError::Other(format!("failed to spawn git commit: {}", e)))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let combined = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        return Err(MorphError::Other(format!(
            "git commit failed: {}",
            combined.trim()
        )));
    }
    git_head_sha(repo_root)?
        .ok_or_else(|| MorphError::Other("git HEAD missing after successful commit".into()))
}

/// Snapshot of the git/morph relationship in reference mode. Used by
/// `morph status`, `morph_eval_gaps`, and the merge gate (PR 7) to
/// answer "is morph in sync with git?"
#[derive(Debug, Clone, serde::Serialize)]
pub struct DriftSummary {
    /// `git rev-parse HEAD` at the moment of the call. `None` when the
    /// repo has no commits yet.
    pub git_head: Option<String>,
    /// Most recent ancestor of `git_head` that already has a morph
    /// counterpart. `None` when no ancestor is mirrored (a fresh
    /// `morph init --reference` with `init_at_git_sha` set but no sync
    /// yet — common for late adoption).
    pub last_mirrored_git_sha: Option<String>,
    /// Number of git commits between `last_mirrored_git_sha` and
    /// `git_head` (exclusive of the mirrored ancestor, inclusive of
    /// the unmirrored tip). `0` means morph is up to date with git.
    pub unmirrored_count: usize,
}

impl DriftSummary {
    pub fn is_up_to_date(&self) -> bool {
        self.unmirrored_count == 0
    }
}

/// Walk first-parent ancestry from `git HEAD` until we hit a commit
/// already mirrored into morph; the count of intervening commits is
/// the drift. Returns `unmirrored_count = 0` when git HEAD is itself
/// mirrored.
///
/// Hard-caps the walk at 10 000 commits to keep the call cheap on huge
/// histories. For typical Stowaway scenarios drift is < 100.
pub fn drift_summary(store: &dyn Store, repo_root: &Path) -> Result<DriftSummary, MorphError> {
    let git_head = git_head_sha(repo_root)?;
    let head = match git_head {
        Some(h) => h,
        None => {
            return Ok(DriftSummary {
                git_head: None,
                last_mirrored_git_sha: None,
                unmirrored_count: 0,
            });
        }
    };
    let cache = build_git_to_morph_cache(store)?;
    if cache.contains_key(&head) {
        return Ok(DriftSummary {
            git_head: Some(head.clone()),
            last_mirrored_git_sha: Some(head),
            unmirrored_count: 0,
        });
    }
    let mut count = 0usize;
    let mut current = head.clone();
    let mut last_mirrored: Option<String> = None;
    while count < 10_000 {
        count += 1;
        let parents = git_parents(repo_root, &current)?;
        let next = match parents.first() {
            Some(p) => p.clone(),
            None => break,
        };
        if cache.contains_key(&next) {
            last_mirrored = Some(next);
            break;
        }
        current = next;
    }
    Ok(DriftSummary {
        git_head: Some(head),
        last_mirrored_git_sha: last_mirrored,
        unmirrored_count: count,
    })
}

/// List morph commit hashes whose `kind: "certification"` annotations
/// have been invalidated by a subsequent `kind: "rewritten"`
/// annotation. PR 4 attaches `rewritten` whenever `git commit --amend`
/// or `git rebase` supersedes a commit; PR 6 surfaces the resulting
/// stale certifications in `morph status` and `morph_eval_gaps`.
pub fn list_stale_certifications(store: &dyn Store) -> Result<Vec<Hash>, MorphError> {
    use std::collections::HashSet;
    let annotation_hashes = store.list(crate::store::ObjectType::Annotation)?;
    let mut rewritten: HashSet<String> = HashSet::new();
    let mut certified: HashSet<String> = HashSet::new();
    for h in &annotation_hashes {
        if let Ok(MorphObject::Annotation(ann)) = store.get(h) {
            match ann.kind.as_str() {
                "rewritten" => {
                    rewritten.insert(ann.target.clone());
                }
                "certification" => {
                    certified.insert(ann.target.clone());
                }
                _ => {}
            }
        }
    }
    let mut intersected: Vec<&String> = rewritten.intersection(&certified).collect();
    intersected.sort();
    let out: Vec<Hash> = intersected
        .iter()
        .filter_map(|s| Hash::from_hex(s).ok())
        .collect();
    Ok(out)
}

/// Outcome of [`ensure_branch_synced`]. `created` is the number of new
/// Morph commits the call wrote; `branch_moved` is true when the
/// `heads/<branch>` morph ref had to be updated to point at a fresh
/// tip; `already_synced` is true when nothing changed (the morph
/// branch already pointed at the matching morph commit).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchSyncOutcome {
    /// Number of new morph commits created during the sync.
    pub created: usize,
    /// `true` when `heads/<branch>` was advanced (or freshly written).
    pub branch_moved: bool,
    /// Git tip SHA the morph branch now mirrors. `None` when the git
    /// branch does not exist (caller falls back to "branch not found").
    pub git_tip: Option<String>,
    /// Morph hash the branch ref now points to. `None` when the git
    /// branch does not exist.
    pub morph_tip: Option<Hash>,
    /// `true` when the morph branch already matched the git tip and no
    /// work was performed. Implies `created == 0 && !branch_moved`.
    pub already_synced: bool,
}

impl BranchSyncOutcome {
    /// `true` when the git branch did not exist (no auto-sync possible).
    /// Lets callers distinguish "git has the branch and we mirrored it"
    /// from "git doesn't have the branch either" without inspecting
    /// `git_tip`.
    pub fn missing_in_git(&self) -> bool {
        self.git_tip.is_none()
    }
}

/// Mirror git's `refs/heads/<branch>` into morph's `heads/<branch>`,
/// creating any missing morph commits in topological (parents-first)
/// order. Used by [`crate::merge::prepare_merge`] / `morph merge` in
/// reference mode so a teammate's git-only branch can still flow
/// through the merge gate without the user manually running
/// `morph reference-sync` first (PR 7, Stowaway mode).
///
/// Behavior:
/// - Returns an outcome with `git_tip = None` (no error) when the git
///   branch doesn't exist. Callers should fall through to whatever
///   "branch not found" handling already exists for the morph store.
/// - When the git branch tip is already mirrored, simply points the
///   morph branch ref at the existing morph commit (creating a fresh
///   `heads/<branch>` ref when needed).
/// - When unmirrored ancestors exist, walks `git log --reverse
///   --topo-order <tip>` and creates morph commits via the same
///   [`sync_one_commit`] path used by `sync_to_head`. New commits
///   carry `morph_origin = "git-hook"` so the policy carve-out
///   exempts them from required-metrics enforcement.
/// - Idempotent: a second call when nothing changed returns
///   `already_synced = true`.
pub fn ensure_branch_synced(
    store: &dyn Store,
    repo_root: &Path,
    branch_name: &str,
    morph_version: Option<&str>,
) -> Result<BranchSyncOutcome, MorphError> {
    if !is_git_working_tree(repo_root) {
        return Ok(BranchSyncOutcome::default());
    }

    let out = Command::new("git")
        .arg("rev-parse")
        .arg("--verify")
        .arg("--quiet")
        .arg(format!("refs/heads/{}", branch_name))
        .current_dir(repo_root)
        .output()
        .map_err(|e| MorphError::Other(format!("git rev-parse failed to spawn: {}", e)))?;
    if !out.status.success() {
        return Ok(BranchSyncOutcome::default());
    }
    let git_tip = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if git_tip.is_empty() {
        return Ok(BranchSyncOutcome::default());
    }

    let mut cache = build_git_to_morph_cache(store)?;
    let morph_ref = format!("heads/{}", branch_name);
    let current_morph_branch = store.ref_read(&morph_ref)?;

    if let Some(&existing_morph_tip) = cache.get(&git_tip) {
        let already_pointing = current_morph_branch == Some(existing_morph_tip);
        if !already_pointing {
            store.ref_write(&morph_ref, &existing_morph_tip)?;
        }
        return Ok(BranchSyncOutcome {
            created: 0,
            branch_moved: !already_pointing,
            git_tip: Some(git_tip),
            morph_tip: Some(existing_morph_tip),
            already_synced: already_pointing,
        });
    }

    let range = git_log_range(repo_root, None, &git_tip)?;
    let mut last_hash: Option<Hash> = None;
    let mut created = 0usize;
    for sha in &range {
        if let Some(&h) = cache.get(sha) {
            last_hash = Some(h);
            continue;
        }
        let hash = sync_one_commit(store, repo_root, sha, "git-hook", morph_version, &mut cache)?;
        last_hash = Some(hash);
        created += 1;
    }

    let morph_tip = last_hash.ok_or_else(|| {
        MorphError::Other(format!(
            "git log produced no commits for branch '{}' (tip {})",
            branch_name, git_tip
        ))
    })?;

    let branch_moved = current_morph_branch != Some(morph_tip);
    if branch_moved {
        store.ref_write(&morph_ref, &morph_tip)?;
    }

    Ok(BranchSyncOutcome {
        created,
        branch_moved,
        git_tip: Some(git_tip),
        morph_tip: Some(morph_tip),
        already_synced: created == 0 && !branch_moved,
    })
}

/// Backfill morph commits from `init_at_git_sha` (inclusive) up to the
/// current git HEAD. Skips git commits already mirrored. This is the
/// late-adoption path: a user who turned off the post-commit hook (or
/// who installed reference mode mid-history) calls
/// `morph reference-sync --backfill` to catch up.
///
/// Returns the number of new morph commits created. The branch ref is
/// advanced to the most recent mirrored commit when at least one is
/// created.
pub fn backfill_from_init(
    store: &dyn Store,
    repo_root: &Path,
    init_at_git_sha: Option<&str>,
    morph_version: Option<&str>,
) -> Result<usize, MorphError> {
    let head_sha = match git_head_sha(repo_root)? {
        Some(s) => s,
        None => return Ok(0),
    };
    let range = git_log_range(repo_root, init_at_git_sha, &head_sha)?;
    if range.is_empty() {
        return Ok(0);
    }

    let mut cache = build_git_to_morph_cache(store)?;
    let mut last_hash: Option<Hash> = None;
    let mut created = 0usize;
    for sha in &range {
        if cache.contains_key(sha) {
            continue;
        }
        let hash = sync_one_commit(store, repo_root, sha, "git-hook", morph_version, &mut cache)?;
        last_hash = Some(hash);
        created += 1;
    }
    if let Some(hash) = last_hash {
        let branch = crate::commit::current_branch(store)?
            .unwrap_or_else(|| crate::commit::DEFAULT_BRANCH.to_string());
        store.ref_write(&format!("heads/{}", branch), &hash)?;
    }
    Ok(created)
}

/// Outcome of `install_reference_hooks`. `installed` lists the hook
/// names that were newly written or rewritten; `already_present`
/// lists those that matched the canonical script byte-for-byte
/// already. The split is exposed so the CLI can print a sensible
/// message ("Installed N hook(s)" vs "already installed").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookInstallReport {
    pub installed: Vec<String>,
    pub already_present: Vec<String>,
}

impl HookInstallReport {
    pub fn changed(&self) -> bool {
        !self.installed.is_empty()
    }
}

/// Idempotently install all reference-mode git hooks into the working
/// tree. Errors when `repo_root` isn't a git working tree, or when a
/// hook file exists with foreign content (so we never clobber a
/// user-authored hook). A hook whose contents already match the
/// canonical script is left alone and reported as already-present.
///
/// Detection of "morph wrote this" relies on substring matches
/// against either the legacy script (`morph reference-sync`) or the
/// PR-4-and-later marker (`morph hook`). That tolerance lets
/// upgrades from older binaries succeed without forcing the user to
/// delete their hooks first.
pub fn install_reference_hooks(repo_root: &Path) -> Result<HookInstallReport, MorphError> {
    if !is_git_working_tree(repo_root) {
        return Err(MorphError::Other(
            "not a git working tree (.git missing)".into(),
        ));
    }
    let hooks_dir = repo_root.join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir)?;

    let mut report = HookInstallReport::default();
    for (name, canonical) in reference_mode_hooks() {
        let hook_path = hooks_dir.join(name);
        if hook_path.exists() {
            let existing = std::fs::read_to_string(&hook_path)?;
            if existing == *canonical {
                report.already_present.push((*name).into());
                continue;
            }
            // Tolerate older morph-authored scripts (the pre-PR4
            // post-commit stub called `morph reference-sync`
            // directly; the PR4-and-later stubs call `morph hook
            // <event>`). Anything without one of those markers is
            // assumed to be a user hook we mustn't clobber.
            let morph_authored =
                existing.contains("morph hook") || existing.contains("morph reference-sync");
            if !morph_authored {
                return Err(MorphError::Other(format!(
                    "{} hook exists with foreign content; refusing to overwrite \
                     (move it aside and re-run)",
                    name
                )));
            }
        }
        std::fs::write(&hook_path, canonical)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&hook_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&hook_path, perms)?;
        }
        report.installed.push((*name).into());
    }
    Ok(report)
}

/// Backwards-compatible thin wrapper kept so existing callers (and
/// any external integrations from the PR-2/PR-3 era) keep working.
/// Always installs the full hook trio under the hood — there's no
/// reference-mode path that wants the post-commit hook in
/// isolation.
pub fn install_post_commit_hook(repo_root: &Path) -> Result<bool, MorphError> {
    Ok(install_reference_hooks(repo_root)?.changed())
}

/// Resolve git's current branch name (the unqualified form, e.g.
/// `"main"`). Returns `Ok(None)` for detached HEAD.
pub fn current_git_branch(repo_root: &Path) -> Result<Option<String>, MorphError> {
    let out = Command::new("git")
        .arg("symbolic-ref")
        .arg("--quiet")
        .arg("--short")
        .arg("HEAD")
        .current_dir(repo_root)
        .output()
        .map_err(|e| MorphError::Other(format!("git symbolic-ref failed to spawn: {}", e)))?;
    if !out.status.success() {
        return Ok(None);
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        Ok(None)
    } else {
        Ok(Some(s))
    }
}

/// Find the morph commit whose `git_origin_sha` matches the supplied
/// git SHA, walking the cache built from every branch's history.
/// Returns `Ok(None)` when no morph mirror exists yet (the user
/// `git checkout`-ed an unmirrored commit).
pub fn lookup_morph_for_git_sha(
    store: &dyn Store,
    git_sha: &str,
) -> Result<Option<Hash>, MorphError> {
    let cache = build_git_to_morph_cache(store)?;
    Ok(cache.get(git_sha).copied())
}

/// Outcome of `handle_post_checkout`. Designed for the CLI to print
/// without ambiguity in unit-tests; the hook itself swallows output
/// in production.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckoutOutcome {
    /// HEAD landed on a branch and morph followed.
    SwitchedBranch { branch: String, morph_hash: Hash },
    /// HEAD is detached (e.g. `git checkout <sha>`); morph stays put.
    DetachedHead,
    /// Branch checkout, but no morph commit exists for the new git
    /// SHA yet. Caller may want to run `morph reference-sync` to
    /// create one.
    NoMatchingMorphCommit { git_sha: String },
    /// File-level checkout (`git checkout -- file`); HEAD didn't
    /// move so morph has nothing to do.
    FileCheckout,
}

/// Handle a `post-checkout` hook firing. The hook receives three
/// args from git: `prev_sha`, `new_sha`, and `branch_flag` (1 for
/// branch checkout, 0 for file checkout). We ignore file checkouts
/// outright. For branch checkouts:
///
///   1. Read git's new symbolic branch (detached HEAD short-circuits
///      to `DetachedHead` so we don't pin morph to nothing).
///   2. Move morph HEAD to that branch — even when no morph commit
///      mirrors `new_sha` yet. That's critical for the "user runs
///      `git checkout -b feature` before any morph commit exists"
///      flow: the next post-commit hook needs to land on the
///      right morph branch, not on whatever branch morph used to
///      be on.
///   3. If a morph commit *does* mirror `new_sha`, fast-forward
///      `heads/<branch>` to it so morph and git agree on tip.
pub fn handle_post_checkout(
    store: &dyn Store,
    repo_root: &Path,
    prev_sha: &str,
    new_sha: &str,
    branch_flag: &str,
) -> Result<CheckoutOutcome, MorphError> {
    let _ = prev_sha;
    if branch_flag != "1" {
        return Ok(CheckoutOutcome::FileCheckout);
    }
    let git_branch = match current_git_branch(repo_root)? {
        Some(b) => b,
        None => return Ok(CheckoutOutcome::DetachedHead),
    };
    crate::commit::set_head_branch(store, &git_branch)?;
    let morph_hash = match lookup_morph_for_git_sha(store, new_sha)? {
        Some(h) => h,
        None => {
            return Ok(CheckoutOutcome::NoMatchingMorphCommit {
                git_sha: new_sha.into(),
            });
        }
    };
    store.ref_write(&format!("heads/{}", git_branch), &morph_hash)?;
    Ok(CheckoutOutcome::SwitchedBranch {
        branch: git_branch,
        morph_hash,
    })
}

/// Outcome of `handle_post_rewrite`. `rewrites` reports the
/// (old_morph, new_morph) pairs the hook produced; `annotated`
/// counts how many old morph commits got a `kind: "rewritten"`
/// annotation (== `rewrites.len()` minus any pair where the old
/// commit was never mirrored to morph in the first place).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewriteOutcome {
    pub command: String,
    pub rewrites: Vec<(Hash, Hash)>,
    pub annotated: usize,
}

/// Handle a `post-rewrite` hook firing. `command` is `"amend"` or
/// `"rebase"`. `rewrite_lines` is the stdin git fed the hook: each
/// line is `old_sha new_sha [extra]`.
///
/// For each pair we:
///   1. Mirror `new_sha` into a morph commit (idempotent — if the
///      post-commit hook already created it, we just look it up).
///   2. If a morph commit exists for `old_sha`, attach a `kind:
///      "rewritten"` annotation to it pointing at the new morph
///      hash. That's the signal certified-evidence consumers use to
///      surface "stale" state — we do not mutate the original
///      certification annotation, since morph's object model is
///      append-only.
///
/// Finally we advance the current branch ref to whichever new morph
/// commit corresponds to git's current HEAD, so subsequent morph
/// operations see the rewritten history.
pub fn handle_post_rewrite(
    store: &dyn Store,
    repo_root: &Path,
    command: &str,
    rewrite_lines: &str,
    morph_version: Option<&str>,
) -> Result<RewriteOutcome, MorphError> {
    let mut outcome = RewriteOutcome {
        command: command.into(),
        ..Default::default()
    };

    let mut cache = build_git_to_morph_cache(store)?;

    for line in rewrite_lines.lines() {
        let mut parts = line.split_whitespace();
        let old_sha = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let new_sha = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let new_morph = if let Some(h) = cache.get(new_sha) {
            *h
        } else {
            sync_one_commit(store, repo_root, new_sha, "git-hook", morph_version, &mut cache)?
        };

        if let Some(old_morph) = cache.get(old_sha).copied() {
            if old_morph != new_morph {
                let mut data = BTreeMap::new();
                data.insert(
                    "successor".into(),
                    serde_json::Value::String(new_morph.to_string()),
                );
                data.insert(
                    "git_command".into(),
                    serde_json::Value::String(command.to_string()),
                );
                data.insert(
                    "old_git_sha".into(),
                    serde_json::Value::String(old_sha.to_string()),
                );
                data.insert(
                    "new_git_sha".into(),
                    serde_json::Value::String(new_sha.to_string()),
                );
                let annotation =
                    crate::annotate::create_annotation(&old_morph, None, "rewritten".into(), data, None);
                store.put(&annotation)?;
                outcome.annotated += 1;
            }
            outcome.rewrites.push((old_morph, new_morph));
        }
    }

    if let Some(head_sha) = git_head_sha(repo_root)? {
        if let Some(h) = cache.get(&head_sha).copied() {
            let branch = current_git_branch(repo_root)?
                .or_else(|| crate::commit::current_branch(store).ok().flatten())
                .unwrap_or_else(|| crate::commit::DEFAULT_BRANCH.to_string());
            store.ref_write(&format!("heads/{}", branch), &h)?;
            crate::commit::set_head_branch(store, &branch)?;
        }
    }

    Ok(outcome)
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
    let mut cursor = Some(*head);
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
            pending.push(h);
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
        assert_ne!(Some(new), first.new_commit);
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
    fn git_parents_returns_empty_for_root_commit() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "root"]);
        let head = git_head_sha(dir.path()).unwrap().unwrap();
        let parents = git_parents(dir.path(), &head).unwrap();
        assert!(parents.is_empty());
    }

    #[test]
    fn git_parents_returns_two_for_merge_commit() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "base"]);
        run_git(dir.path(), &["checkout", "-q", "-b", "feature"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "feat"]);
        run_git(dir.path(), &["checkout", "-q", "main"]);
        run_git(
            dir.path(),
            &["merge", "--no-ff", "feature", "-q", "-m", "merge"],
        );
        let head = git_head_sha(dir.path()).unwrap().unwrap();
        let parents = git_parents(dir.path(), &head).unwrap();
        assert_eq!(parents.len(), 2, "merge commit has two parents");
    }

    #[test]
    fn backfill_creates_one_commit_per_git_commit_in_topo_order() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "g1"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "g2"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "g3"]);
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        let init_sha = git_head_sha(dir.path()).unwrap().unwrap();
        // Backfill from current HEAD: only the init point itself
        // (g3) gets mirrored, since the range `g3^..g3` is just g3.
        let n = backfill_from_init(store.as_ref(), dir.path(), Some(&init_sha), None).unwrap();
        assert_eq!(n, 1);

        // Add more commits, then backfill: g4 enters the range, the
        // already-mirrored g3 is skipped (idempotent).
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "g4"]);
        let n2 = backfill_from_init(store.as_ref(), dir.path(), Some(&init_sha), None).unwrap();
        assert_eq!(n2, 1, "only g4 should be new");
    }

    #[test]
    fn backfill_resolves_merge_commit_parents_from_both_branches() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "base"]);
        let init_sha = git_head_sha(dir.path()).unwrap().unwrap();
        run_git(dir.path(), &["checkout", "-q", "-b", "feature"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "feat"]);
        run_git(dir.path(), &["checkout", "-q", "main"]);
        run_git(
            dir.path(),
            &["merge", "--no-ff", "feature", "-q", "-m", "merge"],
        );
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        let n = backfill_from_init(store.as_ref(), dir.path(), Some(&init_sha), None).unwrap();
        assert_eq!(n, 3, "base + feat + merge → 3 morph commits");

        // Resolve HEAD's two parents and confirm one tracks back to
        // the base commit's mirror and the other to the feature
        // commit's mirror.
        let head = crate::commit::resolve_head(store.as_ref())
            .unwrap()
            .unwrap();
        let merge_commit = match store.get(&head).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected commit"),
        };
        assert_eq!(
            merge_commit.parents.len(),
            2,
            "merge mirror has two morph parents"
        );
        let mut messages: Vec<String> = merge_commit
            .parents
            .iter()
            .map(|p| {
                let h = Hash::from_hex(p).unwrap();
                match store.get(&h).unwrap() {
                    MorphObject::Commit(c) => c.message.trim().to_string(),
                    _ => panic!("parent not a commit"),
                }
            })
            .collect();
        messages.sort();
        assert_eq!(messages, vec!["base".to_string(), "feat".to_string()]);
    }

    #[test]
    fn install_reference_hooks_writes_all_three() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        let _ = std::fs::remove_dir_all(dir.path().join(".git").join("hooks"));
        let report = install_reference_hooks(dir.path()).unwrap();
        assert_eq!(report.installed.len(), 4);
        assert!(report.already_present.is_empty());
        for hook in &["post-commit", "post-checkout", "post-rewrite", "post-merge"] {
            let hook_path = dir.path().join(format!(".git/hooks/{}", hook));
            assert!(hook_path.is_file(), "{} missing", hook);
            let content = std::fs::read_to_string(&hook_path).unwrap();
            assert!(
                content.contains(&format!("morph hook {}", hook)),
                "{} hook content unexpected: {}",
                hook,
                content
            );
            assert!(content.contains("MORPH_INTERNAL"));
        }
        let report2 = install_reference_hooks(dir.path()).unwrap();
        assert_eq!(report2.installed.len(), 0);
        assert_eq!(report2.already_present.len(), 4);
        assert!(!report2.changed());
    }

    #[test]
    fn install_post_commit_hook_back_compat_alias() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        let wrote = install_post_commit_hook(dir.path()).unwrap();
        assert!(wrote);
        let again = install_post_commit_hook(dir.path()).unwrap();
        assert!(!again);
    }

    #[test]
    fn install_reference_hooks_tolerates_legacy_post_commit() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        let hooks_dir = dir.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        // A pre-PR-4 morph stub (PR2/PR3 era).
        std::fs::write(
            hooks_dir.join("post-commit"),
            "#!/bin/sh\nexec morph reference-sync >/dev/null 2>&1 || true\n",
        )
        .unwrap();
        let report = install_reference_hooks(dir.path()).unwrap();
        assert!(report.installed.contains(&"post-commit".to_string()));
        let content = std::fs::read_to_string(hooks_dir.join("post-commit")).unwrap();
        assert!(content.contains("morph hook post-commit"));
    }

    #[test]
    fn install_reference_hooks_refuses_to_clobber_foreign_content() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        let hooks_dir = dir.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("post-checkout"), "#!/bin/sh\necho hi\n").unwrap();
        let err = install_reference_hooks(dir.path())
            .expect_err("user-authored hook should be preserved");
        match err {
            MorphError::Other(msg) => {
                assert!(msg.contains("foreign content"));
                assert!(msg.contains("post-checkout"));
            }
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn handle_post_checkout_advances_morph_branch_to_match_git() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "main 1"]);
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        sync_to_head(store.as_ref(), dir.path(), None).unwrap();

        run_git(dir.path(), &["checkout", "-q", "-b", "feature"]);
        run_git(
            dir.path(),
            &["commit", "--allow-empty", "-q", "-m", "feature 1"],
        );
        let new_sha = git_head_sha(dir.path()).unwrap().unwrap();
        sync_to_head(store.as_ref(), dir.path(), None).unwrap();

        let outcome = handle_post_checkout(store.as_ref(), dir.path(), "", &new_sha, "1").unwrap();
        match outcome {
            CheckoutOutcome::SwitchedBranch { branch, .. } => assert_eq!(branch, "feature"),
            other => panic!("expected SwitchedBranch, got {:?}", other),
        }
        assert_eq!(
            crate::commit::current_branch(store.as_ref()).unwrap().as_deref(),
            Some("feature")
        );
    }

    #[test]
    fn handle_post_checkout_returns_detached_head_outcome() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "first"]);
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        sync_to_head(store.as_ref(), dir.path(), None).unwrap();
        let head = git_head_sha(dir.path()).unwrap().unwrap();
        run_git(dir.path(), &["checkout", "-q", "--detach", "HEAD"]);
        let outcome = handle_post_checkout(store.as_ref(), dir.path(), "", &head, "1").unwrap();
        assert_eq!(outcome, CheckoutOutcome::DetachedHead);
    }

    #[test]
    fn handle_post_checkout_skips_file_checkouts() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "first"]);
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        let outcome = handle_post_checkout(store.as_ref(), dir.path(), "", "", "0").unwrap();
        assert_eq!(outcome, CheckoutOutcome::FileCheckout);
    }

    #[test]
    fn handle_post_rewrite_amend_annotates_old_commit() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "v1"]);
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        sync_to_head(store.as_ref(), dir.path(), None).unwrap();
        let old_git = git_head_sha(dir.path()).unwrap().unwrap();
        let old_morph = crate::commit::resolve_head(store.as_ref()).unwrap().unwrap();
        run_git(
            dir.path(),
            &["commit", "--amend", "-q", "--allow-empty", "-m", "v1 (amended)"],
        );
        let new_git = git_head_sha(dir.path()).unwrap().unwrap();
        let stdin = format!("{} {}\n", old_git, new_git);
        let outcome =
            handle_post_rewrite(store.as_ref(), dir.path(), "amend", &stdin, None).unwrap();
        assert_eq!(outcome.command, "amend");
        assert_eq!(outcome.rewrites.len(), 1);
        assert_eq!(outcome.annotated, 1);
        let anns = crate::annotate::list_annotations(store.as_ref(), &old_morph, None).unwrap();
        assert!(
            anns.iter().any(|(_, a)| a.kind == "rewritten"),
            "expected a `rewritten` annotation on the old morph commit, got {:?}",
            anns
        );
    }

    #[test]
    fn handle_post_rewrite_skips_when_old_morph_unknown() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "v1"]);
        let store: Box<dyn Store> = Box::new(crate::init_repo(dir.path()).unwrap());
        // Don't sync first — old SHA has no morph mirror, so the
        // post-rewrite handler must not panic.
        let stdin = format!("{} {}\n", "a".repeat(40), git_head_sha(dir.path()).unwrap().unwrap());
        let outcome =
            handle_post_rewrite(store.as_ref(), dir.path(), "rebase", &stdin, None).unwrap();
        assert_eq!(outcome.annotated, 0);
        // No mapping for the old SHA, but the new commit was still
        // synced (the rewrite advances the mirror forward even if
        // we can't annotate).
        assert!(outcome.rewrites.is_empty());
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
