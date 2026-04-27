// Integration tests for `morph status` while a merge is in progress,
// driven entirely through the CLI: `morph merge feature` to enter the
// conflict state, `morph status` to inspect it, and `morph merge --abort`
// to clean up.

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;

fn init_repo_at(path: &std::path::Path) {
    cargo_bin_cmd!("morph").arg("init").arg(path).assert().success();
}

fn write(path: &std::path::Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn morph(repo: &std::path::Path, args: &[&str]) -> assert_cmd::assert::Assert {
    cargo_bin_cmd!("morph").current_dir(repo).args(args).assert()
}

#[test]
fn status_during_textual_merge_lists_unmerged_paths_and_hint() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    init_repo_at(repo);

    write(&repo.join("file.txt"), "line1\nbase\nline3\n");
    morph(repo, &["add", "file.txt"]).success();
    morph(repo, &["commit", "-m", "base"]).success();

    morph(repo, &["branch", "feature"]).success();

    write(&repo.join("file.txt"), "line1\nMAIN\nline3\n");
    morph(repo, &["add", "file.txt"]).success();
    morph(repo, &["commit", "-m", "main"]).success();

    morph(repo, &["checkout", "feature"]).success();
    write(&repo.join("file.txt"), "line1\nFEATURE\nline3\n");
    morph(repo, &["add", "file.txt"]).success();
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
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["add", "hello.txt"])
        .assert()
        .success();
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["commit", "-m", "first"])
        .assert()
        .success();

    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("You have unmerged paths.").not())
        .stdout(predicate::str::contains("nothing to commit"));
}
