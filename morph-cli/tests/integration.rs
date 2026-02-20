//! Integration tests: CLI commands against a temp repo.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn init_repo(dir: &std::path::Path) {
    let mut cmd = Command::cargo_bin("morph").unwrap();
    cmd.arg("init").arg(dir).assert().success();
}

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
    assert!(path.join("prompts").is_dir());
    assert!(path.join("programs").is_dir());
    assert!(path.join("evals").is_dir());
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

#[test]
fn prompt_create_and_materialize() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);
    fs::write(path.join("prompts").join("hello.txt"), "Hello world").unwrap();

    let mut create = Command::cargo_bin("morph").unwrap();
    create.current_dir(path).arg("prompt").arg("create").arg("prompts/hello.txt");
    let out = create.assert().success();
    let hash = String::from_utf8_lossy(&out.get_output().stdout).trim().to_string();
    assert_eq!(hash.len(), 64);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

    let mut mat = Command::cargo_bin("morph").unwrap();
    mat.current_dir(path)
        .arg("prompt")
        .arg("materialize")
        .arg(&hash)
        .arg("--output")
        .arg(path.join("prompts").join("out.prompt"));
    mat.assert().success();
    assert_eq!(fs::read_to_string(path.join("prompts").join("out.prompt")).unwrap(), "Hello world");
}

#[test]
fn status_and_add() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);
    fs::write(path.join("prompts").join("p.txt"), "prompt body").unwrap();

    let mut status_cmd = Command::cargo_bin("morph").unwrap();
    status_cmd.current_dir(path).arg("status");
    status_cmd.assert().success().stdout(predicate::str::contains("new"));

    let mut add_cmd = Command::cargo_bin("morph").unwrap();
    add_cmd.current_dir(path).arg("add").arg(".");
    add_cmd.assert().success();

    let mut status_after = Command::cargo_bin("morph").unwrap();
    status_after.current_dir(path).arg("status");
    status_after.assert().success().stdout(predicate::str::contains("tracked"));
}

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
    fs::write(path.join("programs").join("prog.json"), program_json).unwrap();

    let mut create = Command::cargo_bin("morph").unwrap();
    create.current_dir(path).arg("program").arg("create").arg("programs/prog.json");
    let out = create.assert().success();
    let hash = String::from_utf8_lossy(&out.get_output().stdout).trim().to_string();

    let mut show = Command::cargo_bin("morph").unwrap();
    show.current_dir(path).arg("program").arg("show").arg(&hash);
    show.assert().success().stdout(predicate::str::contains("program"));
}

#[test]
fn commit_and_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);

    let program_json = r#"{"graph":{"nodes":[{"id":"n1","kind":"identity","ref":null,"params":{}}],"edges":[]},"prompts":[],"eval_suite":null,"provenance":null}"#;
    fs::write(path.join("programs").join("p.json"), program_json).unwrap();
    let eval_json = r#"{"cases":[],"metrics":[{"name":"acc","aggregation":"mean","threshold":0.0}]}"#;
    fs::write(path.join("evals").join("e.json"), eval_json).unwrap();

    let mut prog_create = Command::cargo_bin("morph").unwrap();
    prog_create.current_dir(path).arg("program").arg("create").arg("programs/p.json");
    let prog_out = prog_create.assert().success();
    let prog_hash = String::from_utf8_lossy(&prog_out.get_output().stdout).trim().to_string();

    let mut add = Command::cargo_bin("morph").unwrap();
    add.current_dir(path).arg("add").arg("evals/e.json");
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

#[test]
fn annotate_and_annotations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    init_repo(path);
    fs::write(path.join("prompts").join("p.txt"), "hello").unwrap();
    let mut add = Command::cargo_bin("morph").unwrap();
    add.current_dir(path).arg("add").arg(".");
    add.assert().success();
    let mut create = Command::cargo_bin("morph").unwrap();
    create.current_dir(path).arg("prompt").arg("create").arg("prompts/p.txt");
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
