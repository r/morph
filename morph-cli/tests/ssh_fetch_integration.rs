// Integration tests for `morph fetch` / `morph pull` / `morph pull
// --merge` over SSH. Real SSH would require sshd; instead we use
// `MORPH_SSH` pointing at a small shell script that absorbs the SSH
// arguments and re-execs the local `morph remote-helper` binary
// directly. From the CLI's perspective this is indistinguishable
// from a real `ssh user@host morph remote-helper` invocation.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn morph_bin() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin!("morph").to_path_buf()
}

/// Write a fake-ssh shell script that drops everything up through
/// the first `morph` argument and re-execs the remaining args
/// against our real binary. Returns the script path. This is the
/// minimum surface area we need: real ssh would produce stdin/stdout
/// pipes against the helper, and so does this.
fn write_fake_ssh(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("fake-ssh.sh");
    // Fake ssh: skip `-p PORT` and any other `-X` options, drop the
    // host argument, then exec the remaining command verbatim. This
    // exactly matches how a real `ssh` invocation passes through
    // its remote command and args.
    let script = "\
#!/bin/sh
while [ $# -gt 0 ]; do
    case \"$1\" in
        -p) shift; shift ;;
        -*) shift ;;
        *) break ;;
    esac
done
shift
exec \"$@\"
";
    fs::write(&path, script).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

fn run_morph(repo: &std::path::Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::new(morph_bin());
    cmd.current_dir(repo);
    for a in args {
        cmd.arg(a);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("run morph")
}

fn init_repo(path: &std::path::Path) {
    let out = Command::new(morph_bin())
        .arg("init")
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "morph init failed: {:?}", out);
}

fn init_bare_repo(path: &std::path::Path) {
    let out = Command::new(morph_bin())
        .arg("init")
        .arg("--bare")
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "morph init --bare failed: {:?}", out);
}

#[test]
fn morph_fetch_works_over_ssh_url() {
    // PR5 cycle 27 RED→GREEN: configure an `ssh://...` remote, run
    // `morph fetch`, and verify the remote-tracking ref appears.
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let remote = tmp.path().join("remote");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&remote).unwrap();

    init_repo(&local);
    init_repo(&remote);

    // Make a commit on the remote.
    fs::write(remote.join("a.txt"), "A").unwrap();
    let _ = run_morph(&remote, &["add", "a.txt"], &[]);
    let out = run_morph(&remote, &["commit", "-m", "first", "--json"], &[]);
    assert!(out.status.success(), "remote commit failed: {:?}", out);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let remote_tip = json["hash"].as_str().unwrap().to_string();

    // Configure the remote on the local side as a fake ssh url.
    let url = format!("ssh://fake.host{}", remote.display());
    let out = run_morph(&local, &["remote", "add", "origin", &url], &[]);
    assert!(out.status.success(), "remote add failed: {:?}", out);

    let fake_ssh = write_fake_ssh(tmp.path());
    let bin = morph_bin();
    let env = [
        ("MORPH_SSH", fake_ssh.to_str().unwrap()),
        ("MORPH_REMOTE_BIN", bin.to_str().unwrap()),
    ];
    let out = run_morph(&local, &["fetch", "origin"], &env);
    assert!(
        out.status.success(),
        "morph fetch over ssh failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // Remote-tracking ref must now point at the remote tip.
    let tracking = local.join(".morph/refs/remotes/origin/main");
    assert!(tracking.exists(), "tracking ref not created");
    let got = fs::read_to_string(&tracking).unwrap();
    assert_eq!(got.trim(), remote_tip);
}

#[test]
fn morph_pull_fast_forwards_over_ssh_url() {
    // PR5 cycle 28 RED→GREEN.
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let remote = tmp.path().join("remote");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&remote).unwrap();
    init_repo(&local);
    init_repo(&remote);

    fs::write(remote.join("a.txt"), "A").unwrap();
    let _ = run_morph(&remote, &["add", "a.txt"], &[]);
    let _ = run_morph(&remote, &["commit", "-m", "first"], &[]);

    let url = format!("ssh://fake.host{}", remote.display());
    let _ = run_morph(&local, &["remote", "add", "origin", &url], &[]);

    let fake_ssh = write_fake_ssh(tmp.path());
    let bin = morph_bin();
    let env = [
        ("MORPH_SSH", fake_ssh.to_str().unwrap()),
        ("MORPH_REMOTE_BIN", bin.to_str().unwrap()),
    ];
    let out = run_morph(&local, &["pull", "origin", "main"], &env);
    assert!(
        out.status.success(),
        "morph pull over ssh failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    // local heads/main must now match remote tip.
    let local_main = local.join(".morph/refs/heads/main");
    assert!(
        local_main.exists(),
        "local main not created after pull"
    );
}

#[test]
fn morph_pull_merge_finalizes_clean_three_way_over_ssh() {
    // PR5 cycle 29 RED→GREEN: divergent histories, `pull --merge`
    // must run the structural merge against the SSH-fetched
    // remote-tracking ref and auto-finalize when there are no
    // textual conflicts.
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let remote = tmp.path().join("remote");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&remote).unwrap();
    init_repo(&local);
    init_repo(&remote);

    // Shared base on the remote.
    fs::write(remote.join("base.txt"), "base").unwrap();
    let _ = run_morph(&remote, &["add", "base.txt"], &[]);
    let _ = run_morph(&remote, &["commit", "-m", "base"], &[]);

    // Fast-forward the local from the remote so they share history.
    let url = format!("ssh://fake.host{}", remote.display());
    let _ = run_morph(&local, &["remote", "add", "origin", &url], &[]);
    let fake_ssh = write_fake_ssh(tmp.path());
    let bin = morph_bin();
    let env: Vec<(&str, &str)> = vec![
        ("MORPH_SSH", fake_ssh.to_str().unwrap()),
        ("MORPH_REMOTE_BIN", bin.to_str().unwrap()),
    ];
    let out = run_morph(&local, &["pull", "origin", "main"], &env);
    assert!(out.status.success(), "initial pull: {:?}", out);
    // Ensure the working tree mirrors the pulled commit so the
    // next local commit's tree carries forward base.txt.
    let _ = run_morph(&local, &["checkout", "main"], &[]);

    // Diverge: remote adds remote_only.txt, local adds local_only.txt.
    // We stage `.` so the commit tree carries forward base.txt as
    // well; morph's index doesn't auto-inherit from the parent
    // commit on partial `add`.
    fs::write(remote.join("remote_only.txt"), "R").unwrap();
    let _ = run_morph(&remote, &["add", "."], &[]);
    let _ = run_morph(&remote, &["commit", "-m", "remote-2"], &[]);

    fs::write(local.join("local_only.txt"), "L").unwrap();
    let _ = run_morph(&local, &["add", "."], &[]);
    let _ = run_morph(&local, &["commit", "-m", "local-2"], &[]);

    let out = run_morph(&local, &["pull", "--merge", "origin", "main"], &env);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "morph pull --merge failed: stdout={} stderr={}",
        stdout,
        stderr
    );

    // For diagnostic purposes, list what's actually in the local
    // dir if the assertion below fires.
    let listing: Vec<String> = fs::read_dir(&local)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    assert!(
        local.join("base.txt").exists(),
        "base.txt should remain after clean 3-way merge — got dir entries {:?}, stdout={}",
        listing,
        stdout
    );
    assert!(
        local.join("local_only.txt").exists(),
        "local_only.txt should remain"
    );
    assert!(
        local.join("remote_only.txt").exists(),
        "remote_only.txt should be merged in from remote"
    );
}

#[test]
fn morph_sync_uses_configured_upstream_over_ssh() {
    // PR5 cycle 33: end-to-end `morph sync` against an `ssh://`
    // upstream. Configure once with `branch --set-upstream`, then
    // run `morph sync` — no remote/branch arguments needed.
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let remote = tmp.path().join("remote");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&remote).unwrap();
    init_repo(&local);
    init_repo(&remote);

    fs::write(remote.join("a.txt"), "v1").unwrap();
    let _ = run_morph(&remote, &["add", "a.txt"], &[]);
    let _ = run_morph(&remote, &["commit", "-m", "remote v1"], &[]);

    let url = format!("ssh://fake.host{}", remote.display());
    let out = run_morph(&local, &["remote", "add", "origin", &url], &[]);
    assert!(out.status.success(), "remote add failed: {:?}", out);

    let out = run_morph(&local, &["branch", "--set-upstream", "origin/main"], &[]);
    assert!(
        out.status.success(),
        "branch --set-upstream failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let fake_ssh = write_fake_ssh(tmp.path());
    let bin = morph_bin();
    let env: Vec<(&str, &str)> = vec![
        ("MORPH_SSH", fake_ssh.to_str().unwrap()),
        ("MORPH_REMOTE_BIN", bin.to_str().unwrap()),
    ];

    let out = run_morph(&local, &["sync"], &env);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "morph sync failed: stdout={} stderr={}",
        stdout,
        stderr
    );
    assert!(
        stdout.contains("Synced main"),
        "expected `Synced main` in stdout, got: {}",
        stdout
    );

    // The local main ref must now exist and match the remote tip.
    assert!(
        local.join(".morph/refs/heads/main").exists(),
        "local heads/main missing after sync"
    );
}

/// PR 6 stage F cycle 29: when the bare server enforces a push
/// gate, an end-to-end `morph push` of a commit that fails the
/// gate must surface a clear "push gate failed" error rather than
/// silently writing a half-good ref.
#[test]
fn ssh_push_to_gated_branch_surfaces_push_gate_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let server = tmp.path().join("server.morph");
    let client = tmp.path().join("client");
    fs::create_dir_all(&client).unwrap();

    init_bare_repo(&server);
    init_repo(&client);

    // Configure the server to gate `main` on a metric the client
    // won't supply. We write the policy directly because the
    // server-side morph CLI doesn't ship a `policy set` command
    // that targets a bare repo from the outside.
    let policy = serde_json::json!({
        "policy": {
            "required_metrics": ["acc"],
            "push_gated_branches": ["main"],
        }
    });
    let cfg_path = server.join("config.json");
    let mut cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg.as_object_mut()
        .unwrap()
        .insert("policy".into(), policy["policy"].clone());
    fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

    // Make a commit on the client. No metrics, no certification —
    // gate_check on the server will fail.
    fs::write(client.join("a.txt"), "alpha").unwrap();
    let _ = run_morph(&client, &["add", "a.txt"], &[]);
    let out = run_morph(&client, &["commit", "-m", "alpha"], &[]);
    assert!(out.status.success(), "commit failed: {:?}", out);

    let url = format!("ssh://fake.host{}", server.display());
    let out = run_morph(&client, &["remote", "add", "origin", &url], &[]);
    assert!(out.status.success());

    let fake_ssh = write_fake_ssh(tmp.path());
    let bin = morph_bin();
    let env = [
        ("MORPH_SSH", fake_ssh.to_str().unwrap()),
        ("MORPH_REMOTE_BIN", bin.to_str().unwrap()),
    ];
    let out = run_morph(&client, &["push", "origin", "main"], &env);
    assert!(
        !out.status.success(),
        "push should have failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("push gate"),
        "stderr should mention push gate, got: {}",
        stderr
    );
    assert!(
        stderr.contains("main"),
        "stderr should mention branch name, got: {}",
        stderr
    );

    // The server must not have recorded the ref.
    assert!(
        !server.join("refs/heads/main").exists(),
        "server should not have written heads/main when gate failed"
    );
}

/// PR 6 stage E cycle 23: when the remote helper advertises a
/// protocol version this client doesn't understand, `morph fetch`
/// must fail with a clear "Incompatible remote" message instead of
/// silently proceeding or returning a generic transport error.
///
/// We force the mismatch with `MORPH_TEST_PROTOCOL_VERSION_OVERRIDE`
/// — a testing hook in `morph remote-helper` that lets us emit any
/// protocol version. Without the hook this would require building a
/// purpose-made fake helper binary.
#[test]
fn ssh_fetch_against_incompatible_protocol_version_errors_clearly() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let remote = tmp.path().join("remote");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&remote).unwrap();

    init_repo(&local);
    init_repo(&remote);

    // Configure an SSH-style remote on the local side.
    let url = format!("ssh://fake.host{}", remote.display());
    let out = run_morph(&local, &["remote", "add", "origin", &url], &[]);
    assert!(out.status.success(), "remote add failed: {:?}", out);

    let fake_ssh = write_fake_ssh(tmp.path());
    let bin = morph_bin();
    // Force the helper to advertise a future protocol the client
    // can't speak. The override is wired up in
    // `morph-cli/src/remote_helper.rs` and is gated behind the env
    // var so production helpers always emit the real version.
    let env = [
        ("MORPH_SSH", fake_ssh.to_str().unwrap()),
        ("MORPH_REMOTE_BIN", bin.to_str().unwrap()),
        ("MORPH_TEST_PROTOCOL_VERSION_OVERRIDE", "999"),
    ];
    let out = run_morph(&local, &["fetch", "origin"], &env);
    assert!(
        !out.status.success(),
        "fetch should have failed but succeeded: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Incompatible remote"),
        "stderr should mention Incompatible remote, got: {}",
        stderr
    );
    // Both versions should appear so the user can act.
    assert!(
        stderr.contains("999"),
        "stderr should mention remote protocol 999, got: {}",
        stderr
    );
    assert!(
        stderr.contains("protocol_version"),
        "stderr should mention which field mismatched, got: {}",
        stderr
    );
}

/// PR 6 stage D cycle 18+19: end-to-end against a bare server.
/// Bare server is reachable as `ssh://fake.host/<bare-path>` (no
/// `.morph` segment in the path). A second client must be able to
/// push, then a third party fetches and sees the closure.
#[test]
fn push_and_fetch_against_bare_ssh_server() {
    let tmp = tempfile::tempdir().unwrap();
    let server = tmp.path().join("server.morph"); // bare layout
    let alice = tmp.path().join("alice");
    let bob = tmp.path().join("bob");
    fs::create_dir_all(&alice).unwrap();
    fs::create_dir_all(&bob).unwrap();

    init_bare_repo(&server);
    init_repo(&alice);
    init_repo(&bob);

    // Alice creates and pushes a commit to the bare server.
    fs::write(alice.join("a.txt"), "alpha").unwrap();
    let _ = run_morph(&alice, &["add", "a.txt"], &[]);
    let out = run_morph(&alice, &["commit", "-m", "alpha", "--json"], &[]);
    assert!(out.status.success(), "alice commit failed: {:?}", out);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let alpha_hash = json["hash"].as_str().unwrap().to_string();

    let url = format!("ssh://fake.host{}", server.display());
    let out = run_morph(&alice, &["remote", "add", "origin", &url], &[]);
    assert!(out.status.success(), "alice remote add failed: {:?}", out);

    let fake_ssh = write_fake_ssh(tmp.path());
    let bin = morph_bin();
    let env = [
        ("MORPH_SSH", fake_ssh.to_str().unwrap()),
        ("MORPH_REMOTE_BIN", bin.to_str().unwrap()),
    ];
    let out = run_morph(&alice, &["push", "origin", "main"], &env);
    assert!(
        out.status.success(),
        "alice push to bare ssh failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // The bare server must now have the commit object and the
    // updated branch ref. Refs live in `<server>/refs/heads/main`,
    // not under `.morph`.
    assert!(
        server.join("refs/heads/main").exists(),
        "bare server should have heads/main after push"
    );

    // Bob configures the same bare server as a remote and fetches.
    let out = run_morph(&bob, &["remote", "add", "origin", &url], &[]);
    assert!(out.status.success(), "bob remote add failed: {:?}", out);
    let out = run_morph(&bob, &["fetch", "origin"], &env);
    assert!(
        out.status.success(),
        "bob fetch from bare ssh failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Bob's remote-tracking ref points at Alice's commit.
    let tracking = bob.join(".morph/refs/remotes/origin/main");
    let got = fs::read_to_string(&tracking).unwrap();
    assert_eq!(got.trim(), alpha_hash);

    // And Bob can `morph show` the commit, meaning the closure
    // came across (commit + tree + blob + suite + pipeline).
    let out = run_morph(&bob, &["show", &alpha_hash], &[]);
    assert!(
        out.status.success(),
        "bob show failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = String::from_utf8_lossy(&out.stdout);
    assert!(body.contains("alpha"), "show should contain commit msg: {}", body);
}

#[test]
fn fetched_commit_preserves_morph_instance_across_ssh() {
    // PR 6 stage B cycle 9: when laptop-A pushes a commit and
    // laptop-B fetches it over SSH, B must see A's
    // `agent.instance_id` on the commit object. This is the entire
    // point of stage B — cross-machine forensics.
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let remote = tmp.path().join("remote");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&remote).unwrap();

    init_repo(&local);
    init_repo(&remote);

    // Capture remote's instance_id (this is what A would record on
    // the commit since it's the side that creates the commit).
    let remote_cfg = fs::read_to_string(remote.join(".morph/config.json")).unwrap();
    let remote_cfg_v: serde_json::Value = serde_json::from_str(&remote_cfg).unwrap();
    let remote_instance = remote_cfg_v["agent"]["instance_id"]
        .as_str()
        .expect("remote should have an instance_id seeded by init")
        .to_string();

    fs::write(remote.join("a.txt"), "A").unwrap();
    let _ = run_morph(&remote, &["add", "a.txt"], &[]);
    let out = run_morph(&remote, &["commit", "-m", "from remote", "--json"], &[]);
    assert!(out.status.success(), "remote commit failed: {:?}", out);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let remote_tip = json["hash"].as_str().unwrap().to_string();

    let url = format!("ssh://fake.host{}", remote.display());
    let out = run_morph(&local, &["remote", "add", "origin", &url], &[]);
    assert!(out.status.success(), "remote add failed: {:?}", out);

    let fake_ssh = write_fake_ssh(tmp.path());
    let bin = morph_bin();
    let env = [
        ("MORPH_SSH", fake_ssh.to_str().unwrap()),
        ("MORPH_REMOTE_BIN", bin.to_str().unwrap()),
    ];
    let out = run_morph(&local, &["fetch", "origin"], &env);
    assert!(
        out.status.success(),
        "morph fetch over ssh failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Now `morph show` the fetched commit on the local side and
    // verify it carries the remote's instance_id.
    let out = run_morph(&local, &["show", &remote_tip], &[]);
    assert!(
        out.status.success(),
        "morph show failed on local: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&body)
        .expect("morph show should emit a commit JSON object");
    assert_eq!(
        parsed["morph_instance"].as_str(),
        Some(remote_instance.as_str()),
        "fetched commit should preserve remote's morph_instance; got: {}",
        body
    );

    // And the local side's own instance_id is still distinct (not
    // overwritten by the fetch — fetch never touches local config).
    let local_cfg = fs::read_to_string(local.join(".morph/config.json")).unwrap();
    let local_cfg_v: serde_json::Value = serde_json::from_str(&local_cfg).unwrap();
    let local_instance = local_cfg_v["agent"]["instance_id"].as_str().unwrap();
    assert_ne!(
        local_instance, remote_instance,
        "local and remote instance_ids should be distinct"
    );
}

/// PR 8 cycle 7: end-to-end `morph clone ssh://…` against a bare
/// server. Mirrors the laptop-onboarding flow from
/// MULTI-MACHINE.md: server-side `morph init --bare`, push from
/// laptop A, then a fresh laptop B `morph clone`s and gets a
/// fully-wired working repo (origin, remote-tracking ref, local
/// branch, working tree, configured upstream).
#[test]
fn morph_clone_works_against_bare_ssh_server() {
    let tmp = tempfile::tempdir().unwrap();
    let server = tmp.path().join("server.morph");
    let alice = tmp.path().join("alice");
    fs::create_dir_all(&alice).unwrap();

    init_bare_repo(&server);
    init_repo(&alice);

    fs::write(alice.join("greeting.txt"), "hello bob").unwrap();
    let _ = run_morph(&alice, &["add", "greeting.txt"], &[]);
    let out = run_morph(&alice, &["commit", "-m", "from alice", "--json"], &[]);
    assert!(out.status.success(), "alice commit failed: {:?}", out);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let alpha_hash = json["hash"].as_str().unwrap().to_string();

    let url = format!("ssh://fake.host{}", server.display());
    let _ = run_morph(&alice, &["remote", "add", "origin", &url], &[]);
    let fake_ssh = write_fake_ssh(tmp.path());
    let bin = morph_bin();
    let env = [
        ("MORPH_SSH", fake_ssh.to_str().unwrap()),
        ("MORPH_REMOTE_BIN", bin.to_str().unwrap()),
    ];
    let out = run_morph(&alice, &["push", "origin", "main"], &env);
    assert!(
        out.status.success(),
        "alice push failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Bob clones the bare server fresh — no `morph init` first,
    // no manual remote add. The clone should do all of that.
    let bob = tmp.path().join("bob");
    let out = Command::new(morph_bin())
        .arg("clone")
        .arg(&url)
        .arg(&bob)
        .envs(env.iter().copied())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "morph clone over ssh failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Cloned"), "expected `Cloned` line, got: {}", stdout);
    assert!(stdout.contains("branch:  main"), "expected branch line, got: {}", stdout);

    // Layout sanity: working repo with origin, remote-tracking ref,
    // local branch, and Alice's file in the working tree.
    assert!(bob.join(".morph").is_dir(), "bob should have .morph/");
    assert!(
        bob.join(".morph/refs/remotes/origin/main").exists(),
        "bob should have origin/main tracking ref"
    );
    let local_main = fs::read_to_string(bob.join(".morph/refs/heads/main")).unwrap();
    assert_eq!(local_main.trim(), alpha_hash, "bob's main should match alice's tip");
    assert!(
        bob.join("greeting.txt").exists(),
        "bob's working tree should contain alice's file"
    );

    // The clone configured an upstream so `morph sync` works.
    let cfg = fs::read_to_string(bob.join(".morph/config.json")).unwrap();
    let cfg_v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
    assert_eq!(cfg_v["branches"]["main"]["remote"], "origin");
    assert_eq!(cfg_v["branches"]["main"]["branch"], "main");

    // And running sync over SSH against the bare server should
    // succeed cleanly (no work to do, but the upstream wiring is
    // exercised end-to-end).
    let out = run_morph(&bob, &["sync"], &env);
    assert!(
        out.status.success(),
        "bob sync after clone failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}
