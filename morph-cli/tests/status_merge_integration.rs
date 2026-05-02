// Integration tests for `morph status` while a merge is in progress,
// driven entirely through the CLI: `morph merge feature` to enter the
// conflict state, `morph status` to inspect it, and `morph merge --abort`
// to clean up.

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;

fn init_repo_at(path: &std::path::Path) {
    cargo_bin_cmd!("morph")
        .arg("init")
        .arg("--git-init")
        .arg("--no-default-policy")
        .arg(path)
        .assert()
        .success();
}

fn write(path: &std::path::Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn morph(repo: &std::path::Path, args: &[&str]) -> assert_cmd::assert::Assert {
    // Stamp git identity per-invocation so reference-mode `morph
    // commit` (which shells out to `git commit`) works on hosts
    // without a global git config (GitHub-Actions runners,
    // fresh VMs). Mirrors what the spec-test harness in
    // `morph-cli/build.rs` already does.
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "morph-test")
        .env("GIT_AUTHOR_EMAIL", "morph-test@example.com")
        .env("GIT_COMMITTER_NAME", "morph-test")
        .env("GIT_COMMITTER_EMAIL", "morph-test@example.com")
        .args(args)
        .assert()
}

fn git_add(repo: &std::path::Path, path: &str) {
    let status = std::process::Command::new("git")
        .current_dir(repo)
        .args(["add", path])
        .status()
        .unwrap();
    assert!(status.success(), "git add {} failed", path);
}

#[test]
fn status_during_textual_merge_lists_unmerged_paths_and_hint() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    init_repo_at(repo);

    write(&repo.join("file.txt"), "line1\nbase\nline3\n");
    git_add(repo, "file.txt");
    morph(repo, &["commit", "-m", "base"]).success();

    morph(repo, &["branch", "feature"]).success();

    write(&repo.join("file.txt"), "line1\nMAIN\nline3\n");
    git_add(repo, "file.txt");
    morph(repo, &["commit", "-m", "main"]).success();

    morph(repo, &["checkout", "feature"]).success();
    write(&repo.join("file.txt"), "line1\nFEATURE\nline3\n");
    git_add(repo, "file.txt");
    morph(repo, &["commit", "-m", "feature"]).success();
    morph(repo, &["checkout", "main"]).success();

    // Drive the conflict via the CLI now that `morph merge` is wired
    // up to `start_merge`. Exits 1 because conflict markers landed
    // on disk and the user needs to run `--continue` after fixing.
    morph(repo, &["merge", "feature"]).code(1);

    morph(repo, &["status"])
        .success()
        .stdout(predicate::str::contains("You have unmerged paths."))
        .stdout(predicate::str::contains("morph merge --continue"))
        .stdout(predicate::str::contains("morph merge --abort"))
        .stdout(predicate::str::contains("file.txt"));

    // `--abort` cleans up and `status` no longer mentions a merge.
    morph(repo, &["merge", "--abort"])
        .success()
        .stdout(predicate::str::contains("Merge aborted"));
    morph(repo, &["status"])
        .success()
        .stdout(predicate::str::contains("You have unmerged paths.").not());
}

#[test]
fn status_outside_merge_does_not_show_merge_hints() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    init_repo_at(repo);

    write(&repo.join("hello.txt"), "world");
    git_add(repo, "hello.txt");
    morph(repo, &["commit", "-m", "first"]).success();

    morph(repo, &["status"])
        .success()
        .stdout(predicate::str::contains("You have unmerged paths.").not())
        .stdout(predicate::str::contains("nothing to commit"));
}
