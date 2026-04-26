// Integration tests for `morph status` while a merge is in progress.
// Sets up state via morph-core APIs (no CLI merge subcommand yet),
// then runs the `morph` binary and asserts the rendered status output.

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;

fn init_repo_at(path: &std::path::Path) {
    let mut cmd = cargo_bin_cmd!("morph");
    cmd.arg("init").arg(path).assert().success();
}

fn write(path: &std::path::Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn open_store(repo: &std::path::Path) -> Box<dyn morph_core::Store> {
    morph_core::open_store(&repo.join(".morph")).unwrap()
}

#[test]
fn status_during_textual_merge_lists_unmerged_paths_and_hint() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    init_repo_at(repo);

    // Build divergent commits via the CLI so the on-disk repo state
    // matches what `start_merge` expects.
    write(&repo.join("file.txt"), "line1\nbase\nline3\n");
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["add", "file.txt"])
        .assert()
        .success();
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["commit", "-m", "base"])
        .assert()
        .success();

    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["branch", "feature"])
        .assert()
        .success();

    write(&repo.join("file.txt"), "line1\nMAIN\nline3\n");
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["add", "file.txt"])
        .assert()
        .success();
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["commit", "-m", "main"])
        .assert()
        .success();

    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["checkout", "feature"])
        .assert()
        .success();
    write(&repo.join("file.txt"), "line1\nFEATURE\nline3\n");
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["add", "file.txt"])
        .assert()
        .success();
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["commit", "-m", "feature"])
        .assert()
        .success();
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["checkout", "main"])
        .assert()
        .success();

    let store = open_store(repo);
    let _ = morph_core::start_merge(
        &*store,
        repo,
        morph_core::StartMergeOpts::new("feature"),
    )
    .expect("start_merge should set up MERGE_HEAD with conflicts");

    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("You have unmerged paths."))
        .stdout(predicate::str::contains("morph merge --continue"))
        .stdout(predicate::str::contains("morph merge --abort"))
        .stdout(predicate::str::contains("file.txt"));
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
