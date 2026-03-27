//! Cucumber (Gherkin) E2E tests: .feature files are the human-readable spec;
//! this file implements the step definitions using assert_cmd and tempfile.
//!
//! All E2E behavior is expressed in Gherkin; the harness runs morph CLI and asserts.

// morph-e2e runs the workspace's morph binary; CARGO_BIN_EXE_morph is only set for the crate that builds it.
// So we keep the deprecated API here and allow the warning.
#![allow(deprecated)]

use assert_cmd::Command;
use cucumber::{given, then, when, World as _};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tempfile::TempDir;

#[derive(Debug, Default, cucumber::World)]
pub struct MorphWorld {
    /// Temp directory for the current scenario (morph repo root).
    pub temp_dir: Option<TempDir>,
    /// Canonicalized path (resolves symlinks like /var/folders → /private/var/folders on macOS).
    pub canon_path: Option<std::path::PathBuf>,
    /// Last command stdout.
    pub last_stdout: String,
    /// Last command stderr: String,
    pub last_stderr: String,
    /// Last command exit code.
    pub last_exit: Option<i32>,
    /// Named captures from "I capture the last output as <name>" for use in <name> in later commands.
    pub captures: HashMap<String, String>,
    /// After a concurrent step: exit codes from each agent.
    pub concurrent_exit_codes: Vec<i32>,
}

impl MorphWorld {
    /// Return the canonicalized repo root path (or fall back to temp_dir path).
    fn repo_root(&self) -> &std::path::Path {
        self.canon_path
            .as_deref()
            .or_else(|| self.temp_dir.as_ref().map(|t| t.path()))
            .expect("given a morph repo first")
    }
}

/// Split CLI string into args, respecting double-quoted strings.
fn split_cli_args(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in cmd.chars() {
        match (c, in_quote) {
            ('"', _) => in_quote = !in_quote,
            (c, true) => cur.push(c),
            (c, false) if c.is_ascii_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (c, false) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn substitute_placeholders(cmd: &str, captures: &HashMap<String, String>) -> String {
    let mut out = cmd.to_string();
    for (name, value) in captures {
        let placeholder = format!("<{}>", name);
        out = out.replace(&placeholder, value);
    }
    out
}

#[given(expr = "a morph repo")]
fn given_morph_repo(w: &mut MorphWorld) {
    let temp = tempfile::tempdir().expect("temp dir");
    // Canonicalize so paths match Python Path.resolve() (e.g. /var/folders → /private/var/folders on macOS)
    let canon = temp
        .path()
        .canonicalize()
        .unwrap_or_else(|_| temp.path().to_path_buf());
    Command::cargo_bin("morph")
        .expect("morph binary")
        .arg("init")
        .arg(&canon)
        .assert()
        .success();
    w.captures.insert("repo_path".to_string(), path.to_string_lossy().to_string());
    w.temp_dir = Some(temp);
    // Store canonicalized path for assertions
    w.canon_path = Some(canon);
}

#[given(regex = r#"a second morph repo at "([^"]+)""#)]
fn given_second_repo(w: &mut MorphWorld, subdir: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let remote_path = root.path().join(&subdir);
    std::fs::create_dir_all(&remote_path).expect("create subdir");
    Command::cargo_bin("morph")
        .expect("morph binary")
        .arg("init")
        .arg(&remote_path)
        .assert()
        .success();
    w.captures.insert(subdir, remote_path.to_string_lossy().to_string());
}

#[given(regex = r#"a file "([^"]+)" with content "([^"]*)""#)]
fn given_file(w: &mut MorphWorld, path: String, content: String) {
    let root = w.repo_root();
    let full = root.join(&path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(&full, content).expect("write file");
}

#[given(regex = r#"a file "([^"]+)" in directory "([^"]+)" with content "([^"]*)""#)]
fn given_file_in_dir(w: &mut MorphWorld, path: String, subdir: String, content: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let full = root.path().join(&subdir).join(&path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(&full, content).expect("write file");
}

#[given(expr = "the identity pipeline and a minimal eval suite exist")]
fn given_pipeline_and_eval_suite(w: &mut MorphWorld) {
    let root = w.repo_root();
    let prog = r#"{"graph":{"nodes":[{"id":"n1","kind":"identity","ref":null,"params":{}}],"edges":[]},"prompts":[],"eval_suite":null,"provenance":null}"#;
    let eval = r#"{"cases":[],"metrics":[{"name":"acc","aggregation":"mean","threshold":0.0}]}"#;
    std::fs::write(root.join("prog.json"), prog).expect("write prog.json");
    let evals_dir = root.join(".morph/evals");
    std::fs::create_dir_all(&evals_dir).expect("create evals dir");
    std::fs::write(evals_dir.join("e.json"), eval).expect("write e.json");
}

#[given(expr = "an eval suite with acc and old_metric")]
fn given_suite_with_old_metric(w: &mut MorphWorld) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let prog = r#"{"graph":{"nodes":[{"id":"n1","kind":"identity","ref":null,"params":{}}],"edges":[]},"prompts":[],"eval_suite":null,"provenance":null}"#;
    let eval = r#"{"cases":[],"metrics":[{"name":"acc","aggregation":"mean","threshold":0.0},{"name":"old_metric","aggregation":"mean","threshold":0.0}]}"#;
    std::fs::write(root.path().join("prog.json"), prog).expect("write prog.json");
    let evals_dir = root.path().join(".morph/evals");
    std::fs::create_dir_all(&evals_dir).expect("create evals dir");
    std::fs::write(evals_dir.join("e.json"), eval).expect("write e.json");
}

#[when(regex = r#"^I run "([^"]+)"$"#)]
fn when_run(w: &mut MorphWorld, cmd: String) {
    let root = w.repo_root().to_path_buf();
    let cmd = substitute_placeholders(&cmd, &w.captures);
    let parts = split_cli_args(&cmd);
    let (bin, args) = parts.split_first().expect("non-empty command");
    let output = if *bin == "morph" {
        Command::cargo_bin("morph")
            .expect("morph binary")
            .args(args)
            .current_dir(&root)
            .output()
            .expect("run morph")
    } else {
        panic!("only morph commands supported, got {}", bin);
    };
    w.last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    w.last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
    w.last_exit = output.status.code();
}

#[when(regex = r#"^I run "([^"]+)" in directory "([^"]+)"$"#)]
fn when_run_in_dir(w: &mut MorphWorld, cmd: String, subdir: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let dir = root.path().join(&subdir);
    let cmd = substitute_placeholders(&cmd, &w.captures);
    let parts = split_cli_args(&cmd);
    let (bin, args) = parts.split_first().expect("non-empty command");
    let output = if *bin == "morph" {
        Command::cargo_bin("morph")
            .expect("morph binary")
            .args(args)
            .current_dir(&dir)
            .output()
            .expect("run morph")
    } else {
        panic!("only morph commands supported, got {}", bin);
    };
    w.last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    w.last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
    w.last_exit = output.status.code();
}

#[when(regex = r#"I capture the last output as "([^"]+)""#)]
fn when_capture_last_output(w: &mut MorphWorld, name: String) {
    let line = w.last_stdout.trim().lines().last().unwrap_or("").trim().to_string();
    w.captures.insert(name, line);
}

#[when(regex = r#"I create a JSON file "([^"]+)" with metrics "([^"]*)""#)]
fn when_create_json_metrics_file(w: &mut MorphWorld, path: String, kv_pairs: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let mut map = serde_json::Map::new();
    for pair in kv_pairs.split(',') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            let val: f64 = v.trim().parse().expect("metric value must be a number");
            map.insert(k.trim().to_string(), serde_json::Value::from(val));
        }
    }
    let json = serde_json::to_string_pretty(&map).expect("serialize JSON");
    let full = root.path().join(&path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(&full, json).expect("write JSON file");
}

#[when(regex = r#"I create a policy file "([^"]+)" with required "([^"]*)" and thresholds "([^"]*)""#)]
fn when_create_policy_file(w: &mut MorphWorld, path: String, required: String, thresholds_str: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let required_metrics: Vec<String> = if required.is_empty() {
        vec![]
    } else {
        required.split(',').map(|s| s.trim().to_string()).collect()
    };
    let mut thresholds = serde_json::Map::new();
    if !thresholds_str.is_empty() {
        for pair in thresholds_str.split(',') {
            if let Some((k, v)) = pair.trim().split_once('=') {
                let val: f64 = v.trim().parse().expect("threshold value must be a number");
                thresholds.insert(k.trim().to_string(), serde_json::Value::from(val));
            }
        }
    }
    let policy = serde_json::json!({
        "required_metrics": required_metrics,
        "thresholds": thresholds,
    });
    let json = serde_json::to_string_pretty(&policy).expect("serialize policy JSON");
    let full = root.path().join(&path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(&full, json).expect("write policy file");
}

#[when(regex = r#"I write file "([^"]+)" with captures and content "([^"]*)""#)]
fn when_write_file_with_captures(w: &mut MorphWorld, path: String, content: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let content = substitute_placeholders(&content, &w.captures);
    let full = root.path().join(&path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(&full, content).expect("write file");
}

#[when(regex = r#"I commit with from-run "([^"]*)" and message "([^"]*)""#)]
fn when_commit_with_from_run(w: &mut MorphWorld, run_hash: String, message: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let run_hash = substitute_placeholders(&run_hash, &w.captures);
    let output = Command::cargo_bin("morph")
        .expect("morph binary")
        .args(["commit", "-m", &message, "--from-run", &run_hash])
        .current_dir(root.path())
        .output()
        .expect("run morph");
    w.last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    w.last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
    w.last_exit = output.status.code();
}

#[when(regex = r#"I run commit with message "([^"]*)" using captured pipeline and eval suite"#)]
fn when_run_commit_captured(w: &mut MorphWorld, message: String) {
    let prog = w.captures.get("prog_hash").expect("capture prog_hash first");
    let suite = w.captures.get("suite_hash").expect("capture suite_hash first");
    let root = w.repo_root().to_path_buf();
    let output = Command::cargo_bin("morph")
        .expect("morph binary")
        .args([
            "commit",
            "-m",
            &message,
            "--pipeline",
            prog,
            "--eval-suite",
            suite,
            "--metrics",
            "{}",
        ])
        .current_dir(&root)
        .output()
        .expect("run morph");
    w.last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    w.last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
    w.last_exit = output.status.code();
}

#[when(regex = r#"I run record-session with prompt "([^"]*)" and response "([^"]*)""#)]
fn when_run_record_session(w: &mut MorphWorld, prompt: String, response: String) {
    let root = w.repo_root().to_path_buf();
    let output = Command::cargo_bin("morph")
        .expect("morph binary")
        .args([
            "run",
            "record-session",
            "--prompt",
            &prompt,
            "--response",
            &response,
        ])
        .current_dir(&root)
        .output()
        .expect("run morph");
    w.last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    w.last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
    w.last_exit = output.status.code();
}

#[then(regex = r#"stdout contains "([^"]+)""#)]
fn then_stdout_contains(w: &mut MorphWorld, needle: String) {
    let needle = substitute_placeholders(&needle, &w.captures);
    assert!(
        w.last_stdout.contains(&needle),
        "stdout should contain {:?}\nstdout: {}",
        needle,
        w.last_stdout
    );
}

#[then(regex = r#"the path "([^"]+)" exists as a directory"#)]
fn then_path_is_dir(w: &mut MorphWorld, path: String) {
    let root = w.repo_root();
    let full = root.join(&path);
    assert!(full.exists(), "path should exist: {}", full.display());
    assert!(full.is_dir(), "path should be a directory: {}", full.display());
}

#[then(regex = r#"the path "([^"]+)" is present"#)]
fn then_path_exists(w: &mut MorphWorld, path: String) {
    let root = w.repo_root();
    let full = root.join(&path);
    assert!(full.exists(), "path should exist: {}", full.display());
}

#[then(regex = r#"the path "([^"]+)" does not exist"#)]
fn then_path_does_not_exist(w: &mut MorphWorld, path: String) {
    let root = w.repo_root();
    let full = root.join(&path);
    assert!(!full.exists(), "path should not exist: {}", full.display());
}

#[then(regex = r#"the file "([^"]+)" has content "([^"]*)""#)]
fn then_file_has_content(w: &mut MorphWorld, path: String, content: String) {
    let root = w.repo_root();
    let full = root.join(&path);
    let actual = std::fs::read_to_string(&full).expect("read file");
    let actual = actual.trim_end_matches('\n');
    assert_eq!(actual, content, "file content mismatch: {}", full.display());
}

#[when(expr = "the last command succeeded")]
#[then(expr = "the last command succeeded")]
fn last_command_succeeded(w: &mut MorphWorld) {
    assert_eq!(
        w.last_exit,
        Some(0),
        "expected exit 0, got {:?}\nstderr: {}",
        w.last_exit,
        w.last_stderr
    );
}

#[when(regex = r#"I commit with message "([^"]*)" pipeline "([^"]*)" suite "([^"]*)" and metrics \{([^\}]*)\}"#)]
fn when_commit_with_metrics(w: &mut MorphWorld, msg: String, pipeline: String, suite: String, metrics_inner: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let pipeline = substitute_placeholders(&pipeline, &w.captures);
    let suite = substitute_placeholders(&suite, &w.captures);
    let metrics_json = format!("{{{}}}", metrics_inner);
    let output = Command::cargo_bin("morph")
        .expect("morph binary")
        .args(["commit", "-m", &msg, "--pipeline", &pipeline, "--eval-suite", &suite, "--metrics", &metrics_json])
        .current_dir(root.path())
        .output()
        .expect("run morph");
    w.last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    w.last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
    w.last_exit = output.status.code();
}

#[when(regex = r#"I merge "([^"]*)" with message "([^"]*)" pipeline "([^"]*)" suite "([^"]*)" and metrics \{([^\}]*)\}"#)]
fn when_merge_with_metrics(w: &mut MorphWorld, branch: String, msg: String, pipeline: String, suite: String, metrics_inner: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let pipeline = substitute_placeholders(&pipeline, &w.captures);
    let suite = substitute_placeholders(&suite, &w.captures);
    let metrics_json = format!("{{{}}}", metrics_inner);
    let output = Command::cargo_bin("morph")
        .expect("morph binary")
        .args(["merge", &branch, "-m", &msg, "--pipeline", &pipeline, "--eval-suite", &suite, "--metrics", &metrics_json])
        .current_dir(root.path())
        .output()
        .expect("run morph");
    w.last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    w.last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
    w.last_exit = output.status.code();
}

#[when(regex = r#"I merge "([^"]*)" with message "([^"]*)" pipeline "([^"]*)" and metrics \{([^\}]*)\}"#)]
fn when_merge_auto_suite(w: &mut MorphWorld, branch: String, msg: String, pipeline: String, metrics_inner: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let pipeline = substitute_placeholders(&pipeline, &w.captures);
    let metrics_json = format!("{{{}}}", metrics_inner);
    let output = Command::cargo_bin("morph")
        .expect("morph binary")
        .args(["merge", &branch, "-m", &msg, "--pipeline", &pipeline, "--metrics", &metrics_json])
        .current_dir(root.path())
        .output()
        .expect("run morph");
    w.last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    w.last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
    w.last_exit = output.status.code();
}

#[when(regex = r#"I merge "([^"]*)" with message "([^"]*)" pipeline "([^"]*)" metrics \{([^\}]*)\} and retire "([^"]*)""#)]
fn when_merge_with_retire(w: &mut MorphWorld, branch: String, msg: String, pipeline: String, metrics_inner: String, retire: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let pipeline = substitute_placeholders(&pipeline, &w.captures);
    let metrics_json = format!("{{{}}}", metrics_inner);
    let output = Command::cargo_bin("morph")
        .expect("morph binary")
        .args(["merge", &branch, "-m", &msg, "--pipeline", &pipeline, "--metrics", &metrics_json, "--retire", &retire])
        .current_dir(root.path())
        .output()
        .expect("run morph");
    w.last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    w.last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
    w.last_exit = output.status.code();
}

#[then(expr = "the last command failed")]
fn last_command_failed(w: &mut MorphWorld) {
    assert_ne!(
        w.last_exit,
        Some(0),
        "expected non-zero exit, got {:?}\nstdout: {}",
        w.last_exit,
        w.last_stdout
    );
}

#[then(regex = r#"stderr contains "([^"]+)""#)]
fn then_stderr_contains(w: &mut MorphWorld, needle: String) {
    assert!(
        w.last_stderr.contains(&needle),
        "stderr should contain {:?}\nstderr: {}",
        needle,
        w.last_stderr
    );
}

// --- Concurrent agents (Phase 2) ---

#[when(regex = r#"(\d+) agents run record-session concurrently"#)]
fn when_agents_run_record_session_concurrently(w: &mut MorphWorld, n: u32) {
    let root = Arc::new(w.repo_root().to_path_buf());
    let mut handles = Vec::with_capacity(n as usize);
    for i in 0..n {
        let root_clone = Arc::clone(&root);
        let handle = std::thread::spawn(move || {
            let output = Command::cargo_bin("morph")
                .expect("morph binary")
                .args([
                    "run",
                    "record-session",
                    "--prompt",
                    &format!("agent {} prompt", i + 1),
                    "--response",
                    &format!("agent {} response", i + 1),
                ])
                .current_dir(&*root_clone)
                .output()
                .expect("run morph");
            output.status.code().unwrap_or(-1)
        });
        handles.push(handle);
    }
    w.concurrent_exit_codes = handles
        .into_iter()
        .map(|h| h.join().expect("thread join"))
        .collect();
}

#[then(expr = "all agents succeeded")]
fn then_all_agents_succeeded(w: &mut MorphWorld) {
    for (i, &code) in w.concurrent_exit_codes.iter().enumerate() {
        assert_eq!(code, 0, "agent {} exit code {}", i + 1, code);
    }
}

#[then(regex = r#"the repo has exactly (\d+) run records"#)]
fn then_repo_has_n_run_records(w: &mut MorphWorld, n: u32) {
    let root = w.repo_root();
    let runs_dir = root.join(".morph/runs");
    let count = std::fs::read_dir(&runs_dir)
        .map(|rd| rd.count())
        .unwrap_or(0);
    assert_eq!(
        count as u32,
        n,
        "expected {} run records in .morph/runs, found {}",
        n,
        count
    );
}

// --- Hook script steps ---

/// Resolve the morph binary path so hook scripts can find it on PATH.
fn morph_bin_path() -> std::path::PathBuf {
    Command::cargo_bin("morph")
        .expect("morph binary")
        .get_program()
        .to_owned()
        .into()
}

#[when(regex = r#"I pipe into hook "([^"]+)" the JSON:"#)]
fn when_pipe_into_hook(w: &mut MorphWorld, hook_rel: String, step: &cucumber::gherkin::Step) {
    let repo_path = w.repo_root().to_string_lossy().to_string();
    let body = step.docstring.as_deref().unwrap_or("");
    // Replace <REPO> placeholder with actual temp dir path
    let body = body.replace("<REPO>", &repo_path);

    // Hook scripts live relative to the workspace root
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let hook_path = workspace_root.join(&hook_rel);
    assert!(
        hook_path.exists(),
        "hook script not found: {}",
        hook_path.display()
    );

    // Put the morph binary on PATH so the hook scripts can call `morph`
    let morph_bin = morph_bin_path();
    let morph_bin_dir = morph_bin.parent().expect("morph bin dir");
    let path_env = format!(
        "{}:{}",
        morph_bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut child = std::process::Command::new("bash")
        .arg(&hook_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PATH", &path_env)
        .current_dir(&repo_path)
        .spawn()
        .expect("spawn hook");

    if !body.trim().is_empty() {
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(body.as_bytes())
            .expect("write stdin");
    }
    // Drop stdin to close it
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait hook");
    w.last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    w.last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
    w.last_exit = output.status.code();
}

#[then(expr = "the hook exited successfully")]
fn then_hook_exited_successfully(w: &mut MorphWorld) {
    assert_eq!(
        w.last_exit,
        Some(0),
        "expected hook exit 0, got {:?}\nstderr: {}",
        w.last_exit,
        w.last_stderr
    );
}

#[then(regex = r#"the file "([^"]+)" contains "([^"]+)""#)]
fn then_file_contains(w: &mut MorphWorld, path: String, needle: String) {
    let root = w.repo_root();
    let full = root.join(&path);
    let content = std::fs::read_to_string(&full).unwrap_or_else(|e| {
        panic!("failed to read {}: {}", full.display(), e);
    });
    assert!(
        content.contains(&needle),
        "file {} should contain {:?}\ncontent: {}",
        full.display(),
        needle,
        content
    );
}

#[tokio::main]
async fn main() {
    // Features live in morph-e2e/features; cwd is workspace root when running cargo test.
    let features_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("features");
    MorphWorld::run(features_path.to_string_lossy().as_ref()).await;
}
