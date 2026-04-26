// Integration tests for `morph remote-helper` — the hidden
// JSON-RPC subcommand spawned over `ssh user@host morph
// remote-helper`. Each request/response is a single JSON line. The
// helper acts as a thin wrapper around `morph_core::Store` so that
// `SshStore` (PR5 Stage D) can drive a real repo on the other side.

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

fn init_repo_at(path: &std::path::Path) {
    let mut cmd = cargo_bin_cmd!("morph");
    cmd.arg("init").arg(path).assert().success();
}

/// Spawn `morph remote-helper --repo-root <path>` as a child
/// process; `body` is written to stdin then stdin is closed; the
/// returned String is whatever the helper wrote to stdout before
/// exiting.
fn run_helper(repo_root: &std::path::Path, body: &str) -> (String, String, i32) {
    // assert_cmd::cargo::cargo_bin!("morph") is the macro form that
    // remains compatible with custom cargo build-dirs.
    let bin = assert_cmd::cargo::cargo_bin!("morph");
    let mut child = Command::new(bin)
        .arg("remote-helper")
        .arg("--repo-root")
        .arg(repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn remote-helper");

    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin.write_all(body.as_bytes()).unwrap();
    }
    drop(child.stdin.take());

    let out = child
        .wait_with_output()
        .expect("wait remote-helper");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let code = out.status.code().unwrap_or(-1);
    (stdout, stderr, code)
}

#[test]
fn remote_helper_exits_cleanly_on_eof() {
    // PR5 cycle 4 RED: the subcommand must exist, accept --repo-root
    // pointing at an initialized morph repo, and exit 0 when stdin
    // closes with no bytes written.
    let dir = tempfile::tempdir().unwrap();
    init_repo_at(dir.path());

    let (stdout, stderr, code) = run_helper(dir.path(), "");
    assert_eq!(code, 0, "expected clean exit on EOF, stderr={}", stderr);
    assert!(
        stdout.is_empty(),
        "expected no output on EOF, got: {}",
        stdout
    );
}

#[test]
fn remote_helper_rejects_missing_repo_root() {
    // PR5 cycle 4 RED: pointing the helper at a non-morph directory
    // must fail loudly so SSH callers can show a meaningful error
    // instead of hanging.
    let dir = tempfile::tempdir().unwrap();
    let (_stdout, stderr, code) = run_helper(dir.path(), "");
    assert_ne!(code, 0, "expected non-zero exit on non-morph dir");
    assert!(
        stderr.to_lowercase().contains("not a morph") || stderr.contains("repo"),
        "expected helpful error, got: {}",
        stderr
    );
}

#[test]
fn remote_helper_responds_to_hello() {
    // PR5 cycle 5 RED: `hello` is the handshake message. Helper
    // returns `{ok: true, morph_version: "..."}`. Used by SshStore
    // to verify the remote is a Morph helper at all.
    let dir = tempfile::tempdir().unwrap();
    init_repo_at(dir.path());

    let req = json!({"op": "hello"}).to_string() + "\n";
    let (stdout, stderr, code) = run_helper(dir.path(), &req);
    assert_eq!(code, 0, "stderr={}", stderr);

    let line = stdout.lines().next().expect("hello response");
    let resp: serde_json::Value = serde_json::from_str(line)
        .expect("hello response is JSON");
    assert_eq!(resp["ok"], json!(true));
    assert!(
        resp["morph_version"].is_string(),
        "expected morph_version, got: {}",
        resp
    );
}

#[test]
fn remote_helper_lists_branches() {
    // PR5 cycle 6 RED: `list-branches` returns `(name, hash)` pairs
    // for everything under refs/heads. Drives `fetch_remote` over
    // SSH (PR5 Stage F).
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    init_repo_at(repo);

    // Make two branches via the CLI.
    std::fs::write(repo.join("a.txt"), "A").unwrap();
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["add", "a.txt"])
        .assert()
        .success();
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["commit", "-m", "first"])
        .assert()
        .success();
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["branch", "feature"])
        .assert()
        .success();

    let req = json!({"op": "list-branches"}).to_string() + "\n";
    let (stdout, stderr, code) = run_helper(repo, &req);
    assert_eq!(code, 0, "stderr={}", stderr);

    let line = stdout.lines().next().expect("response");
    let resp: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(resp["ok"], json!(true));
    let branches = resp["branches"].as_array().expect("branches array");
    let names: Vec<String> = branches
        .iter()
        .map(|b| b["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"main".to_string()), "got: {:?}", names);
    assert!(names.contains(&"feature".to_string()), "got: {:?}", names);
}

#[test]
fn remote_helper_ref_read_returns_hash_or_null() {
    // PR5 cycle 7 RED: `ref-read` returns `{ok: true, hash: "..." | null}`.
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    init_repo_at(repo);

    std::fs::write(repo.join("a.txt"), "A").unwrap();
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["add", "a.txt"])
        .assert()
        .success();
    let out = cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["commit", "-m", "first", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json_out: serde_json::Value =
        serde_json::from_slice(&out).expect("--json output");
    let head_hash = json_out["hash"].as_str().unwrap().to_string();

    // Existing ref.
    let req = json!({"op": "ref-read", "name": "heads/main"}).to_string()
        + "\n"
        + &json!({"op": "ref-read", "name": "heads/nope"}).to_string()
        + "\n";
    let (stdout, stderr, code) = run_helper(repo, &req);
    assert_eq!(code, 0, "stderr={}", stderr);
    let mut lines = stdout.lines();

    let r1: serde_json::Value =
        serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(r1["ok"], json!(true));
    assert_eq!(r1["hash"].as_str().unwrap(), head_hash);

    let r2: serde_json::Value =
        serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(r2["ok"], json!(true));
    assert!(r2["hash"].is_null(), "missing ref must be null: {}", r2);
}

#[test]
fn remote_helper_has_and_get_object() {
    // PR5 cycle 8 RED: `has` returns `{ok: true, has: bool}`;
    // `get` returns `{ok: true, object: <morph object json>}`.
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    init_repo_at(repo);

    std::fs::write(repo.join("a.txt"), "A").unwrap();
    cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["add", "a.txt"])
        .assert()
        .success();
    let out = cargo_bin_cmd!("morph")
        .current_dir(repo)
        .args(["commit", "-m", "first", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json_out: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let head_hash = json_out["hash"].as_str().unwrap().to_string();
    let bogus = "0".repeat(64);

    let req = json!({"op": "has", "hash": head_hash}).to_string()
        + "\n"
        + &json!({"op": "has", "hash": bogus}).to_string()
        + "\n"
        + &json!({"op": "get", "hash": head_hash}).to_string()
        + "\n";
    let (stdout, stderr, code) = run_helper(repo, &req);
    assert_eq!(code, 0, "stderr={}", stderr);
    let mut lines = stdout.lines();

    let r1: serde_json::Value =
        serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(r1["ok"], json!(true));
    assert_eq!(r1["has"], json!(true));

    let r2: serde_json::Value =
        serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(r2["ok"], json!(true));
    assert_eq!(r2["has"], json!(false));

    let r3: serde_json::Value =
        serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(r3["ok"], json!(true));
    assert!(
        r3["object"].is_object(),
        "expected object payload, got: {}",
        r3
    );
    assert_eq!(
        r3["object"]["type"].as_str().unwrap(),
        "commit",
        "head should be a commit, got: {}",
        r3
    );
}

#[test]
fn remote_helper_put_object_returns_hash() {
    // PR5 cycle 9 RED: `put` accepts a Morph object payload and
    // returns the content hash. SshStore uses this to push closure
    // objects to the remote during `morph push` over SSH.
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    init_repo_at(repo);

    let blob = json!({
        "type": "blob",
        "kind": "prompt",
        "content": {"text": "hello"},
    });
    let req = json!({"op": "put", "object": blob}).to_string() + "\n";
    let (stdout, stderr, code) = run_helper(repo, &req);
    assert_eq!(code, 0, "stderr={}", stderr);

    let line = stdout.lines().next().expect("put response");
    let resp: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(resp["ok"], json!(true));
    let hash = resp["hash"].as_str().expect("hash field");
    assert_eq!(hash.len(), 64);
    // Object must be retrievable on the same repo via the same helper.
    let req2 = json!({"op": "has", "hash": hash}).to_string() + "\n";
    let (stdout2, _, _) = run_helper(repo, &req2);
    let r2: serde_json::Value =
        serde_json::from_str(stdout2.lines().next().unwrap()).unwrap();
    assert_eq!(r2["has"], json!(true));
}

#[test]
fn remote_helper_unknown_op_is_an_error_response() {
    // PR5 cycle 10 RED preview: any unrecognized op must produce a
    // structured error so the SSH client can surface a real error,
    // not a silent hang.
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    init_repo_at(repo);

    let req = json!({"op": "frob"}).to_string() + "\n";
    let (stdout, stderr, code) = run_helper(repo, &req);
    assert_eq!(code, 0, "helper stays alive on unknown op, stderr={}", stderr);
    let line = stdout.lines().next().expect("error response");
    let resp: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(resp["ok"], json!(false));
    assert!(
        resp["error"].as_str().unwrap_or("").contains("frob")
            || resp["error_kind"].as_str().unwrap_or("").contains("unknown_op"),
        "expected useful error, got: {}",
        resp
    );

    // suppress unused-import warning when the binary is fast.
    let _ = BufReader::new(std::io::empty()).lines();
    let _ = Duration::from_secs(0);
}
