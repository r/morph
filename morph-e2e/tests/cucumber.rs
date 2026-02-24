//! Cucumber (Gherkin) E2E tests: .feature files are the human-readable spec;
//! this file implements the step definitions using assert_cmd and tempfile.
//!
//! All E2E behavior is expressed in Gherkin; the harness runs morph CLI and asserts.

use assert_cmd::Command;
use cucumber::{given, then, when, World as _};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

#[derive(Debug, Default, cucumber::World)]
pub struct MorphWorld {
    /// Temp directory for the current scenario (morph repo root).
    pub temp_dir: Option<TempDir>,
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
    let path = temp.path();
    Command::cargo_bin("morph")
        .expect("morph binary")
        .arg("init")
        .arg(path)
        .assert()
        .success();
    w.temp_dir = Some(temp);
}

#[given(regex = r#"a file "([^"]+)" with content "([^"]*)""#)]
fn given_file(w: &mut MorphWorld, path: String, content: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let full = root.path().join(&path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(&full, content).expect("write file");
}

#[given(expr = "the identity program and a minimal eval suite exist")]
fn given_program_and_eval_suite(w: &mut MorphWorld) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let prog = r#"{"graph":{"nodes":[{"id":"n1","kind":"identity","ref":null,"params":{}}],"edges":[]},"prompts":[],"eval_suite":null,"provenance":null}"#;
    let eval = r#"{"cases":[],"metrics":[{"name":"acc","aggregation":"mean","threshold":0.0}]}"#;
    std::fs::write(root.path().join("prog.json"), prog).expect("write prog.json");
    let evals_dir = root.path().join(".morph/evals");
    std::fs::create_dir_all(&evals_dir).expect("create evals dir");
    std::fs::write(evals_dir.join("e.json"), eval).expect("write e.json");
}

#[when(regex = r#"I run "([^"]+)""#)]
fn when_run(w: &mut MorphWorld, cmd: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let cmd = substitute_placeholders(&cmd, &w.captures);
    let parts = split_cli_args(&cmd);
    let (bin, args) = parts.split_first().expect("non-empty command");
    let output = if *bin == "morph" {
        Command::cargo_bin("morph")
            .expect("morph binary")
            .args(args)
            .current_dir(root.path())
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

#[when(regex = r#"I run commit with message "([^"]*)" using captured program and eval suite"#)]
fn when_run_commit_captured(w: &mut MorphWorld, message: String) {
    let prog = w.captures.get("prog_hash").expect("capture prog_hash first");
    let suite = w.captures.get("suite_hash").expect("capture suite_hash first");
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let output = Command::cargo_bin("morph")
        .expect("morph binary")
        .args([
            "commit",
            "-m",
            &message,
            "--program",
            prog,
            "--eval-suite",
            suite,
            "--metrics",
            "{}",
        ])
        .current_dir(root.path())
        .output()
        .expect("run morph");
    w.last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    w.last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
    w.last_exit = output.status.code();
}

#[when(regex = r#"I run record-session with prompt "([^"]*)" and response "([^"]*)""#)]
fn when_run_record_session(w: &mut MorphWorld, prompt: String, response: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
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
        .current_dir(root.path())
        .output()
        .expect("run morph");
    w.last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    w.last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
    w.last_exit = output.status.code();
}

#[then(regex = r#"stdout contains "([^"]+)""#)]
fn then_stdout_contains(w: &mut MorphWorld, needle: String) {
    assert!(
        w.last_stdout.contains(&needle),
        "stdout should contain {:?}\nstdout: {}",
        needle,
        w.last_stdout
    );
}

#[then(regex = r#"the path "([^"]+)" exists as a directory"#)]
fn then_path_is_dir(w: &mut MorphWorld, path: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let full = root.path().join(&path);
    assert!(full.exists(), "path should exist: {}", full.display());
    assert!(full.is_dir(), "path should be a directory: {}", full.display());
}

#[then(regex = r#"the path "([^"]+)" is present"#)]
fn then_path_exists(w: &mut MorphWorld, path: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let full = root.path().join(&path);
    assert!(full.exists(), "path should exist: {}", full.display());
}

#[then(regex = r#"the path "([^"]+)" does not exist"#)]
fn then_path_does_not_exist(w: &mut MorphWorld, path: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let full = root.path().join(&path);
    assert!(!full.exists(), "path should not exist: {}", full.display());
}

#[then(regex = r#"the file "([^"]+)" has content "([^"]*)""#)]
fn then_file_has_content(w: &mut MorphWorld, path: String, content: String) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let full = root.path().join(&path);
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

// --- Concurrent agents (Phase 2) ---

#[when(regex = r#"(\d+) agents run record-session concurrently"#)]
fn when_agents_run_record_session_concurrently(w: &mut MorphWorld, n: u32) {
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let root = Arc::new(root.path().to_path_buf());
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
    let root = w.temp_dir.as_ref().expect("given a morph repo first");
    let runs_dir = root.path().join(".morph/runs");
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

#[tokio::main]
async fn main() {
    // Features live in morph-e2e/features; cwd is workspace root when running cargo test.
    let features_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("features");
    MorphWorld::run(features_path.to_string_lossy().as_ref()).await;
}
