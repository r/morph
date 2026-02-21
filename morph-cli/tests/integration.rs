//! Integration tests: CLI commands against a temp repo.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn init_repo(dir: &std::path::Path) {
    let mut cmd = Command::cargo_bin("morph").unwrap();
    cmd.arg("init").arg(dir).assert().success();
}

// ── init ──────────────────────────────────────────────────────────────

#[test]
fn init_creates_morph_dir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);

    let morph_dir = path.join(".morph");
    assert!(morph_dir.is_dir(), ".morph should exist");
    assert!(morph_dir.join("objects").is_dir());
    assert!(morph_dir.join("refs").is_dir());
    assert!(morph_dir.join("refs/heads").is_dir());
    assert!(morph_dir.join("prompts").is_dir(), ".morph/prompts should exist");
    assert!(morph_dir.join("evals").is_dir(), ".morph/evals should exist");
}

#[test]
fn init_does_not_create_top_level_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);

    assert!(!path.join("prompts").exists(), "top-level prompts/ must not exist");
    assert!(!path.join("programs").exists(), "top-level programs/ must not exist");
    assert!(!path.join("evals").exists(), "top-level evals/ must not exist");
}

#[test]
fn init_prints_message() {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("morph").unwrap();
    cmd.arg("init").arg(dir.path());
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Initialized Morph repository"));
}

// ── status: working directory files ───────────────────────────────────

#[test]
fn status_shows_working_dir_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);
    fs::write(path.join("hello.txt"), "world").unwrap();

    let mut cmd = Command::cargo_bin("morph").unwrap();
    cmd.current_dir(path).arg("status");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("hello.txt"))
        .stdout(predicate::str::contains("new"));
}

#[test]
fn status_shows_nested_working_dir_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/app.rs"), "fn main() {}").unwrap();

    let mut cmd = Command::cargo_bin("morph").unwrap();
    cmd.current_dir(path).arg("status");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("app.rs"));
}

#[test]
fn status_empty_when_no_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);

    let mut cmd = Command::cargo_bin("morph").unwrap();
    cmd.current_dir(path).arg("status");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("No files"));
}

// ── add: git-like staging ─────────────────────────────────────────────

#[test]
fn add_stages_any_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);
    fs::write(path.join("code.py"), "print('hi')").unwrap();

    let mut add_cmd = Command::cargo_bin("morph").unwrap();
    add_cmd.current_dir(path).arg("add").arg("code.py");
    let out = add_cmd.assert().success();
    let hash = String::from_utf8_lossy(&out.get_output().stdout).trim().to_string();
    assert_eq!(hash.len(), 64, "should output a hash");

    let mut status_cmd = Command::cargo_bin("morph").unwrap();
    status_cmd.current_dir(path).arg("status");
    status_cmd.assert().success().stdout(predicate::str::contains("tracked"));
}

#[test]
fn add_dot_stages_all_working_dir_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);
    fs::write(path.join("a.txt"), "aaa").unwrap();
    fs::create_dir_all(path.join("lib")).unwrap();
    fs::write(path.join("lib/b.txt"), "bbb").unwrap();

    let mut add_cmd = Command::cargo_bin("morph").unwrap();
    add_cmd.current_dir(path).arg("add").arg(".");
    let out = add_cmd.assert().success();
    let output = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let hashes: Vec<_> = output.trim().lines().collect();
    assert!(hashes.len() >= 2, "should stage at least 2 files, got {}", hashes.len());
}

// ── prompt operations ─────────────────────────────────────────────────

#[test]
fn prompt_create_and_materialize() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);
    fs::write(path.join(".morph/prompts/hello.txt"), "Hello world").unwrap();

    let mut create = Command::cargo_bin("morph").unwrap();
    create.current_dir(path).arg("prompt").arg("create").arg(".morph/prompts/hello.txt");
    let out = create.assert().success();
    let hash = String::from_utf8_lossy(&out.get_output().stdout).trim().to_string();
    assert_eq!(hash.len(), 64);

    let mut mat = Command::cargo_bin("morph").unwrap();
    mat.current_dir(path)
        .arg("prompt")
        .arg("materialize")
        .arg(&hash)
        .arg("--output")
        .arg(path.join(".morph/prompts/out.prompt"));
    mat.assert().success();
    assert_eq!(fs::read_to_string(path.join(".morph/prompts/out.prompt")).unwrap(), "Hello world");
}

// ── program operations (from any path) ────────────────────────────────

#[test]
fn program_create_and_show() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);
    let program_json = r#"{
      "graph": {
        "nodes": [{"id": "n1", "kind": "identity", "ref": null, "params": {}}],
        "edges": []
      },
      "prompts": [],
      "eval_suite": null,
      "provenance": null
    }"#;
    fs::write(path.join("prog.json"), program_json).unwrap();

    let mut create = Command::cargo_bin("morph").unwrap();
    create.current_dir(path).arg("program").arg("create").arg("prog.json");
    let out = create.assert().success();
    let hash = String::from_utf8_lossy(&out.get_output().stdout).trim().to_string();

    let mut show = Command::cargo_bin("morph").unwrap();
    show.current_dir(path).arg("program").arg("show").arg(&hash);
    show.assert().success().stdout(predicate::str::contains("program"));
}

// ── commit and log ────────────────────────────────────────────────────

#[test]
fn commit_and_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);

    let program_json = r#"{"graph":{"nodes":[{"id":"n1","kind":"identity","ref":null,"params":{}}],"edges":[]},"prompts":[],"eval_suite":null,"provenance":null}"#;
    fs::write(path.join("prog.json"), program_json).unwrap();
    let eval_json = r#"{"cases":[],"metrics":[{"name":"acc","aggregation":"mean","threshold":0.0}]}"#;
    fs::write(path.join(".morph/evals/e.json"), eval_json).unwrap();

    let mut prog_create = Command::cargo_bin("morph").unwrap();
    prog_create.current_dir(path).arg("program").arg("create").arg("prog.json");
    let prog_out = prog_create.assert().success();
    let prog_hash = String::from_utf8_lossy(&prog_out.get_output().stdout).trim().to_string();

    let mut add = Command::cargo_bin("morph").unwrap();
    add.current_dir(path).arg("add").arg(".morph/evals/e.json");
    let add_out = add.assert().success();
    let suite_hash = String::from_utf8_lossy(&add_out.get_output().stdout).trim().lines().next().unwrap().to_string();

    let mut commit_cmd = Command::cargo_bin("morph").unwrap();
    commit_cmd
        .current_dir(path)
        .arg("commit")
        .arg("-m")
        .arg("first commit")
        .arg("--program")
        .arg(&prog_hash)
        .arg("--eval-suite")
        .arg(&suite_hash)
        .arg("--metrics")
        .arg("{}");
    let commit_out = commit_cmd.assert().success();
    let commit_hash = String::from_utf8_lossy(&commit_out.get_output().stdout).trim().to_string();
    assert_eq!(commit_hash.len(), 64);

    let mut log_cmd = Command::cargo_bin("morph").unwrap();
    log_cmd.current_dir(path).arg("log");
    log_cmd.assert().success().stdout(predicate::str::contains("first commit"));
}

// ── run and eval record ───────────────────────────────────────────────

#[test]
fn run_record_and_eval_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);

    let trace_json = r#"{"type":"trace","events":[{"id":"e1","seq":0,"ts":"2025-01-01T00:00:00Z","kind":"prompt","payload":{}}]}"#;
    fs::write(path.join("trace.json"), trace_json).unwrap();

    let trace_obj: morph_core::MorphObject = serde_json::from_str(trace_json).unwrap();
    let trace_hash = morph_core::content_hash(&trace_obj).unwrap();

    let run_json = format!(
        r#"{{"type":"run","program":"{}","commit":null,"environment":{{"model":"test","version":"0","parameters":{{}},"toolchain":{{}}}},"input_state_hash":"0000000000000000000000000000000000000000000000000000000000000000","output_artifacts":[],"metrics":{{}},"trace":"{}","agent":{{"id":"cli","version":"0","policy":null}}}}"#,
        "0".repeat(64),
        trace_hash
    );
    fs::write(path.join("run.json"), &run_json).unwrap();

    let mut run_record = Command::cargo_bin("morph").unwrap();
    run_record
        .current_dir(path)
        .arg("run")
        .arg("record")
        .arg("run.json")
        .arg("--trace")
        .arg("trace.json");
    let out = run_record.assert().success();
    let run_hash = String::from_utf8_lossy(&out.get_output().stdout).trim().to_string();
    assert_eq!(run_hash.len(), 64);

    let metrics_json = r#"{"metrics":{"accuracy":0.92,"latency_p95":1.2}}"#;
    fs::write(path.join("metrics.json"), metrics_json).unwrap();

    let mut eval_record = Command::cargo_bin("morph").unwrap();
    eval_record.current_dir(path).arg("eval").arg("record").arg("metrics.json");
    eval_record.assert().success().stdout(predicate::str::contains("0.92"));
}

// ── annotate ──────────────────────────────────────────────────────────

#[test]
fn annotate_and_annotations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);
    fs::write(path.join(".morph/prompts/p.txt"), "hello").unwrap();
    let mut add = Command::cargo_bin("morph").unwrap();
    add.current_dir(path).arg("add").arg(".");
    add.assert().success();
    let mut create = Command::cargo_bin("morph").unwrap();
    create.current_dir(path).arg("prompt").arg("create").arg(".morph/prompts/p.txt");
    let out = create.assert().success();
    let target_hash = String::from_utf8_lossy(&out.get_output().stdout).trim().to_string();

    let mut annotate_cmd = Command::cargo_bin("morph").unwrap();
    annotate_cmd
        .current_dir(path)
        .arg("annotate")
        .arg(&target_hash)
        .arg("--kind")
        .arg("feedback")
        .arg("--data")
        .arg(r#"{"rating":"good"}"#);
    let ann_out = annotate_cmd.assert().success();
    let ann_hash = String::from_utf8_lossy(&ann_out.get_output().stdout).trim().to_string();
    assert_eq!(ann_hash.len(), 64);

    let mut list_cmd = Command::cargo_bin("morph").unwrap();
    list_cmd.current_dir(path).arg("annotations").arg(&target_hash);
    list_cmd.assert().success().stdout(predicate::str::contains("feedback"));
}
