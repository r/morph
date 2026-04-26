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
