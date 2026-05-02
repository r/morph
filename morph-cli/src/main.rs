//! Morph CLI: read path and manual write operations.

mod cli;
mod inspect;
mod remote_helper;
#[cfg(feature = "cursor-setup")]
mod setup;

use clap::Parser;
use cli::*;
use morph_core::{
    find_repo, hex_prefix, migrate_0_0_to_0_2, migrate_0_2_to_0_3, migrate_to_latest, open_store,
    read_repo_version, require_store_version, resolve_revision, short_hash_str, Hash, MorphObject,
    ObjectType, Store, STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_0_4, STORE_VERSION_INIT,
    SUPPORTED_REPO_VERSIONS,
};
use std::path::PathBuf;

pub(crate) fn get_store(verbose: bool) -> anyhow::Result<(PathBuf, Box<dyn Store>)> {
    let cwd = std::env::current_dir()?;
    let repo_root =
        find_repo(&cwd).ok_or_else(|| anyhow::anyhow!("not a morph repository (or any parent)"))?;
    let morph_dir = repo_root.join(".morph");
    let version = read_repo_version(&morph_dir)?;
    verbose_msg(
        verbose,
        &format!("repo {} (store version {})", repo_root.display(), version),
    );
    require_store_version(&morph_dir, SUPPORTED_REPO_VERSIONS)?;
    let store = open_store(&morph_dir)?;
    Ok((repo_root, store))
}

fn parse_hash(s: &str) -> anyhow::Result<Hash> {
    Hash::from_hex(s).map_err(|e| anyhow::anyhow!("invalid hash: {}", e))
}

/// Resolve a user-supplied identifier (hash, ref, prefix) against the
/// store. Delegates to [`morph_core::resolve_revision`] so HEAD,
/// branches, tags, and short prefixes all work uniformly.
pub(crate) fn resolve_obj_hash(store: &dyn Store, s: &str) -> anyhow::Result<Hash> {
    resolve_revision(store, s).map_err(|e| anyhow::anyhow!("{}", e))
}

fn verbose_msg(on: bool, msg: &str) {
    if on {
        eprintln!("morph: {}", msg);
    }
}

/// Default destination directory for `morph clone`. Mirrors git:
/// the basename of the URL, minus a trailing `.morph` if present.
/// e.g. `you@host:repos/myproject.morph` → `myproject`.
///
/// We delegate URL discrimination to `SshUrl::parse` so this stays
/// in sync with the rest of the SSH plumbing (IPv6 brackets, SCP
/// form, etc.). Local paths fall through to plain basename logic.
fn default_clone_dest(url: &str) -> String {
    let path = match morph_core::ssh_store::SshUrl::parse(url) {
        Some(parsed) => parsed.path,
        None => url.to_string(),
    };
    let trimmed = path.trim_end_matches('/');
    let after_slash = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let base = after_slash.trim_end_matches(".morph");
    if base.is_empty() {
        "morph-clone".to_string()
    } else {
        base.to_string()
    }
}

/// Lower-case ASCII slug suitable for the local-part of a synthetic
/// email when only a bare name is available. Anything outside
/// `[a-z0-9._-]` is dropped; an empty result falls back to "user".
fn slug_for_email(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect();
    if cleaned.is_empty() {
        "user".to_string()
    } else {
        cleaned
    }
}

/// After `morph eval run` (or `eval from-output --record`) persists
/// a metric-bearing Run, drop a `LAST_RUN.json` breadcrumb under
/// `.morph/` so the next `morph commit` (without explicit
/// `--from-run`/`--metrics`) can auto-attach the run's metrics +
/// provenance. Failures are demoted to a stderr warning so a slow
/// disk or read-only `.morph/` never breaks the eval flow itself.
fn write_last_run_breadcrumb(store: &dyn Store, repo_root: &std::path::Path, run_hash: &Hash) {
    let morph_dir = repo_root.join(".morph");
    if let Err(e) = morph_core::record_last_run(store, &morph_dir, run_hash) {
        eprintln!("warning: could not write LAST_RUN breadcrumb: {}", e);
    }
}

/// Run the repository's configured `commit.test_command` (Phase 2,
/// v0.44+) before a commit is recorded, parse its output, and write
/// a fresh `LAST_RUN.json` breadcrumb so the commit's metric
/// resolution path picks the metrics up via the existing auto-run
/// machinery.
///
/// Returns:
/// - `Ok(true)` when the test command ran (or was found stale-and-
///   re-run); the breadcrumb is now fresh and the caller should
///   re-resolve `auto_run_hash` to pick it up.
/// - `Ok(false)` when no auto-run was warranted: `--no-test` was set,
///   `--from-run` already provides evidence, no command is
///   configured, or the existing breadcrumb is fresh and
///   `--rerun` wasn't passed.
/// - `Err` only on configuration / parse / shell-out failures.
///
/// On a non-zero exit from the configured command, the commit is
/// aborted: a failing test is treated as evidence the code is not in
/// a committable state. Override with `--no-test` (or fix the test).
fn maybe_run_configured_test(
    store: &dyn Store,
    repo_root: &std::path::Path,
    morph_dir: &std::path::Path,
    no_test: bool,
    rerun: bool,
    has_from_run: bool,
) -> anyhow::Result<bool> {
    if no_test || has_from_run {
        return Ok(false);
    }
    let command = match morph_core::read_commit_test_command(morph_dir)? {
        Some(c) if !c.trim().is_empty() => c,
        _ => return Ok(false),
    };
    if !rerun {
        // Reuse a fresh breadcrumb: skip the (potentially expensive)
        // shell-out when the most recent `morph eval run` already
        // covers this commit.
        if let Ok((Some(_), _)) = morph_core::resolve_fresh_last_run(store, morph_dir) {
            return Ok(false);
        }
    }
    let argv = shlex::split(&command).ok_or_else(|| {
        anyhow::anyhow!(
            "commit.test_command is not parseable as POSIX shell argv: {:?}",
            command
        )
    })?;
    if argv.is_empty() {
        return Ok(false);
    }
    eprintln!("running configured test command: {}", command);
    let outcome = morph_core::run_test_command(store, repo_root, &argv, "auto", None)?;
    if let Some(code) = outcome.exit_code {
        if code != 0 {
            // Drop the breadcrumb anyway so the failing run is
            // inspectable via `morph show <hash>`. Then bail —
            // committing on a failing test would attach negative
            // evidence the merge gate would later reject anyway.
            write_last_run_breadcrumb(store, repo_root, &outcome.run_hash);
            return Err(anyhow::anyhow!(
                "commit.test_command exited with code {} (run {}). \
                 Fix the failure and re-run, or pass `--no-test` to \
                 commit without behavioral evidence.",
                code,
                outcome.run_hash.short()
            ));
        }
    }
    write_last_run_breadcrumb(store, repo_root, &outcome.run_hash);
    Ok(true)
}

/// Print a one-line policy summary at the end of `morph init` so a
/// fresh user can see the behavioral gate they're under (and how to
/// change it) without running `morph policy show`. The output is
/// purely informational; it never fails the init.
fn print_policy_summary(policy: &morph_core::RepoPolicy) {
    if policy.required_metrics.is_empty() {
        println!("  policy: relaxed (metrics optional) — tighten with `morph policy init`");
    } else {
        println!(
            "  policy: strict (requires {}) — loosen with `morph policy require-metrics`",
            policy.required_metrics.join(", ")
        );
    }
}

/// Phase 4.1 (v0.46+): handler for `morph eval add` (the flat
/// spelling that replaced the v0.46-deprecated, v0.48-removed
/// `morph eval add-case`). Ingests one or more spec files /
/// directories of specs as acceptance cases and either extends the
/// policy default suite or builds a fresh one.
fn do_eval_add(
    verbose: bool,
    paths: Vec<PathBuf>,
    suite: Option<String>,
    no_default: bool,
    no_set_default: bool,
) -> anyhow::Result<()> {
    if paths.is_empty() {
        return Err(anyhow::anyhow!(
            "no paths supplied. Usage: morph eval add <file_or_dir>..."
        ));
    }
    let (repo_root, store) = get_store(verbose)?;
    let cases = morph_core::add_cases_from_paths(&paths)?;
    if cases.is_empty() {
        return Err(anyhow::anyhow!(
            "no acceptance cases found in: {}",
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let morph_dir = repo_root.join(".morph");
    let policy = morph_core::read_policy(&morph_dir)?;
    let prev: Option<morph_core::Hash> = if no_default {
        None
    } else if let Some(s) = suite.as_deref() {
        let resolved = resolve_obj_hash(store.as_ref(), s)?;
        Some(resolved)
    } else {
        match policy.default_eval_suite.as_deref() {
            Some(h) => Some(morph_core::Hash::from_hex(h)?),
            None => None,
        }
    };
    let new_hash = morph_core::build_or_extend_suite(store.as_ref(), prev, &cases)?;
    if !no_set_default {
        let mut updated = policy.clone();
        updated.default_eval_suite = Some(new_hash.to_string());
        morph_core::write_policy(&morph_dir, &updated)?;
    }
    eprintln!(
        "Added {} case{} to suite {}",
        cases.len(),
        if cases.len() == 1 { "" } else { "s" },
        new_hash
    );
    println!("{}", new_hash);
    Ok(())
}

/// Phase 4.1 (v0.46+): handler for `morph eval rebuild` (the flat
/// spelling that replaced the v0.46-deprecated, v0.48-removed
/// `morph eval suite-from-specs`). Walks the supplied directories,
/// ingests every `*.yaml` / `*.yml` / `*.feature`, and replaces the
/// default suite with the result.
fn do_eval_rebuild(verbose: bool, paths: Vec<PathBuf>, no_set_default: bool) -> anyhow::Result<()> {
    if paths.is_empty() {
        return Err(anyhow::anyhow!(
            "no paths supplied. Usage: morph eval rebuild <dir>..."
        ));
    }
    let (repo_root, store) = get_store(verbose)?;
    let cases = morph_core::add_cases_from_paths(&paths)?;
    if cases.is_empty() {
        return Err(anyhow::anyhow!(
            "no acceptance cases found in: {}",
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let new_hash = morph_core::build_or_extend_suite(store.as_ref(), None, &cases)?;
    let morph_dir = repo_root.join(".morph");
    if !no_set_default {
        let mut policy = morph_core::read_policy(&morph_dir)?;
        policy.default_eval_suite = Some(new_hash.to_string());
        morph_core::write_policy(&morph_dir, &policy)?;
    }
    eprintln!(
        "Built fresh suite with {} case{}: {}",
        cases.len(),
        if cases.len() == 1 { "" } else { "s" },
        new_hash
    );
    println!("{}", new_hash);
    Ok(())
}

/// Phase 4.1 (v0.46+): handler for `morph eval show` (the flat
/// spelling that replaced the v0.46-deprecated, v0.48-removed
/// `morph eval suite-show`). Prints the contents of the default
/// suite (or `--suite <hash>`) in human-readable form, or as JSON
/// when `--json` is set.
fn do_eval_show(verbose: bool, suite: Option<String>, json: bool) -> anyhow::Result<()> {
    let (repo_root, store) = get_store(verbose)?;
    let morph_dir = repo_root.join(".morph");
    let policy = morph_core::read_policy(&morph_dir)?;
    let target_hash: morph_core::Hash = match suite.as_deref() {
        Some(s) => resolve_obj_hash(store.as_ref(), s)?,
        None => match policy.default_eval_suite.as_deref() {
            Some(h) => morph_core::Hash::from_hex(h)?,
            None => {
                return Err(anyhow::anyhow!(
                    "no suite hash supplied and policy.default_eval_suite is unset. \
                     Run `morph eval add <spec>` first or pass `--suite <hash>`."
                ));
            }
        },
    };
    let obj = store.get(&target_hash)?;
    let suite_obj = match obj {
        morph_core::MorphObject::EvalSuite(s) => s,
        _ => {
            return Err(anyhow::anyhow!(
                "object {} is not an EvalSuite",
                target_hash
            ));
        }
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&suite_obj)?);
    } else {
        println!("Suite {}", target_hash);
        println!("  cases:    {}", suite_obj.cases.len());
        println!("  metrics:  {}", suite_obj.metrics.len());
        for c in &suite_obj.cases {
            let kind = c.input.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
            println!("    - {}  [{}]  metric={}", c.id, kind, c.metric);
        }
        for m in &suite_obj.metrics {
            println!(
                "    metric: {} agg={} threshold={} dir={}",
                m.name, m.aggregation, m.threshold, m.direction
            );
        }
    }
    Ok(())
}

/// Phase 4.1 (v0.46+): shared body for the four `morph session`
/// subcommands and the deprecated `morph run *` aliases. Dispatches
/// to the existing record/list/show/export plumbing in `morph_core`
/// and `inspect`.
fn do_session_dispatch(verbose: bool, sub: SessionCmd) -> anyhow::Result<()> {
    match sub {
        SessionCmd::List { json } => do_session_list(verbose, json),
        SessionCmd::Show {
            hash,
            json,
            with_trace,
        } => do_session_show(verbose, hash, json, with_trace),
        SessionCmd::Record {
            prompt,
            response,
            messages,
            model_name,
            agent_id,
        } => do_session_record(verbose, prompt, response, messages, model_name, agent_id),
        SessionCmd::Import {
            run_file,
            trace,
            artifact,
        } => do_session_import(verbose, run_file, trace, artifact),
        SessionCmd::Export {
            mode,
            output,
            model,
            agent,
            min_steps,
        } => inspect::run_export(
            verbose,
            &mode,
            output.as_deref(),
            model,
            agent,
            Some(min_steps),
        ),
    }
}

/// Phase 4.3 (v0.48+): JSON-ingest path for a pre-built Run object.
/// Folded under `morph session import` from the now-removed
/// `morph run record <run.json>`. Used by automation that builds Run
/// objects out of band — CI pipelines, MCP bridges, and the
/// `morph_run_record` MCP tool.
fn do_session_import(
    verbose: bool,
    run_file: PathBuf,
    trace: Option<PathBuf>,
    artifact: Vec<PathBuf>,
) -> anyhow::Result<()> {
    let (repo_root, store) = get_store(verbose)?;
    let full_run = if run_file.is_absolute() {
        run_file
    } else {
        repo_root.join(&run_file)
    };
    let trace_opt = trace.map(|t| {
        if t.is_absolute() {
            t
        } else {
            repo_root.join(&t)
        }
    });
    let artifact_paths: Vec<_> = artifact
        .iter()
        .map(|a| {
            if a.is_absolute() {
                a.clone()
            } else {
                repo_root.join(a)
            }
        })
        .collect();
    let refs: Vec<_> = artifact_paths.iter().map(|p| p.as_path()).collect();
    println!(
        "{}",
        morph_core::record_run(&store, &full_run, trace_opt.as_deref(), &refs)?
    );
    Ok(())
}

fn do_session_list(verbose: bool, json: bool) -> anyhow::Result<()> {
    let (_repo_root, store) = get_store(verbose)?;
    let runs = store.list(ObjectType::Run)?;
    if json {
        let entries: Vec<_> = runs
            .iter()
            .map(|h| {
                let h_str = h.to_string();
                let mut entry = serde_json::json!({
                    "hash": h_str,
                    "short": short_hash_str(&h_str),
                });
                if let Ok(MorphObject::Run(r)) = store.get(h) {
                    entry["agent_id"] = serde_json::Value::String(r.agent.id.clone());
                    entry["agent_version"] = serde_json::Value::String(r.agent.version.clone());
                    entry["model"] = serde_json::Value::String(r.environment.model.clone());
                    entry["pipeline"] = serde_json::Value::String(r.pipeline.clone());
                    if let Some(c) = &r.commit {
                        entry["commit"] = serde_json::Value::String(c.clone());
                    }
                    entry["has_metrics"] = serde_json::Value::Bool(!r.metrics.is_empty());
                    if !r.metrics.is_empty() {
                        if let Ok(m) = serde_json::to_value(&r.metrics) {
                            entry["metrics"] = m;
                        }
                    }
                }
                entry
            })
            .collect();
        let body = serde_json::json!({ "runs": entries, "count": runs.len() });
        println!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        for h in runs {
            println!("{}", h);
        }
    }
    Ok(())
}

fn do_session_show(
    verbose: bool,
    hash: String,
    json: bool,
    with_trace: bool,
) -> anyhow::Result<()> {
    let (_repo_root, store) = get_store(verbose)?;
    let hash = resolve_obj_hash(store.as_ref(), &hash)?;
    let obj = store.get(&hash)?;
    match &obj {
        MorphObject::Run(run) => {
            if json {
                println!("{}", serde_json::to_string_pretty(run)?);
            } else {
                println!(
                    "run    {}\ntrace  {}\npipeline {}\nagent  {} {}",
                    hash, run.trace, run.pipeline, run.agent.id, run.agent.version
                );
                if let Some(ref c) = run.commit {
                    println!("commit {}", c);
                }
                if !run.metrics.is_empty() {
                    println!("metrics {:?}", run.metrics);
                }
            }
            if with_trace {
                let trace_obj = store.get(&parse_hash(&run.trace)?)?;
                if let MorphObject::Trace(t) = &trace_obj {
                    println!();
                    inspect::print_trace_events(t);
                } else {
                    anyhow::bail!("object {} is not a trace", run.trace);
                }
            }
        }
        _ => anyhow::bail!("object {} is not a run", hash),
    }
    Ok(())
}

fn do_session_record(
    verbose: bool,
    prompt: Option<String>,
    response: Option<String>,
    messages: Option<String>,
    model_name: Option<String>,
    agent_id: Option<String>,
) -> anyhow::Result<()> {
    let (_repo_root, store) = get_store(verbose)?;
    let hash = if let Some(ref json) = messages {
        let msgs: Vec<morph_core::ConversationMessage> = serde_json::from_str(json)
            .map_err(|e| anyhow::anyhow!("invalid --messages JSON: {}", e))?;
        morph_core::record_conversation(&store, &msgs, model_name.as_deref(), agent_id.as_deref())?
    } else {
        morph_core::record_session(
            &store,
            prompt.as_deref().unwrap_or(""),
            response.as_deref().unwrap_or(""),
            model_name.as_deref(),
            agent_id.as_deref(),
        )?
    };
    println!("{}", hash);
    Ok(())
}

/// Dispatch a git-hook event into the right per-event handler. Called
/// from `morph hook <event>`, which is what every installed
/// reference-mode hook stub `exec`s. Errors out loudly when:
///
///   - the repo isn't found (we shouldn't have been invoked at all),
///   - the repo isn't in reference mode (likewise),
///   - or the event name isn't on the supported list (a stale stub
///     from a future binary, or a typo).
///
/// Output is best-effort: the installed shell stubs already redirect
/// stdout/stderr away and `|| true` away the exit code, so a useful
/// message printed here lands in user-visible terminals only when the
/// user runs `morph hook ...` themselves.
fn run_hook(event: &str, args: &[String]) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let repo_root =
        morph_core::find_repo(&cwd).ok_or_else(|| anyhow::anyhow!("not in a morph repository"))?;
    let morph_dir = repo_root.join(".morph");
    let store = morph_core::open_store(&morph_dir)?;
    let version = Some(env!("CARGO_PKG_VERSION"));

    match event {
        "post-commit" => {
            // post-commit fires per local `git commit`, so the typical
            // outcome is `created == 1`. We still honour `created > 1`
            // here in case the user disabled the hook for a stretch
            // and a later `git commit` triggers `sync_to_head`'s
            // walk-back-to-last-mirrored logic — better than silently
            // under-reporting.
            let outcome = morph_core::sync_to_head(store.as_ref(), &repo_root, version)?;
            if outcome.already_synced {
                println!("Already up to date.");
            } else if let Some(sha) = outcome.git_sha.as_deref() {
                let plural = if outcome.created == 1 {
                    "commit"
                } else {
                    "commits"
                };
                println!(
                    "Synced {} git {} (HEAD {}).",
                    outcome.created,
                    plural,
                    hex_prefix(sha, 8),
                );
            }
        }
        "post-checkout" => {
            // git passes: <prev_sha> <new_sha> <branch_flag>
            let prev = args.first().map(String::as_str).unwrap_or("");
            let new = args.get(1).map(String::as_str).unwrap_or("");
            let flag = args.get(2).map(String::as_str).unwrap_or("0");
            let outcome =
                morph_core::handle_post_checkout(store.as_ref(), &repo_root, prev, new, flag)?;
            match outcome {
                morph_core::CheckoutOutcome::SwitchedBranch { branch, .. } => {
                    println!("morph HEAD now tracks git branch '{}'.", branch);
                }
                morph_core::CheckoutOutcome::DetachedHead => {
                    println!("git HEAD detached; morph HEAD unchanged.");
                }
                morph_core::CheckoutOutcome::NoMatchingMorphCommit { git_sha } => {
                    println!(
                        "no morph commit mirrors git {}; run `morph reference-sync` to create one.",
                        hex_prefix(&git_sha, 8)
                    );
                }
                morph_core::CheckoutOutcome::FileCheckout => {}
            }
        }
        "post-rewrite" => {
            // git passes: <command>; stdin is "<old> <new> [extra]" lines.
            let command = args.first().map(String::as_str).unwrap_or("rebase");
            let mut buf = String::new();
            use std::io::Read;
            std::io::stdin().read_to_string(&mut buf)?;
            let outcome = morph_core::handle_post_rewrite(
                store.as_ref(),
                &repo_root,
                command,
                &buf,
                version,
            )?;
            println!(
                "Rewrote {} commit(s) ({} annotated as 'rewritten').",
                outcome.rewrites.len(),
                outcome.annotated
            );
        }
        "post-merge" => {
            // git passes <is_squash> as $1 (we ignore it; sync_to_head
            // is idempotent either way).
            //
            // post-merge fires for both non-fast-forward `git merge`
            // (one new merge commit) and fast-forward `git pull`s
            // that bring in many remote commits at once. In the
            // multi-commit FF case `sync_to_head` walks back to the
            // last-mirrored ancestor and mirrors every commit in the
            // span, so `created` may be > 1.
            let outcome = morph_core::sync_to_head(store.as_ref(), &repo_root, version)?;
            if outcome.already_synced {
                println!("Already up to date.");
            } else if let Some(sha) = outcome.git_sha.as_deref() {
                let plural = if outcome.created == 1 {
                    "commit"
                } else {
                    "commits"
                };
                println!(
                    "Synced {} git {} (HEAD {}).",
                    outcome.created,
                    plural,
                    hex_prefix(sha, 8),
                );
            }
        }
        "pre-merge-commit" => {
            // PR 10: Solo-submode gate. Fires while git is mid-merge
            // (`.git/MERGE_HEAD` exists). Resolves both parents'
            // morph mirrors, runs dominance against the
            // worse-of-parents bar, and exits 1 with explanation if
            // the merge would regress. "No claim" stays a warning.
            let outcome = morph_core::handle_pre_merge_commit(store.as_ref(), &repo_root, version)?;
            for side in &outcome.no_claim_sides {
                eprintln!(
                    "morph: no morph evidence on '{}' — pre-merge gate proceeds without behavioral assertion from this side",
                    side
                );
            }
            if !outcome.violations.is_empty() {
                eprintln!(
                    "morph: pre-merge gate blocked the merge — merged result would regress on certified parent metrics:"
                );
                for v in &outcome.violations {
                    eprintln!("  {}", v);
                }
                eprintln!(
                    "  resolve by re-running tests on the merged tree and using `morph merge` (which gates with the merged metrics) or set MORPH_NO_GATE=1 to override one-off"
                );
                std::process::exit(1);
            }
        }
        other => {
            anyhow::bail!(
                "unknown hook event '{}': expected one of post-commit, post-checkout, post-rewrite, post-merge, pre-merge-commit",
                other
            );
        }
    }
    Ok(())
}

/// PR 5: `morph commit` in reference mode wraps `git commit` so the
/// two stay 1:1 by construction. The wrapper:
///   1. resolves observed metrics from `--metrics` / `--from-run` /
///      `LAST_RUN.json` (same precedence as standalone),
///   2. enforces `policy.required_metrics` *before* running git so a
///      policy reject doesn't leak a stranded git commit,
///   3. shells out to `git commit -m <msg>` with `MORPH_INTERNAL=1` so
///      the post-commit hook short-circuits,
///   4. mirrors the new git HEAD into a morph commit with
///      `morph_origin = "cli"` (distinct from passive hook commits),
///   5. attaches a `kind: "certification"` annotation when metrics are
///      present so the merge gate can read them via
///      `effective_metrics`,
///   6. attaches a `kind: "introduces_cases"` annotation when
///      `--new-cases` is provided.
#[allow(clippy::too_many_arguments)]
fn run_reference_commit(
    store: &dyn Store,
    repo_root: &std::path::Path,
    morph_dir: &std::path::Path,
    version: &str,
    message: &str,
    metrics: Option<&str>,
    from_run: Option<&str>,
    new_cases: Option<&str>,
    eval_suite: Option<&str>,
    pipeline: Option<&str>,
    author: Option<&str>,
    allow_empty_metrics: bool,
    allow_empty_commit: bool,
    no_auto_run: bool,
    no_test: bool,
    rerun: bool,
    json: bool,
) -> anyhow::Result<()> {
    let prog_hash: Option<Hash> = pipeline.map(|s| resolve_obj_hash(store, s)).transpose()?;
    let policy = morph_core::read_policy(morph_dir)?;

    // Phase 2 (v0.44+): when `commit.test_command` is configured and
    // the user didn't already supply evidence, run it now so the
    // breadcrumb is fresh by the time `auto_run_hash` is resolved
    // below. Skipped under `--no-test`, when `--from-run` already
    // attaches a Run, or when a fresh breadcrumb already exists
    // (unless `--rerun` is passed).
    if !no_auto_run {
        maybe_run_configured_test(
            store,
            repo_root,
            morph_dir,
            no_test,
            rerun,
            from_run.is_some(),
        )?;
    }

    let auto_run_hash: Option<Hash> = if no_auto_run || from_run.is_some() {
        None
    } else {
        match morph_core::resolve_fresh_last_run(store, morph_dir) {
            Ok((Some(last), _)) => Hash::from_hex(&last.run).ok(),
            _ => None,
        }
    };

    let mut observed_metrics: std::collections::BTreeMap<String, f64> = metrics
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?
        .unwrap_or_default();

    if observed_metrics.is_empty() {
        // Both paths consume Run.metrics → Commit.observed_metrics
        // when the user didn't pass `--metrics` explicitly. The
        // banner that announces the attach is emitted once, below,
        // by the unified `evidence_run_hash` resolution block so
        // we don't print twice for the auto_run path.
        if let Some(s) = from_run {
            let run_hash = resolve_obj_hash(store, s)?;
            if let Ok(MorphObject::Run(run)) = store.get(&run_hash) {
                observed_metrics = run.metrics.clone();
            }
        } else if let Some(ref run_hash) = auto_run_hash {
            if let Ok(MorphObject::Run(run)) = store.get(run_hash) {
                if !run.metrics.is_empty() {
                    observed_metrics = run.metrics.clone();
                }
            }
        }
    }

    if !allow_empty_metrics {
        let missing = morph_core::missing_required_metrics(&policy, &observed_metrics);
        if !missing.is_empty() {
            return Err(anyhow::anyhow!(
                "policy requires metrics that are missing: [{}]. \
                 Pass --metrics with these keys, run `morph eval record`, \
                 or override with --allow-empty-metrics.",
                missing.join(", ")
            ));
        }
    }

    // Stage every tracked & untracked working-tree change before the
    // git commit. `morph commit` is a behavioral checkpoint at HEAD —
    // users expect "whatever's in the working tree" to land, the same
    // way standalone-mode `morph commit` walked the working tree
    // directly. Auto-staging keeps the agent flow ("edit files, then
    // morph commit") working without an extra `morph add` step.
    if let Err(e) = std::process::Command::new("git")
        .current_dir(repo_root)
        .args(["add", "-A", "--"])
        .status()
    {
        eprintln!("warning: git add -A failed before commit: {}", e);
    }

    // Default to `--allow-empty` so morph commits remain valid as
    // pure behavioral checkpoints (recording a fresh run / metric
    // bundle without code change). Strict-git users opt out with
    // explicit git commits. Honors `--allow-empty-commit` as a
    // no-op (always-true here); leaves the flag in place so callers
    // who pass it surface intent in scripts.
    let _ = allow_empty_commit;
    // Resolve author through morph's identity chain (explicit flag >
    // MORPH_AUTHOR_* env > .morph/config user.name/email) and pass it
    // to `git commit --author=...`. Without this, `morph config
    // user.name X` would silently lose to whatever git's own config /
    // env says, breaking the round-trip between `morph config` and
    // `morph show`.
    let resolved_author = morph_core::resolve_author_for_repo(morph_dir, author)
        .ok()
        .filter(|s| !s.is_empty())
        // git's `--author` insists on `Name <email>` — refuses bare
        // names. Morph identity allows email-less authors, so when
        // we only have a name we synthesise a placeholder `<>` to
        // satisfy git's parser. The author string Morph stores comes
        // from `git log %aN <%aE>` after the commit, so the
        // placeholder is preserved verbatim, which is fine for tests
        // and acceptable for users (they can reset with --author or
        // morph config).
        .map(|a| {
            if a.contains('<') && a.contains('>') {
                a
            } else {
                format!("{} <{}@morph.local>", a, slug_for_email(&a))
            }
        });
    let new_git_sha = morph_core::run_git_commit_with_morph_internal(
        repo_root,
        message,
        true,
        resolved_author.as_deref(),
    )
    .map_err(|e| anyhow::anyhow!("{}", e))?;

    let outcome = morph_core::sync_to_head_with_origin(store, repo_root, "cli", Some(version))?;
    let mirrored_hash = outcome.new_commit.ok_or_else(|| {
        anyhow::anyhow!(
            "git commit {} did not produce a new morph commit (already mirrored?)",
            hex_prefix(&new_git_sha, 12)
        )
    })?;

    // Reference-mode mirror commits are created with empty inline
    // metrics + an empty pipeline + an empty eval-suite by
    // `sync_one_commit`. `morph commit` is the user-driven path,
    // and its callers expect inline metrics, the user-supplied
    // pipeline, and a real suite hash so commits look the same as
    // they did in standalone mode. Resolve the user's choices,
    // re-derive the commit object with those fields, and advance
    // the branch ref onto the rewritten hash. The mirror commit
    // produced moments ago is harmlessly orphaned (still
    // reachable by hash, but no ref).
    let policy_for_suite = morph_core::read_policy(morph_dir).ok();
    let suite_hash_str = match eval_suite {
        Some(s) => Some(resolve_obj_hash(store, s)?.to_string()),
        None => match policy_for_suite
            .as_ref()
            .and_then(|p| p.default_eval_suite.as_deref())
        {
            Some(s) => Some(resolve_obj_hash(store, s)?.to_string()),
            None => None,
        },
    };
    // Auto-attach evidence_refs from --from-run / breadcrumb so the
    // commit links back to the originating run — matches standalone
    // mode's implicit "the run that produced these metrics is the
    // evidence" linkage. Explicit --from-run with a nonexistent hash
    // is a hard error: silently swallowing it would let mistyped
    // hashes land uncertified commits.
    let (evidence_run_hash, mut evidence_refs): (Option<Hash>, Option<Vec<String>>) =
        if let Some(s) = from_run {
            let h = resolve_obj_hash(store, s)?;
            let run = match store.get(&h) {
                Ok(MorphObject::Run(r)) => r,
                Ok(_) => {
                    return Err(anyhow::anyhow!(
                        "--from-run target {} is not a Run object",
                        s
                    ));
                }
                Err(_) => {
                    return Err(anyhow::anyhow!("--from-run target {} not found", s));
                }
            };
            // Always print the evidence-attach banner (with metrics
            // preview when available) so audit tooling sees the
            // same trail whether evidence came from --from-run or
            // the LAST_RUN breadcrumb.
            let preview: Vec<String> = run
                .metrics
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect();
            eprintln!(
                "attaching evidence from run {}: {}",
                h.short(),
                preview.join(", "),
            );
            // evidence_refs link both the Run and its Trace so
            // downstream consumers (`morph show`, certify gating,
            // SSH push closure) reach the full provenance graph
            // from a single starting hash.
            let mut refs = vec![h.to_string()];
            if !run.trace.is_empty() {
                refs.push(run.trace.clone());
            }
            (Some(h), Some(refs))
        } else if let Some(rh) = auto_run_hash {
            let mut refs = vec![rh.to_string()];
            if let Ok(MorphObject::Run(r)) = store.get(&rh) {
                if !r.trace.is_empty() {
                    refs.push(r.trace.clone());
                }
                let preview: Vec<String> = r
                    .metrics
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect();
                eprintln!(
                    "attaching evidence from run {}: {}",
                    rh.short(),
                    preview.join(", "),
                );
            }
            (Some(rh), Some(refs))
        } else {
            (None, None)
        };
    // Silence dead-store warning in branches where we don't override
    // evidence_refs further; explicit reassignment keeps intent clear.
    let _ = &mut evidence_refs;

    // Pull env_constraints + contributors off the evidence Run so
    // the rewritten commit looks the same as a standalone-mode
    // commit produced via the run-recording flow. Run.environment
    // is a structured record (model, version, parameters,
    // toolchain); we project it onto the commit's
    // `env_constraints: BTreeMap<String, Value>` field by name so
    // downstream tools (like merge gating's environment pinning)
    // can reason about it directly.
    let mut env_constraints: Option<std::collections::BTreeMap<String, serde_json::Value>> = None;
    let mut commit_contributors: Option<Vec<morph_core::CommitContributor>> = None;
    if let Some(rh) = evidence_run_hash {
        if let Ok(MorphObject::Run(run)) = store.get(&rh) {
            let mut map: std::collections::BTreeMap<String, serde_json::Value> =
                std::collections::BTreeMap::new();
            if !run.environment.model.is_empty() {
                map.insert(
                    "model".to_string(),
                    serde_json::Value::String(run.environment.model.clone()),
                );
            }
            if !run.environment.version.is_empty() {
                map.insert(
                    "version".to_string(),
                    serde_json::Value::String(run.environment.version.clone()),
                );
            }
            if !run.environment.parameters.is_empty() {
                if let Ok(v) = serde_json::to_value(&run.environment.parameters) {
                    map.insert("parameters".to_string(), v);
                }
            }
            if !run.environment.toolchain.is_empty() {
                if let Ok(v) = serde_json::to_value(&run.environment.toolchain) {
                    map.insert("toolchain".to_string(), v);
                }
            }
            if !map.is_empty() {
                env_constraints = Some(map);
            }
            // Project Run.contributors (Vec<ContributorInfo>) +
            // Run.agent (the primary contributor) onto
            // Commit.contributors (Vec<CommitContributor>) by id +
            // role. The primary agent is recorded with role
            // "primary" so reviewers can distinguish the agent
            // that drove the run from sidecar review/retrieval
            // agents that contributed evidence. The shape is
            // intentionally narrower than ContributorInfo —
            // version/instance/policy detail lives on the linked
            // Run rather than duplicated on every commit.
            let mut mapped: Vec<morph_core::CommitContributor> = Vec::new();
            if !run.agent.id.is_empty() {
                mapped.push(morph_core::CommitContributor {
                    id: run.agent.id.clone(),
                    role: Some("primary".to_string()),
                });
            }
            if let Some(run_contribs) = &run.contributors {
                for c in run_contribs {
                    mapped.push(morph_core::CommitContributor {
                        id: c.id.clone(),
                        role: c.role.clone(),
                    });
                }
            }
            if !mapped.is_empty() {
                commit_contributors = Some(mapped);
            }
        }
    }

    // v0.42.1: when a Run is attached, compute mixed-authorship
    // attribution against the staged tree before the rewrite. We
    // need the mirror commit's tree for `compute_human_edits` (it
    // gives us per-path blob hashes the trace can be compared
    // against) and the parent commit's tree for the
    // `no-trace-record` carve-out (a path that already existed at
    // the parent shouldn't be flagged as human-authored just
    // because the agent didn't touch it). Both lookups are
    // best-effort: a missing tree falls back to the empty case so
    // the rewrite still produces a valid commit.
    let human_edits: Option<Vec<morph_core::objects::HumanEdit>> = if let Some(rh) =
        evidence_run_hash
    {
        let mirrored_for_edits = match store.get(&mirrored_hash)? {
            MorphObject::Commit(c) => c,
            _ => {
                return Err(anyhow::anyhow!(
                    "mirrored commit {} is not a Commit object",
                    mirrored_hash
                ));
            }
        };
        let staged: std::collections::BTreeMap<String, String> = mirrored_for_edits
            .tree
            .as_deref()
            .and_then(|t| morph_core::Hash::from_hex(t).ok())
            .and_then(|h| morph_core::flatten_tree(store, &h).ok())
            .unwrap_or_default();
        let parent_tree: Option<std::collections::BTreeMap<String, String>> = mirrored_for_edits
            .parents
            .first()
            .and_then(|p| morph_core::Hash::from_hex(p).ok())
            .and_then(|h| match store.get(&h).ok()? {
                MorphObject::Commit(c) => c.tree,
                _ => None,
            })
            .and_then(|t| morph_core::Hash::from_hex(&t).ok())
            .and_then(|h| morph_core::flatten_tree(store, &h).ok());
        let edits = morph_core::compute_human_edits(store, &rh, &staged, parent_tree.as_ref())
            .unwrap_or_default();
        if edits.is_empty() {
            None
        } else {
            Some(edits)
        }
    } else {
        None
    };

    // v0.42.1: when a Run is attached, fold the human author into
    // `Commit.contributors` with role = `human-author`. Without
    // this the human who actually ran `morph commit` is recorded
    // only in `commit.author` and never appears in the structured
    // contributors list — so a downstream tool reading the
    // contributors list sees only the agent and concludes the
    // human had no hand in the change.
    if evidence_run_hash.is_some() {
        let author_for_attribution = resolved_author
            .clone()
            .or_else(|| author.map(|a| a.to_string()))
            .unwrap_or_else(|| "morph".to_string());
        commit_contributors = morph_core::fold_human_author_into_contributors(
            commit_contributors,
            &author_for_attribution,
        );
    }

    let mut new_morph_hash = mirrored_hash;
    let needs_rewrite = !observed_metrics.is_empty()
        || prog_hash.is_some()
        || suite_hash_str.is_some()
        || evidence_refs.is_some()
        || env_constraints.is_some()
        || commit_contributors.is_some()
        || human_edits.is_some();
    if needs_rewrite {
        let mirrored = match store.get(&mirrored_hash)? {
            MorphObject::Commit(c) => c,
            _ => {
                return Err(anyhow::anyhow!(
                    "mirrored commit {} is not a Commit object",
                    mirrored_hash
                ));
            }
        };
        let final_pipeline: String = prog_hash
            .as_ref()
            .map(|h: &Hash| h.to_string())
            .unwrap_or(mirrored.pipeline.clone());
        let final_suite = suite_hash_str.unwrap_or(mirrored.eval_contract.suite.clone());
        let mut new_commit = mirrored.clone();
        new_commit.pipeline = final_pipeline;
        new_commit.eval_contract = morph_core::objects::EvalContract {
            suite: final_suite,
            observed_metrics: observed_metrics.clone(),
        };
        if let Some(refs) = &evidence_refs {
            new_commit.evidence_refs = Some(refs.clone());
        }
        if let Some(ec) = &env_constraints {
            new_commit.env_constraints = Some(ec.clone());
        }
        if let Some(c) = &commit_contributors {
            new_commit.contributors = Some(c.clone());
        }
        if let Some(h) = &human_edits {
            new_commit.human_edits = Some(h.clone());
        }
        let rewritten = store.put(&MorphObject::Commit(new_commit))?;
        new_morph_hash = rewritten;
        let branch = morph_core::current_branch(store)
            .unwrap_or(None)
            .unwrap_or_else(|| "main".to_string());
        store.ref_write(&format!("heads/{}", branch), &rewritten)?;
    }

    if !observed_metrics.is_empty() {
        let cert = morph_core::certify_commit(
            store,
            morph_dir,
            &new_morph_hash,
            &observed_metrics,
            None,
            eval_suite,
        )?;
        if !cert.passed {
            eprintln!("warning: certification failed for {}:", new_morph_hash);
            for f in &cert.failures {
                eprintln!("  {}", f);
            }
        }
    }

    // v0.42: `--new-cases` selection.
    //   * `--new-cases "a,b"` → record exactly those ids (manual
    //     override).
    //   * `--new-cases ""` → explicit opt-out, skip auto-detect.
    //   * Flag absent → diff the about-to-commit suite against the
    //     first-parent's suite and auto-record the difference. The
    //     diff lets the merge gate attribute new acceptance cases
    //     to this commit without the user typing them out.
    let auto_cases: Vec<String> = match new_cases {
        Some(arg) => morph_core::parse_introduces_cases_arg(arg),
        None => {
            let parent_hash = match store.get(&new_morph_hash)? {
                morph_core::MorphObject::Commit(c) => c
                    .parents
                    .first()
                    .and_then(|p| morph_core::Hash::from_hex(p).ok()),
                _ => None,
            };
            let suite = match store.get(&new_morph_hash)? {
                morph_core::MorphObject::Commit(c) => c.eval_contract.suite.clone(),
                _ => String::new(),
            };
            morph_core::auto_detect_introduces_cases(store, parent_hash.as_ref(), &suite)
                .unwrap_or_default()
        }
    };
    let branch = morph_core::current_branch(store)?;
    if let Some(ann) =
        morph_core::build_introduces_cases_annotation(&new_morph_hash, &auto_cases, branch)
    {
        store.put(&ann)?;
    }

    // Clear the morph staging index after a successful reference-mode
    // commit. Symmetric with the standalone path which also clears
    // it; without this `morph status` would still report files as
    // staged after `morph commit` because `morph add` writes both
    // `.morph/index.json` and `git add`'s index.
    if let Err(e) = morph_core::clear_index(morph_dir) {
        eprintln!("warning: could not clear staging index after commit: {}", e);
    }

    if observed_metrics.is_empty() {
        eprintln!(
            "warning: commit has no observed_metrics. Morph cannot enforce \
             behavioral merge gating without evidence. Pass --metrics, \
             run `morph eval record` / `morph eval run`, or set a policy \
             via `morph policy init`."
        );
    }

    if json {
        let out = serde_json::json!({
            "hash": new_morph_hash.to_string(),
            "git_origin_sha": new_git_sha,
            "morph_origin": "cli",
            "message": message,
        });
        println!("{}", serde_json::to_string(&out)?);
    } else {
        println!("[{} (cli)] {}", new_morph_hash.short(), message);
        println!("  git: {}", hex_prefix(&new_git_sha, 12));
    }
    Ok(())
}

/// Structured version output for `morph version --json`. Stable
/// shape (additive only): release pipelines and downstream tooling
/// rely on the field names below to verify the binary's identity
/// without parsing the human-readable line.
fn version_json() -> String {
    let value = serde_json::json!({
        "name": "morph",
        "version": env!("CARGO_PKG_VERSION"),
        "build_date": env!("MORPH_BUILD_DATE"),
        "protocol_version": morph_core::ssh_proto::MORPH_PROTOCOL_VERSION,
        "supported_repo_versions": SUPPORTED_REPO_VERSIONS,
    });
    // Serializing a serde_json::Value built from string/number/array
    // literals is infallible — there are no Serialize-impl errors to
    // surface. Round-tripping is exercised by `version_json_has_stable_field_set`.
    serde_json::to_string(&value).expect("infallible: serializing serde_json::Value")
}

/// PR 9: reference-mode `morph merge` wrapper. Symmetric with PR 5's
/// `run_reference_commit` — `morph merge X` becomes the canonical
/// way to merge in reference mode, driving `git merge` first (with
/// `MORPH_INTERNAL=1` so morph hooks short-circuit) and then
/// mirroring the resulting git HEAD into morph with
/// `morph_origin = "cli"`. Plain `git merge` keeps working for
/// teammates via the post-merge hook (PR 6).
///
/// PR 9 covers the clean-merge cases: divergent merge with no
/// conflicts, fast-forward, already-up-to-date, and gate-rejection
/// (the gate runs *before* git merge so a doomed merge never
/// produces a stranded git commit). Conflict resolution is wired
/// in PR 11.
#[allow(clippy::too_many_arguments)]
fn run_reference_merge(
    store: &dyn morph_core::Store,
    morph_dir: &std::path::Path,
    repo_root: &std::path::Path,
    branch: &str,
    pipeline: Option<String>,
    eval_suite: Option<String>,
    metrics: Option<String>,
    message: Option<String>,
    author: Option<String>,
    retire: Option<String>,
    retire_reason: Option<String>,
) -> anyhow::Result<()> {
    let version = read_repo_version(morph_dir)?;
    let bare_branch = branch.strip_prefix("heads/").unwrap_or(branch);

    ensure_reference_synced_for_merge(store, morph_dir, repo_root, branch)?;

    let observed: Option<std::collections::BTreeMap<String, f64>> = metrics
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?;

    let suite_hash_opt = eval_suite
        .as_deref()
        .map(|s| resolve_obj_hash(store, s))
        .transpose()?;
    // Resolve the user-supplied --pipeline argument (a hash or a
    // short prefix) once so both the rebuild and the breadcrumb
    // record the canonical 64-char hex.
    let user_pipeline_hash: Option<String> = pipeline
        .as_deref()
        .map(|s| resolve_obj_hash(store, s))
        .transpose()?
        .map(|h| h.to_string());
    let retired: Option<Vec<String>> = retire
        .as_deref()
        .map(|s| s.split(',').map(|m| m.trim().to_string()).collect());
    let mut plan = morph_core::prepare_merge(
        store,
        bare_branch,
        suite_hash_opt.as_ref(),
        retired.as_deref(),
    )?;
    plan.retire_reason = retire_reason.clone();
    warn_when_no_morph_claim(store, &plan);

    // Author drives the synthesised review-node attribution (when
    // `--retire` is in play). Resolve through morph's identity
    // chain so review nodes carry the same identity as commit
    // authors. Fall back to "morph-cli" only when absolutely no
    // identity can be established — review-node attribution is
    // user-visible and a meaningful identity is always preferred.
    let resolved_author = morph_core::resolve_author_for_repo(morph_dir, author.as_deref())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "morph-cli".to_string());

    if let Some(ref obs) = observed {
        let dominance = plan.check_dominance(obs);
        if !dominance.passed {
            let mut msg =
                String::from("merge rejected: merged metrics do not dominate both parents\n");
            for v in &dominance.violations {
                msg.push_str(&format!("  {}\n", v));
            }
            anyhow::bail!(msg.trim().to_string());
        }
    }

    let merge_message = message
        .clone()
        .unwrap_or_else(|| format!("Merge branch '{}'", bare_branch));
    // When the user provided -m, they want a real merge commit
    // (otherwise the message would have nowhere to go). Without -m,
    // let git fast-forward when it can — that mirrors `git merge`'s
    // own UX exactly.
    let no_ff = message.is_some();

    let outcome = morph_core::run_git_merge_with_morph_internal(
        repo_root,
        bare_branch,
        &merge_message,
        no_ff,
    )?;

    match outcome {
        morph_core::GitMergeOutcome::AlreadyUpToDate => {
            println!("Already up to date.");
            // Be defensive: make sure morph branch ref matches git's
            // current HEAD even if it already did. No new commit.
            morph_core::sync_to_head_with_origin(store, repo_root, "cli", Some(&version))?;
        }
        morph_core::GitMergeOutcome::FastForward { new_head } => {
            morph_core::sync_to_head_with_origin(store, repo_root, "cli", Some(&version))?;
            println!("Fast-forwarded to {}.", hex_prefix(&new_head, 12));
        }
        morph_core::GitMergeOutcome::Merged { new_head } => {
            let sync_outcome =
                morph_core::sync_to_head_with_origin(store, repo_root, "cli", Some(&version))?;
            let mirrored = sync_outcome.new_commit.ok_or_else(|| {
                anyhow::anyhow!(
                    "git merge {} did not produce a new morph commit",
                    hex_prefix(&new_head, 12)
                )
            })?;
            // v0.42: rebuild the mirror commit so it carries the
            // user's (or auto-union'd) eval suite, pipeline, and
            // retired-metrics decision instead of `sync_one_commit`'s
            // empty placeholders. Without this, the merge gate has
            // nothing to enforce and metric-retirement / review-node
            // attribution silently disappear.
            let opts = morph_core::MergeRebuildOpts {
                user_pipeline: user_pipeline_hash.clone(),
                user_eval_suite: suite_hash_opt.as_ref().map(|h| h.to_string()),
                user_metrics: observed.clone().unwrap_or_default(),
                retired_metrics: retired.clone().unwrap_or_default(),
                retire_reason: plan.retire_reason.clone(),
                author: resolved_author.clone(),
                morph_instance: morph_core::read_instance_id(morph_dir).ok().flatten(),
            };
            let new_morph = morph_core::rebuild_merge_commit(store, &mirrored, &plan, &opts)?;
            if let Some(obs) = observed {
                let cert =
                    morph_core::certify_commit(store, morph_dir, &new_morph, &obs, None, None)?;
                if !cert.passed {
                    eprintln!("warning: certification failed for {}:", new_morph);
                    for f in &cert.failures {
                        eprintln!("  {}", f);
                    }
                }
            }
            println!("{}", new_morph);
        }
        morph_core::GitMergeOutcome::Conflicts { paths } => {
            // PR 11: write the breadcrumb so `morph merge --continue`
            // / `--abort` know what to commit / abort. We need both
            // parents' git SHAs and the synthesised merge message.
            // `git rev-parse HEAD` still points at the OLD head here
            // (the unfinished merge hasn't created a commit yet).
            let head_git_sha = morph_core::git_head_sha(repo_root)?
                .ok_or_else(|| anyhow::anyhow!("git HEAD missing during conflict path"))?;
            let other_git_sha = morph_core::lookup_branch_git_sha(repo_root, bare_branch)?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "could not resolve git SHA for branch '{}' after conflict",
                        bare_branch
                    )
                })?;
            let breadcrumb = morph_core::ReferenceMergeBreadcrumb {
                other_branch: bare_branch.to_string(),
                other_git_sha,
                head_git_sha,
                message: merge_message.clone(),
                pipeline: user_pipeline_hash.clone(),
                eval_suite: suite_hash_opt.as_ref().map(|h| h.to_string()),
                retired_metrics: retired.clone().unwrap_or_default(),
                retire_reason: plan.retire_reason.clone(),
            };
            morph_core::write_merge_breadcrumb(morph_dir, &breadcrumb)?;
            eprintln!(
                "Auto-merging failed for {} path{}; conflict markers written to disk.",
                paths.len(),
                if paths.len() == 1 { "" } else { "s" }
            );
            for p in &paths {
                eprintln!("  conflict: {}", p);
            }
            eprintln!(
                "Resolve the conflicts (edit + `git add` each path), then run \
                 `morph merge --continue` or `morph merge --abort`."
            );
            anyhow::bail!("merge produced conflicts");
        }
    }

    Ok(())
}

/// PR 11: reference-mode `morph merge --continue`. Reads the
/// breadcrumb left by [`run_reference_merge`]'s Conflicts arm,
/// verifies the user resolved every unmerged path, finalizes the
/// git merge under `MORPH_INTERNAL=1` (so the post-merge hook
/// short-circuits), mirrors into morph with `morph_origin = "cli"`,
/// optionally attaches certification metrics, and clears the
/// breadcrumb.
fn run_reference_merge_continue(
    store: &dyn morph_core::Store,
    morph_dir: &std::path::Path,
    repo_root: &std::path::Path,
    message_override: Option<String>,
    metrics: Option<String>,
    author: Option<String>,
) -> anyhow::Result<()> {
    let breadcrumb = morph_core::read_merge_breadcrumb(morph_dir)?.ok_or_else(|| {
        anyhow::anyhow!(
            "no merge in progress (.morph/MERGE_REF.json missing). \
             Did you mean to start a new merge with `morph merge <branch>`?"
        )
    })?;

    // Refuse to commit until git's index is clean — every unmerged
    // path must be resolved AND staged.
    let unmerged = morph_core::list_unmerged_paths(repo_root)?;
    if !unmerged.is_empty() {
        eprintln!(
            "Cannot continue: {} path{} still unmerged.",
            unmerged.len(),
            if unmerged.len() == 1 { "" } else { "s" }
        );
        for p in &unmerged {
            eprintln!("  unmerged: {}", p);
        }
        anyhow::bail!(
            "resolve conflicts and `git add` each path, then re-run `morph merge --continue`"
        );
    }

    let version = read_repo_version(morph_dir)?;
    let message = message_override.unwrap_or_else(|| breadcrumb.message.clone());

    // v0.42: rebuild the merge plan up-front. Calling `prepare_merge`
    // here (with the breadcrumb's recorded suite and retired
    // metrics) reproduces the exact union suite + reference bar the
    // user committed to when they started the merge, so
    // `--continue`'s rewrite uses the same shape as the single-shot
    // path. The plan must be computed BEFORE the git commit lands —
    // afterwards HEAD has advanced and `prepare_merge` would see
    // the merged commit as its own ancestor.
    let suite_override_hash = match breadcrumb.eval_suite.as_deref() {
        Some(s) => Some(resolve_obj_hash(store, s)?),
        None => None,
    };
    let mut plan = morph_core::prepare_merge(
        store,
        &breadcrumb.other_branch,
        suite_override_hash.as_ref(),
        if breadcrumb.retired_metrics.is_empty() {
            None
        } else {
            Some(&breadcrumb.retired_metrics[..])
        },
    )?;
    plan.retire_reason = breadcrumb.retire_reason.clone();

    // Drive `git commit -m <msg>` under MORPH_INTERNAL=1. Git will
    // create the merge commit using `.git/MERGE_HEAD` (still in
    // place since the original `git merge` left it), reusing the
    // already-staged conflict resolutions.
    let new_git_sha =
        morph_core::run_git_commit_with_morph_internal(repo_root, &message, false, None)
            .map_err(|e| anyhow::anyhow!("{}", e))?;

    // Mirror the new merge commit into morph with origin = "cli".
    let outcome = morph_core::sync_to_head_with_origin(store, repo_root, "cli", Some(&version))?;
    let mirrored = outcome.new_commit.ok_or_else(|| {
        anyhow::anyhow!(
            "git merge {} did not produce a new morph commit",
            hex_prefix(&new_git_sha, 12)
        )
    })?;

    // Apply the same v0.42 rebuild the single-shot path uses so
    // `--continue` finalizes a fully-fledged merge commit (suite,
    // pipeline-with-review-node, observed_metrics, evidence_refs).
    let observed_for_rebuild: std::collections::BTreeMap<String, f64> = match &metrics {
        Some(m) => {
            serde_json::from_str(m).map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?
        }
        None => std::collections::BTreeMap::new(),
    };
    let resolved_author = morph_core::resolve_author_for_repo(morph_dir, author.as_deref())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "morph-cli".to_string());
    let opts = morph_core::MergeRebuildOpts {
        user_pipeline: breadcrumb.pipeline.clone(),
        user_eval_suite: breadcrumb.eval_suite.clone(),
        user_metrics: observed_for_rebuild.clone(),
        retired_metrics: breadcrumb.retired_metrics.clone(),
        retire_reason: breadcrumb.retire_reason.clone(),
        author: resolved_author,
        morph_instance: morph_core::read_instance_id(morph_dir).ok().flatten(),
    };
    let new_morph = morph_core::rebuild_merge_commit(store, &mirrored, &plan, &opts)?;

    // Optional certification — same code path as the single-shot
    // merge in `run_reference_merge`.
    if let Some(metrics_str) = metrics {
        let observed: std::collections::BTreeMap<String, f64> = serde_json::from_str(&metrics_str)
            .map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?;
        if !observed.is_empty() {
            let cert =
                morph_core::certify_commit(store, morph_dir, &new_morph, &observed, None, None)?;
            if !cert.passed {
                eprintln!("warning: certification failed for {}:", new_morph);
                for f in &cert.failures {
                    eprintln!("  {}", f);
                }
            }
        }
    }

    morph_core::clear_merge_breadcrumb(morph_dir)?;
    println!("{}", new_morph);
    Ok(())
}

/// PR 11: reference-mode `morph merge --abort`. Best-effort: tolerate
/// a missing breadcrumb (user manually ran `git merge --abort`) and a
/// missing `.git/MERGE_HEAD` (already-aborted merge). Always clears
/// the breadcrumb so a subsequent `morph merge` starts clean.
fn run_reference_merge_abort(
    morph_dir: &std::path::Path,
    repo_root: &std::path::Path,
) -> anyhow::Result<()> {
    let breadcrumb = morph_core::read_merge_breadcrumb(morph_dir)?;
    let aborted = morph_core::run_git_merge_abort_with_morph_internal(repo_root)
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    morph_core::clear_merge_breadcrumb(morph_dir)?;
    match (breadcrumb.is_some(), aborted) {
        (true, true) => println!("Merge aborted; working tree restored."),
        (true, false) => println!("Merge breadcrumb cleared; git was already in a clean state."),
        (false, true) => println!("git merge --abort completed; no morph breadcrumb to clear."),
        // Match `git merge --abort` UX: refusing to abort when no
        // merge is in progress is an error, not a no-op. Lets
        // scripts notice the mistake instead of silently moving on.
        (false, false) => {
            anyhow::bail!("no merge in progress");
        }
    }
    Ok(())
}

/// PR 7: Stowaway-mode pre-flight for `morph merge`. In reference
/// mode, mirror any unmirrored git history for both the current
/// branch (in case the user committed via plain `git commit` with
/// `MORPH_INTERNAL=1` or hooks were uninstalled) and the merge
/// target (a teammate's branch that may exist only in git). Also
/// surface a warning when either side has no morph evidence so the
/// user knows the gate has nothing to enforce on that side. Bails
/// out cheaply (`Ok(())`) for non-reference repos and for git tips
/// already mirrored.
fn ensure_reference_synced_for_merge(
    store: &dyn morph_core::Store,
    morph_dir: &std::path::Path,
    repo_root: &std::path::Path,
    branch: &str,
) -> anyhow::Result<()> {
    // Reference mode is the only mode (v0.40+); skip the auto-mirror
    // only for the rare unit-test case where the repo is missing its
    // sibling `.git/` (`init_repo` used as a tempdir fixture).
    if !morph_core::is_git_working_tree(repo_root) {
        return Ok(());
    }
    let version = read_repo_version(morph_dir)?;

    // Mirror current branch (HEAD) so the merge gate sees the user's
    // latest git commits even if they bypassed the post-commit hook.
    let head = morph_core::sync_to_head(store, repo_root, Some(&version))?;
    if let Some(_new) = head.new_commit {
        let sha = head
            .git_sha
            .as_deref()
            .map(|s| &s[..s.len().min(7)])
            .unwrap_or("?");
        eprintln!(
            "morph: auto-mirroring current branch — git HEAD ({}) was ahead of morph",
            sha
        );
    }

    // Mirror the merge target. Strip a leading `heads/` so callers
    // can pass either form (matches `prepare_merge`'s rules).
    let bare = branch.strip_prefix("heads/").unwrap_or(branch);
    let other = morph_core::ensure_branch_synced(store, repo_root, bare, Some(&version))?;
    if other.missing_in_git() {
        // No git branch by that name either. Fall through and let
        // `prepare_merge` / `start_merge` produce the canonical
        // "branch not found" error.
        return Ok(());
    }
    if other.created > 0 {
        let tip = other
            .git_tip
            .as_deref()
            .map(|s| &s[..s.len().min(7)])
            .unwrap_or("?");
        eprintln!(
            "morph: auto-mirroring '{}' from git into morph ({} new commit{}, tip {})",
            bare,
            other.created,
            if other.created == 1 { "" } else { "s" },
            tip
        );
    } else if other.branch_moved {
        eprintln!(
            "morph: pointing morph branch '{}' at the existing mirror",
            bare
        );
    }

    Ok(())
}

/// PR 7: emit a "no morph claim from <side>" warning when a parent
/// commit has no observed metrics and no certification annotations.
/// In reference / Stowaway mode this is the common case (a teammate's
/// branch never ran `morph eval run`), and we want the user to know
/// the merge gate has nothing to enforce on that side rather than
/// quietly blessing an evidence-free commit.
fn warn_when_no_morph_claim(store: &dyn morph_core::Store, plan: &morph_core::MergePlan) {
    if plan.head_metrics.is_empty() {
        let label = plan.head_branch.as_deref().unwrap_or("HEAD");
        eprintln!(
            "morph: no morph evidence on '{}' — merge proceeds without behavioral assertion from this side",
            label
        );
    }
    if plan.other_metrics.is_empty() {
        eprintln!(
            "morph: no morph evidence on '{}' — merge proceeds without behavioral assertion from this side",
            plan.other_branch
        );
    }
    let _ = store;
}

/// Dispatch for `morph merge`. Maps the flag combo onto the four
/// merge-flow entry points: `start_merge` (positional `<branch>`),
/// `continue_merge` (`--continue`), `abort_merge` (`--abort`), and
/// `resolve_node` (`resolve-node` subcommand). When the user passes
/// a branch *and* the legacy single-shot flags (`--pipeline`,
/// `--metrics`, `-m`), a clean three-way merge auto-finalizes via
/// `execute_merge` for backwards compatibility with PR≤3 scripts.
#[allow(clippy::too_many_arguments)]
fn run_merge(
    verbose: bool,
    branch: Option<String>,
    cont: bool,
    abort: bool,
    message: Option<String>,
    pipeline: Option<String>,
    eval_suite: Option<String>,
    metrics: Option<String>,
    author: Option<String>,
    retire: Option<String>,
    retire_reason: Option<String>,
    sub: Option<MergeCmd>,
) -> anyhow::Result<()> {
    let (repo_root, store) = get_store(verbose)?;
    let morph_dir = repo_root.join(".morph");

    if let Some(MergeCmd::ResolveNode { node, pick }) = sub {
        morph_core::resolve_node(store.as_ref(), &repo_root, &node, &pick)?;
        println!("Resolved pipeline node `{}` -> {}", node, pick);
        return Ok(());
    }

    // Reference mode is the only mode (v0.40+); `--abort` and
    // `--continue` route to the reference-mode handlers (which drive
    // `git merge --abort` / `git commit -m <msg>`). The plain
    // `morph_core::abort_merge` path is only reached in unit-test
    // tempdirs that lack a `.git/` sibling.
    let is_reference = morph_core::is_git_working_tree(&repo_root);

    if abort {
        if is_reference {
            return run_reference_merge_abort(&morph_dir, &repo_root);
        }
        morph_core::abort_merge(store.as_ref(), &repo_root)?;
        println!("Merge aborted; working tree restored to ORIG_HEAD.");
        return Ok(());
    }

    if cont {
        if is_reference {
            return run_reference_merge_continue(
                store.as_ref(),
                &morph_dir,
                &repo_root,
                message,
                metrics,
                author,
            );
        }
        let cont_outcome = morph_core::continue_merge(
            store.as_ref(),
            &repo_root,
            morph_core::ContinueMergeOpts { message, author },
        )?;
        println!("{}", cont_outcome.merge_commit);
        return Ok(());
    }

    let branch = branch.ok_or_else(|| {
        anyhow::anyhow!(
            "missing branch argument (use `morph merge <branch>` to start, \
             `--continue` / `--abort` to manage an in-progress merge, \
             or `morph merge resolve-node <id> --pick ours|theirs|base`)"
        )
    })?;

    // PR 9: in reference mode, `morph merge X` becomes the canonical
    // merge driver. It calls `git merge` with MORPH_INTERNAL=1 and
    // mirrors after the fact, exactly mirroring PR 5's `morph commit`
    // wrapper. Standalone repos keep the structural-merge code path
    // below unchanged.
    if is_reference {
        return run_reference_merge(
            store.as_ref(),
            &morph_dir,
            &repo_root,
            &branch,
            pipeline,
            eval_suite,
            metrics,
            message,
            author,
            retire,
            retire_reason,
        );
    }

    ensure_reference_synced_for_merge(store.as_ref(), &morph_dir, &repo_root, &branch)?;

    // Single-shot path: caller supplied pipeline + metrics + message
    // up front, so we skip the stateful start/continue dance. Bind
    // them as a tuple so any future addition to the trio is caught
    // by exhaustiveness instead of three coupled `unwrap`s.
    if let (Some(pipeline_ref), Some(metrics_json), Some(commit_message)) =
        (pipeline.as_deref(), metrics.as_deref(), message.as_ref())
    {
        let version = read_repo_version(&morph_dir)?;
        let prog_hash = resolve_obj_hash(store.as_ref(), pipeline_ref)?;
        let suite_hash_opt = eval_suite
            .as_deref()
            .map(|s| resolve_obj_hash(store.as_ref(), s))
            .transpose()?;
        let observed: std::collections::BTreeMap<String, f64> = serde_json::from_str(metrics_json)
            .map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?;
        let retired: Option<Vec<String>> =
            retire.map(|s| s.split(',').map(|m| m.trim().to_string()).collect());
        let mut plan = morph_core::prepare_merge(
            store.as_ref(),
            &branch,
            suite_hash_opt.as_ref(),
            retired.as_deref(),
        )?;
        warn_when_no_morph_claim(store.as_ref(), &plan);
        plan.retire_reason = retire_reason;
        let resolved_author = morph_core::resolve_author_for_repo(&morph_dir, author.as_deref())?;
        let hash = morph_core::execute_merge(
            store.as_ref(),
            &plan,
            &prog_hash,
            observed,
            commit_message.clone(),
            Some(resolved_author),
            Some(&repo_root),
            Some(&version),
        )?;
        println!("{}", hash);
        return Ok(());
    }

    // Stateful flow: kick off the structural merge, then either
    // surface the conflicts (so the user can resolve and run
    // `--continue`) or auto-finalize a clean three-way merge.
    let retired_metrics: Vec<String> = retire
        .as_deref()
        .map(|s| s.split(',').map(|m| m.trim().to_string()).collect())
        .unwrap_or_default();
    let mut start_opts = morph_core::StartMergeOpts::new(&branch);
    start_opts.retired_metrics = &retired_metrics;
    start_opts.retire_reason = retire_reason.as_deref();
    let outcome = morph_core::start_merge(store.as_ref(), &repo_root, start_opts)?;

    if outcome.needs_resolution {
        if !outcome.textual_conflicts.is_empty() {
            println!(
                "Auto-merging failed for {} path{}; conflict markers written to disk.",
                outcome.textual_conflicts.len(),
                if outcome.textual_conflicts.len() == 1 {
                    ""
                } else {
                    "s"
                }
            );
            for p in &outcome.textual_conflicts {
                println!("  CONFLICT (content): {}", p);
            }
        }
        if !outcome.pipeline_node_conflicts.is_empty() {
            println!(
                "Pipeline has {} node-level conflict{}:",
                outcome.pipeline_node_conflicts.len(),
                if outcome.pipeline_node_conflicts.len() == 1 {
                    ""
                } else {
                    "s"
                }
            );
            for c in &outcome.pipeline_node_conflicts {
                println!("  CONFLICT (pipeline node): {}", c.id);
            }
            println!("  resolve with: morph merge resolve-node <id> --pick ours|theirs|base");
        }
        println!("Run `morph status` for details, then `morph merge --continue`.");
        std::process::exit(1);
    }

    if matches!(
        outcome.trivial,
        morph_core::TrivialOutcome::AlreadyMerged | morph_core::TrivialOutcome::AlreadyAhead
    ) {
        println!("Already up to date.");
        return Ok(());
    }

    if matches!(outcome.trivial, morph_core::TrivialOutcome::FastForward) {
        let branch_ref =
            morph_core::current_branch(store.as_ref())?.unwrap_or_else(|| "main".to_string());
        store.ref_write(&format!("heads/{}", branch_ref), &outcome.other)?;
        morph_core::checkout_tree(store.as_ref(), &repo_root, &branch_ref)?;
        println!("Fast-forwarded {} to {}.", branch_ref, outcome.other);
        return Ok(());
    }

    let cont_outcome = morph_core::continue_merge(
        store.as_ref(),
        &repo_root,
        morph_core::ContinueMergeOpts {
            message: message.or_else(|| Some(format!("Merge branch '{}'", branch))),
            author,
        },
    )?;
    println!("{}", cont_outcome.merge_commit);
    Ok(())
}

/// Backwards-compat alias retained for call sites that historically
/// only accepted ref-style identifiers (e.g. `morph diff`, `morph log`,
/// `morph checkout`). Resolution rules are unified in
/// [`resolve_obj_hash`] / [`morph_core::resolve_revision`].
fn resolve_ref_name(store: &dyn Store, r: &str) -> anyhow::Result<Hash> {
    resolve_obj_hash(store, r)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let verbose = cli.verbose;

    match cli.command {
        Command::Version { json } => {
            if json {
                println!("{}", version_json());
            } else {
                println!(
                    "morph {} (built {})",
                    env!("CARGO_PKG_VERSION"),
                    env!("MORPH_BUILD_DATE"),
                );
            }
        }

        Command::Init {
            path,
            bare,
            no_default_policy,
            solo,
            git_init,
            no_git_init,
        } => {
            verbose_msg(
                verbose,
                &format!(
                    "initializing {} repo at {}",
                    if bare { "bare" } else { "reference-mode" },
                    path.display()
                ),
            );
            if bare {
                morph_core::init_bare(&path)?;
                let abs = path
                    .canonicalize()
                    .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&path));
                println!("Initialized bare Morph repository in {}/", abs.display());
                // Surface the active policy so the operator knows the
                // gate they're under without `morph policy show`.
                // Bare repos keep the opinionated default from
                // `init_morph_dir_at` (tests_total + tests_passed
                // required); print the loosening recipe alongside.
                // Bare layout puts `config.json` directly at the repo
                // root (no `.morph/` wrapper).
                let bare_policy = morph_core::read_policy(&abs).unwrap_or_default();
                print_policy_summary(&bare_policy);
                return Ok(());
            }

            // Reference-mode (the only working-tree mode in v0.40+).
            // morph requires a git repository alongside; if `path`
            // isn't already a git working tree, ask before running
            // `git init`. `--git-init` and `--no-git-init` skip the
            // prompt for scripting / CI.
            if !morph_core::is_git_working_tree(&path) {
                let should_git_init = if git_init {
                    true
                } else if no_git_init {
                    false
                } else if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                    use std::io::Write;
                    eprint!("morph requires a git repository here. Run `git init` for you? [y/N] ");
                    let _ = std::io::stderr().flush();
                    let mut answer = String::new();
                    if std::io::stdin().read_line(&mut answer).is_err() {
                        false
                    } else {
                        let trimmed = answer.trim().to_lowercase();
                        trimmed == "y" || trimmed == "yes"
                    }
                } else {
                    false
                };
                if should_git_init {
                    // `-b main` pins the initial branch to `main`
                    // regardless of the host's `init.defaultBranch`
                    // setting. Without this, a fresh GitHub-Actions
                    // runner (or any host that hasn't opted in to
                    // git 2.28's default-branch rename) lands on
                    // `master`, and downstream `morph checkout main`
                    // / `morph branch feature` then fail with
                    // `pathspec 'main' did not match` because git's
                    // `main` ref doesn't exist. Morph's user-facing
                    // contract is `main` (DEFAULT_BRANCH in
                    // morph-core); aligning git on init avoids the
                    // ref-mode mismatch from the start.
                    let status = std::process::Command::new("git")
                        .arg("init")
                        .arg("-b")
                        .arg(morph_core::DEFAULT_BRANCH)
                        .arg(&path)
                        .status()
                        .map_err(|e| anyhow::anyhow!("failed to spawn `git init`: {}", e))?;
                    if !status.success() {
                        anyhow::bail!(
                            "`git init -b {} {}` failed (exit {}); fix the underlying error and re-run",
                            morph_core::DEFAULT_BRANCH,
                            path.display(),
                            status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                        );
                    }
                } else {
                    eprintln!(
                        "morph init: {} is not a git repository (no .git/ found). \
                         Run `git init` first, or pass `--git-init` to morph init to do it for you.",
                        path.display()
                    );
                    std::process::exit(1);
                }
            }

            morph_core::init_repo(&path)?;
            let init_at = morph_core::git_head_sha(&path)?;
            let morph_dir = path.join(".morph");
            if let Some(sha) = init_at.as_deref() {
                morph_core::write_init_at_git_sha(&morph_dir, sha)?;
            }
            let submode = if solo {
                morph_core::RepoSubmode::Solo
            } else {
                morph_core::RepoSubmode::Stowaway
            };
            morph_core::write_repo_submode(&morph_dir, submode)?;
            // Reference-mode default policy: empty `required_metrics`
            // plus a carve-out for git-hook commits in `gate_check`.
            // Every git commit produces a morph commit before the user
            // has had a chance to certify it; without this carve-out
            // the manual gate would always fail. The merge gate is
            // unaffected — it still requires evidence on each parent.
            // Written even when `--no-default-policy` is set, because
            // reference-mode correctness *depends* on the carve-out.
            let policy = morph_core::RepoPolicy {
                exempt_origins: vec!["git-hook".to_string()],
                ..Default::default()
            };
            morph_core::write_policy(&morph_dir, &policy)?;
            let hook_report = morph_core::install_reference_hooks(&path, submode)?;
            let exclude_added = morph_core::ensure_morph_in_git_info_exclude(&path)?;
            let abs_morph = path
                .canonicalize()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&path))
                .join(".morph");
            println!(
                "Initialized empty Morph repository in {}/",
                abs_morph.display()
            );
            if let Some(sha) = init_at.as_deref() {
                println!("  bound to git HEAD {}", hex_prefix(sha, 12));
            } else {
                println!("  bound to empty git repository (no commits yet)");
            }
            let total = hook_report.installed.len() + hook_report.already_present.len();
            println!(
                "  installed {} git hook(s) at .git/hooks/ ({} present, {} already up to date)",
                total,
                hook_report.installed.len(),
                hook_report.already_present.len()
            );
            if exclude_added {
                println!(
                    "  morph state is local to this clone (.morph/ added to .git/info/exclude)"
                );
            } else {
                println!(
                    "  morph state is local to this clone (.git/info/exclude already excludes .morph/)"
                );
            }
            // Surface the active policy so a fresh user knows the gate
            // they're under without having to run `morph policy show`.
            // Reference-mode init writes a permissive policy with a
            // git-hook carve-out; print "relaxed" plus the tightening
            // recipe (`morph policy init`).
            print_policy_summary(&policy);
            match submode {
                morph_core::RepoSubmode::Stowaway => {
                    println!(
                        "  teammates not using morph are unaffected — your git workflow is unchanged"
                    );
                }
                morph_core::RepoSubmode::Solo => {
                    println!(
                        "  Solo submode: pre-merge-commit hook is active — plain `git merge` is gated"
                    );
                    println!(
                        "  bypass the gate one-off with MORPH_NO_GATE=1; flip back with `morph install-hooks --stowaway`"
                    );
                }
            }
            // The opinionated default policy (with `git-hook` carve-
            // out) is correctness-load-bearing for reference mode, so
            // even `--no-default-policy` keeps it. The flag still has
            // an effect on bare repos (handled above) and is kept for
            // legacy spec fixtures.
            let _ = no_default_policy;
        }

        Command::ReferenceSync { backfill } => {
            let cwd = std::env::current_dir()?;
            let repo_root = morph_core::find_repo(&cwd)
                .ok_or_else(|| anyhow::anyhow!("not in a morph repository"))?;
            let morph_dir = repo_root.join(".morph");
            if !morph_core::is_git_working_tree(&repo_root) {
                eprintln!(
                    "morph reference-sync: {} is not a git working tree (no .git/ found). \
                     Reference-mode sync needs a git repository alongside the morph one.",
                    repo_root.display()
                );
                std::process::exit(1);
            }
            let store = morph_core::open_store(&morph_dir)?;
            let version = Some(env!("CARGO_PKG_VERSION"));
            if backfill {
                let init_sha = morph_core::read_init_at_git_sha(&morph_dir)?;
                let count = morph_core::backfill_from_init(
                    store.as_ref(),
                    &repo_root,
                    init_sha.as_deref(),
                    version,
                )?;
                if count == 0 {
                    println!("Already up to date.");
                } else {
                    let plural = if count == 1 { "commit" } else { "commits" };
                    println!("Synced {} git {}.", count, plural);
                }
            } else {
                // Plain `morph reference-sync`: walks back to the
                // last-mirrored ancestor of git HEAD and mirrors every
                // commit in the unmirrored span. `outcome.created`
                // reports the actual count, which may exceed 1 when
                // the user has been running `MORPH_INTERNAL=1 git
                // commit` or pulled multi-commit FFs without hooks.
                let outcome = morph_core::sync_to_head(store.as_ref(), &repo_root, version)?;
                if outcome.already_synced {
                    println!("Already up to date.");
                } else {
                    let short = outcome
                        .git_sha
                        .as_deref()
                        .map(|s| &s[..s.len().min(8)])
                        .unwrap_or("?");
                    let plural = if outcome.created == 1 {
                        "commit"
                    } else {
                        "commits"
                    };
                    println!(
                        "Synced {} git {} (HEAD {}).",
                        outcome.created, plural, short
                    );
                }
            }
        }

        Command::InstallHooks { solo, stowaway } => {
            let cwd = std::env::current_dir()?;
            let repo_root = morph_core::find_repo(&cwd)
                .ok_or_else(|| anyhow::anyhow!("not in a morph repository"))?;
            let morph_dir = repo_root.join(".morph");
            if !morph_core::is_git_working_tree(&repo_root) {
                eprintln!(
                    "morph install-hooks: {} is not a git working tree (no .git/ found). \
                     Hooks live in `.git/hooks/`; this morph repo has no git to install into.",
                    repo_root.display()
                );
                std::process::exit(1);
            }
            // PR 10: `--solo` and `--stowaway` flip the submode and
            // install/remove the pre-merge-commit gate accordingly.
            // Without either flag, keep whatever submode is already
            // recorded — reinstall is a pure idempotent rewrite.
            let submode = if solo {
                morph_core::write_repo_submode(&morph_dir, morph_core::RepoSubmode::Solo)?;
                morph_core::RepoSubmode::Solo
            } else if stowaway {
                morph_core::write_repo_submode(&morph_dir, morph_core::RepoSubmode::Stowaway)?;
                morph_core::RepoSubmode::Stowaway
            } else {
                morph_core::read_repo_submode(&morph_dir)?
            };
            let report = morph_core::install_reference_hooks(&repo_root, submode)?;
            let exclude_added = morph_core::ensure_morph_in_git_info_exclude(&repo_root)?;
            if report.changed() {
                if !report.installed.is_empty() {
                    println!(
                        "Installed {} hook(s) at .git/hooks/: {}.",
                        report.installed.len(),
                        report.installed.join(", ")
                    );
                }
                if !report.removed.is_empty() {
                    println!(
                        "Removed {} hook(s) (submode downgrade): {}.",
                        report.removed.len(),
                        report.removed.join(", ")
                    );
                }
                if !report.already_present.is_empty() {
                    println!(
                        "  ({} already up to date: {})",
                        report.already_present.len(),
                        report.already_present.join(", ")
                    );
                }
            } else {
                println!(
                    "All {} reference-mode hooks already installed (no changes).",
                    report.already_present.len()
                );
            }
            match submode {
                morph_core::RepoSubmode::Stowaway => println!("  submode: stowaway"),
                morph_core::RepoSubmode::Solo => {
                    println!("  submode: solo — `git merge` is gated by pre-merge-commit hook");
                    println!(
                        "  bypass once with MORPH_NO_GATE=1; flip back with `morph install-hooks --stowaway`"
                    );
                }
            }
            if exclude_added {
                println!("Added .morph/ to .git/info/exclude (local to this clone).");
            }
        }

        Command::Hook { event, args } => {
            run_hook(&event, &args)?;
        }

        Command::Clone {
            url,
            destination,
            branch,
            bare,
        } => {
            let dest =
                destination.unwrap_or_else(|| std::path::PathBuf::from(default_clone_dest(&url)));
            verbose_msg(
                verbose,
                &format!(
                    "cloning {} -> {} ({} clone)",
                    url,
                    dest.display(),
                    if bare { "bare" } else { "working" }
                ),
            );
            let outcome =
                morph_core::clone_repo(&url, &dest, morph_core::CloneOpts { branch, bare })?;
            let abs = dest
                .canonicalize()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&dest));
            println!(
                "Cloned {} into {}",
                url,
                if bare {
                    format!("{}/ (bare)", abs.display())
                } else {
                    abs.display().to_string()
                }
            );
            println!("  branch:  {} ({})", outcome.branch, outcome.tip.short());
            println!("  fetched: {} branch(es)", outcome.fetched.len());
        }

        #[cfg(feature = "cursor-setup")]
        Command::Setup { sub } => match sub {
            SetupCmd::Cursor { path } => {
                let root = std::path::Path::new(&path)
                    .canonicalize()
                    .unwrap_or_else(|_| path.clone());
                verbose_msg(
                    verbose,
                    &format!("setting up Cursor integration at {}", root.display()),
                );
                let report = setup::setup_cursor(&root)?;
                println!("Cursor integration installed in {}", root.display());
                println!("  Hook scripts: {}", report.hooks_written.join(", "));
                println!("  Rules: {}", report.rules_written.join(", "));
                println!(
                    "  .cursor/hooks.json: {}",
                    if report.hooks_json_updated {
                        "updated"
                    } else {
                        "unchanged"
                    }
                );
                println!(
                    "  .cursor/mcp.json: {}",
                    if report.mcp_json_updated {
                        "updated"
                    } else {
                        "unchanged"
                    }
                );
            }
            SetupCmd::Opencode { path } => {
                let root = std::path::Path::new(&path)
                    .canonicalize()
                    .unwrap_or_else(|_| path.clone());
                verbose_msg(
                    verbose,
                    &format!("setting up OpenCode integration at {}", root.display()),
                );
                let report = setup::setup_opencode(&root)?;
                println!("OpenCode integration installed in {}", root.display());
                println!(
                    "  opencode.json: {}",
                    if report.opencode_json_updated {
                        "updated"
                    } else {
                        "unchanged"
                    }
                );
                println!(
                    "  AGENTS.md: {}",
                    if report.agents_md_written {
                        "written"
                    } else {
                        "unchanged"
                    }
                );
                println!(
                    "  .opencode/plugins/morph-record.ts: {}",
                    if report.plugin_written {
                        "written"
                    } else {
                        "unchanged"
                    }
                );
            }
            SetupCmd::ClaudeCode { path } => {
                let root = std::path::Path::new(&path)
                    .canonicalize()
                    .unwrap_or_else(|_| path.clone());
                verbose_msg(
                    verbose,
                    &format!("setting up Claude Code integration at {}", root.display()),
                );
                let report = setup::setup_claude_code(&root)?;
                println!("Claude Code integration installed in {}", root.display());
                println!("  Hook scripts: {}", report.hooks_written.join(", "));
                println!(
                    "  .claude/settings.json: {}",
                    if report.settings_json_updated {
                        "updated"
                    } else {
                        "unchanged"
                    }
                );
            }
            SetupCmd::Aoe {
                path,
                agent,
                skip_agents,
                no_bind_mount,
                no_dockerfile,
            } => {
                let root = std::path::Path::new(&path)
                    .canonicalize()
                    .unwrap_or_else(|_| path.clone());
                verbose_msg(
                    verbose,
                    &format!(
                        "setting up Agent of Empires integration at {}",
                        root.display()
                    ),
                );
                let opts = setup::AoeSetupOpts {
                    agents: agent,
                    skip_agents,
                    bind_mount: !no_bind_mount,
                    write_dockerfile: !no_dockerfile,
                };
                let report = setup::setup_aoe(&root, &opts)?;
                println!(
                    "Agent of Empires integration installed in {}",
                    root.display()
                );
                println!(
                    "  .agent-of-empires/config.toml: {}",
                    if report.config_toml_updated {
                        "updated"
                    } else {
                        "unchanged"
                    }
                );
                println!(
                    "  .agent-of-empires/Dockerfile.morph-aoe: {}",
                    if report.dockerfile_written {
                        "written"
                    } else {
                        "skipped"
                    }
                );
                println!(
                    "  AGENTS.md: {}",
                    if report.agents_md_written {
                        "written"
                    } else {
                        "unchanged"
                    }
                );
                if report.delegated.is_empty() {
                    println!("  Per-agent setups: skipped");
                } else {
                    println!("  Per-agent setups: {}", report.delegated.join(", "));
                }
            }
        },

        Command::Upgrade => {
            let cwd = std::env::current_dir()?;
            let repo_root = find_repo(&cwd)
                .ok_or_else(|| anyhow::anyhow!("not a morph repository (or any parent)"))?;
            let morph_dir = repo_root.join(".morph");
            let report = migrate_to_latest(&morph_dir)?;
            verbose_msg(
                verbose,
                &format!(
                    "{} → {} ({} step(s))",
                    report.initial_version,
                    report.final_version,
                    report.steps.len()
                ),
            );
            if report.is_noop() {
                println!(
                    "Store version is {} (latest). No upgrade needed.",
                    report.final_version
                );
            } else if report.steps.len() == 1 {
                let s = &report.steps[0];
                println!(
                    "Migrated store from {} to {} ({}).",
                    s.from, s.to, s.description
                );
            } else {
                let detail: Vec<String> = report
                    .steps
                    .iter()
                    .map(|s| format!("{}→{} ({})", s.from, s.to, s.description))
                    .collect();
                println!(
                    "Migrated store from {} to {} via {}.",
                    report.initial_version,
                    report.final_version,
                    detail.join(", ")
                );
            }

            // v0.40 standalone → reference migration. Pre-0.40 repos
            // could carry `repo_mode: "standalone"` in their config;
            // that flag is gone as of 0.40 (reference is the only
            // mode). The migration:
            //   1. Drop the legacy `repo_mode` key from config.
            //   2. If a sibling `.git/` exists, install the
            //      reference-mode hooks + `.git/info/exclude` line +
            //      capture `init_at_git_sha` so the repo gets the
            //      same wiring fresh `morph init` produces.
            //   3. If no `.git/` exists, error out with the recipe.
            let was_legacy = morph_core::is_legacy_standalone(&morph_dir)?;
            if was_legacy {
                if !morph_core::is_git_working_tree(&repo_root) {
                    anyhow::bail!(
                        "morph upgrade: this repo was Standalone (pre-0.40) and has no `.git/` \
                         alongside. v0.40+ requires git. Either run `git init` here and \
                         re-run `morph upgrade`, or pin to morph 0.39.x with `cargo install \
                         --version 0.39.2 morph-cli`."
                    );
                }
                morph_core::drop_legacy_repo_mode(&morph_dir)?;
                if morph_core::read_init_at_git_sha(&morph_dir)?.is_none() {
                    if let Some(sha) = morph_core::git_head_sha(&repo_root)? {
                        morph_core::write_init_at_git_sha(&morph_dir, &sha)?;
                    }
                }
                let submode = morph_core::read_repo_submode(&morph_dir)?;
                let _ = morph_core::install_reference_hooks(&repo_root, submode)?;
                let _ = morph_core::ensure_morph_in_git_info_exclude(&repo_root)?;
                println!(
                    "Migrated repo mode: standalone → reference. \
                     `.morph/` is now in `.git/info/exclude`; reference-mode hooks installed. \
                     If `.morph/` was previously tracked by git, run \
                     `git rm -r --cached .morph && git commit` to stop tracking it."
                );
            }
        }

        Command::RemoteHelper { repo_root } => {
            // The helper intentionally bypasses `get_store` so it
            // can produce a clear "not a morph repository" message
            // for the SSH client instead of inheriting the CLI's
            // discover-via-cwd behavior.
            remote_helper::run(&repo_root)?;
        }

        Command::Forget {
            hash,
            reason,
            force,
            remote,
            dry_run,
            yes,
        } => {
            let (repo_root, _store_handle) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let fs_store = morph_core::FsStore::from_store_version(&morph_dir)?;

            let target = morph_core::resolve_hash_prefix(&fs_store, &hash)?;

            // Pre-flight: load and classify so dry-run / confirm
            // can be honest about what's about to die.
            if morph_core::Store::is_forgotten(&fs_store, &target)? {
                anyhow::bail!("{} is already forgotten in this store", target);
            }
            let obj = morph_core::Store::get(&fs_store, &target)?;
            let kind = morph_core::forget::forgettable_kind_label(&obj).ok_or_else(|| {
                anyhow::anyhow!(
                    "refused: {} is not a forgettable kind. \
                     `morph forget` only retires runs, traces, and prompt blobs; \
                     other kinds carry structural meaning the DAG depends on.",
                    target
                )
            })?;
            let referencing = morph_core::commits_referencing(&fs_store, &target)?;

            println!("Forget plan:");
            println!("  hash:     {}", target);
            println!("  kind:     {}", kind);
            println!("  reason:   {}", reason.as_deref().unwrap_or("(none)"));
            if referencing.is_empty() {
                println!("  commits:  0 (no commit references this hash)");
            } else {
                let preview: Vec<String> = referencing
                    .iter()
                    .take(3)
                    .map(|h| hex_prefix(&h.to_string(), 12).to_string())
                    .collect();
                let extra = if referencing.len() > 3 {
                    format!(" (+{} more)", referencing.len() - 3)
                } else {
                    String::new()
                };
                println!(
                    "  commits:  {} referencing: {}{}",
                    referencing.len(),
                    preview.join(", "),
                    extra
                );
                if !force {
                    println!(
                        "  refuse:   yes — pass --force to forget anyway \
                         (merge gate will read those refs as 'no claim')"
                    );
                }
            }
            if let Some(name) = &remote {
                println!(
                    "  remote:   {} (push tombstone with `morph push {}`)",
                    name, name
                );
            }

            if dry_run {
                println!("\n--dry-run: no objects deleted.");
                return Ok(());
            }

            if !referencing.is_empty() && !force {
                anyhow::bail!(
                    "refused: {} is named in evidence_refs of {} commit(s). \
                     Pass --force to forget anyway.",
                    target,
                    referencing.len()
                );
            }

            // Interactive confirmation. Non-TTY callers must pass
            // --yes; this stops a runaway script from forgetting
            // the wrong hash.
            if !yes {
                use std::io::IsTerminal;
                if !std::io::stdin().is_terminal() {
                    anyhow::bail!(
                        "morph forget refuses non-interactive input without --yes. \
                         Re-run as `morph forget {} --yes` (and consider --reason '<...>').",
                        &hash
                    );
                }
                eprint!(
                    "Forget {} (kind={}). Type 'forget' to confirm: ",
                    target, kind
                );
                use std::io::Write;
                std::io::stderr().flush().ok();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                if line.trim() != "forget" {
                    anyhow::bail!("aborted: confirmation text did not match 'forget'");
                }
            }

            let actor = morph_core::resolve_author_for_repo(&morph_dir, None)?;
            let report =
                morph_core::forget_local(&fs_store, &target, &actor, reason.as_deref(), force)?;

            println!(
                "forgot {} {}; tombstone {}",
                report.original_kind, report.original_hash, report.tombstone_hash
            );
            if !report.referencing_commits.is_empty() {
                println!(
                    "{} commit(s) now read as 'no claim' on this evidence \
                     (merge gate will warn at next merge).",
                    report.referencing_commits.len()
                );
            }

            if let Some(name) = &remote {
                let remotes = morph_core::read_remotes(&morph_dir)?;
                if !remotes.contains_key(name) {
                    eprintln!(
                        "warning: remote '{}' is not configured. \
                         The tombstone is recorded locally; \
                         configure with `morph remote add {} <url>` and run \
                         `morph push {} <branch>` to ship it.",
                        name, name, name
                    );
                } else {
                    println!(
                        "Tombstone queued for remote '{}'. Run `morph push {} <branch>` \
                         to propagate.",
                        name, name
                    );
                }
            }

            println!("\nNote: {}", morph_core::RETROACTIVE_NOTE);
        }

        Command::Gc => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let fs_store = morph_core::FsStore::from_store_version(&morph_dir)?;
            let result = morph_core::gc(&fs_store, &morph_dir)?;
            if result.objects_removed == 0 {
                println!(
                    "Nothing to clean up. {} objects total.",
                    result.objects_before
                );
            } else {
                let freed_mb = result.bytes_freed as f64 / 1_048_576.0;
                println!(
                    "Removed {} unreachable objects ({:.1} MB freed). {} objects remaining.",
                    result.objects_removed, freed_mb, result.objects_after
                );
            }
            drop(store);
        }

        #[cfg(feature = "visualize")]
        Command::Visualize {
            path,
            port,
            interface,
        } => {
            let repo_root = path.canonicalize().unwrap_or(path);
            let morph_dir = if repo_root.join(".morph").exists() {
                repo_root.join(".morph")
            } else {
                find_repo(&repo_root)
                    .ok_or_else(|| anyhow::anyhow!("not a morph repository"))?
                    .join(".morph")
            };
            let addr = format!("{}:{}", interface, port)
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid address: {}:{}", interface, port))?;
            morph_serve::run_blocking(morph_dir, addr).map_err(|e| anyhow::anyhow!("{}", e))?;
        }

        #[cfg(feature = "visualize")]
        Command::Serve {
            repos,
            port,
            interface,
            org_policy,
        } => {
            let repo_entries = if repos.is_empty() {
                let cwd = std::env::current_dir()?;
                let repo_root = find_repo(&cwd).ok_or_else(|| {
                    anyhow::anyhow!("not a morph repository; specify --repo name=path")
                })?;
                vec![morph_serve::RepoEntry {
                    name: "default".into(),
                    morph_dir: repo_root.join(".morph"),
                }]
            } else {
                repos
                    .iter()
                    .map(|spec| {
                        let (name, path_str) = spec.split_once('=').ok_or_else(|| {
                            anyhow::anyhow!("repo spec must be name=path, got: {}", spec)
                        })?;
                        let path = PathBuf::from(path_str);
                        let morph_dir = if path.join(".morph").exists() {
                            path.join(".morph")
                        } else {
                            find_repo(&path)
                                .ok_or_else(|| {
                                    anyhow::anyhow!("not a morph repository: {}", path_str)
                                })?
                                .join(".morph")
                        };
                        Ok(morph_serve::RepoEntry {
                            name: name.to_string(),
                            morph_dir,
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?
            };
            let addr: std::net::SocketAddr = format!("{}:{}", interface, port)
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid address: {}:{}", interface, port))?;
            morph_serve::run_service(morph_serve::ServiceConfig {
                repos: repo_entries,
                addr,
                org_policy_path: org_policy,
            })
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        }

        Command::Diff {
            old_ref,
            new_ref,
            json,
        } => {
            let (_repo_root, store) = get_store(verbose)?;
            let old_hash = resolve_ref_name(&store, &old_ref)?;
            let new_hash = resolve_ref_name(&store, &new_ref)?;
            let entries = morph_core::diff_commits(&store, &old_hash, &new_hash)?;
            if json {
                let changes: Vec<_> = entries
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "status": e.status.to_string(),
                            "path": e.path,
                        })
                    })
                    .collect();
                let body = serde_json::json!({
                    "from": { "ref": old_ref, "hash": old_hash.to_string() },
                    "to":   { "ref": new_ref, "hash": new_hash.to_string() },
                    "changes": changes,
                    "count": entries.len(),
                });
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                for e in &entries {
                    println!("{}  {}", e.status, e.path);
                }
            }
        }

        Command::Tag { name, delete, json } => {
            let (_repo_root, store) = get_store(verbose)?;
            if delete {
                let name = name.ok_or_else(|| anyhow::anyhow!("tag name required with -d"))?;
                morph_core::delete_tag(&store, &name)?;
                println!("Deleted tag {}", name);
            } else if let Some(name) = name {
                let head = morph_core::resolve_head(&store)?
                    .ok_or_else(|| anyhow::anyhow!("no commit yet"))?;
                morph_core::create_tag(&store, &name, &head)?;
                println!("Tagged {} as {}", head, name);
            } else {
                let tags = morph_core::list_tags(&store)?;
                if json {
                    let entries: Vec<_> = tags
                        .iter()
                        .map(|(name, hash)| {
                            serde_json::json!({
                                "name": name,
                                "hash": hash.to_string(),
                                "short": hash.short(),
                            })
                        })
                        .collect();
                    let body = serde_json::json!({ "tags": entries, "count": tags.len() });
                    println!("{}", serde_json::to_string_pretty(&body)?);
                } else {
                    for (name, hash) in &tags {
                        println!("{}  {}", name, hash);
                    }
                }
            }
        }

        Command::Stash { sub } => {
            let (repo_root, _store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            match sub {
                StashCmd::Save { message } => {
                    let entry = morph_core::stash_save(&morph_dir, message.as_deref())?;
                    println!(
                        "Saved stash {}{}",
                        entry.id,
                        entry
                            .message
                            .as_ref()
                            .map(|m| format!(": {}", m))
                            .unwrap_or_default()
                    );
                }
                StashCmd::Pop => {
                    let entry = morph_core::stash_pop(&morph_dir)?;
                    println!(
                        "Restored stash {}{}",
                        entry.id,
                        entry
                            .message
                            .as_ref()
                            .map(|m| format!(": {}", m))
                            .unwrap_or_default()
                    );
                }
                StashCmd::List => {
                    for (i, entry) in morph_core::stash_list(&morph_dir)?.iter().enumerate() {
                        println!(
                            "stash@{{{}}}: {}",
                            i,
                            entry.message.as_deref().unwrap_or("(no message)")
                        );
                    }
                }
            }
        }

        Command::Revert { commit, author } => {
            let (_repo_root, store) = get_store(verbose)?;
            let hash = resolve_obj_hash(store.as_ref(), &commit)?;
            let revert_hash = morph_core::revert_commit(&store, &hash, author)?;
            println!("{}", revert_hash);
        }

        Command::Prompt { sub } => match sub {
            PromptCmd::Create { path } => {
                let (repo_root, store) = get_store(verbose)?;
                let full = if path.is_absolute() {
                    path
                } else {
                    repo_root.join(&path)
                };
                let obj = morph_core::blob_from_prompt_file(&full)?;
                let hash = store.put(&obj)?;
                println!("{}", hash);
            }
            PromptCmd::Materialize { hash, output } => {
                let (repo_root, store) = get_store(verbose)?;
                let h = resolve_obj_hash(store.as_ref(), &hash)?;
                let dest = output.unwrap_or_else(|| {
                    repo_root
                        .join(".morph")
                        .join("prompts")
                        .join(format!("{}.prompt", h))
                });
                morph_core::materialize_blob(&store, &h, &dest)?;
                println!("Materialized to {}", dest.display());
            }
            PromptCmd::Show {
                run_ref,
                run_upgrade,
            } => {
                cmd_prompt_show(verbose, &run_ref, run_upgrade)?;
            }
        },

        Command::Pipeline { sub } => match sub {
            PipelineCmd::Create { path } => {
                let (repo_root, store) = get_store(verbose)?;
                let full = if path.is_absolute() {
                    path
                } else {
                    repo_root.join(&path)
                };
                let obj = morph_core::pipeline_from_file(&full)?;
                println!("{}", store.put(&obj)?);
            }
            PipelineCmd::IdentityHash => {
                let (_repo_root, store) = get_store(verbose)?;
                println!("{}", store.put(&morph_core::identity_pipeline())?);
            }
            PipelineCmd::Show { hash } => {
                let (_repo_root, store) = get_store(verbose)?;
                let obj = store.get(&resolve_obj_hash(store.as_ref(), &hash)?)?;
                println!("{}", serde_json::to_string_pretty(&obj)?);
            }
            PipelineCmd::Extract { from_run } => {
                let (_repo_root, store) = get_store(verbose)?;
                let run_hash = resolve_obj_hash(store.as_ref(), &from_run)?;
                println!(
                    "{}",
                    morph_core::extract_pipeline_from_run(&store, &run_hash)?
                );
            }
        },

        Command::Status { json } => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let changes = morph_core::working_status(&store, &repo_root)?;
            let summary = morph_core::activity_summary(&store, &repo_root)?;
            let merge_progress = morph_core::merge_progress_summary(&*store, &repo_root)?;
            if json {
                let body = morph_core::build_status_json(&repo_root, store.as_ref())?;
                println!("{}", serde_json::to_string_pretty(&body)?);
                return Ok(());
            }

            if let Some(progress) = &merge_progress {
                if let Some(branch) = &progress.on_branch {
                    println!("On branch {}", branch);
                }
                println!("You have unmerged paths.");
                println!("  (fix conflicts and run \"morph merge --continue\")");
                println!("  (use \"morph merge --abort\" to abort the merge)");
                println!();
                if !progress.unmerged_paths.is_empty() {
                    println!("Unmerged paths:");
                    println!("  (use \"morph add <file>...\" to mark resolution)");
                    for p in &progress.unmerged_paths {
                        println!("\tboth modified:   {}", p);
                    }
                    println!();
                }
                if !progress.pipeline_node_conflicts.is_empty() {
                    println!("Pipeline nodes needing resolution:");
                    println!("  (use \"morph merge resolve-node <id> --pick ours|theirs\")");
                    for id in &progress.pipeline_node_conflicts {
                        println!("\tnode:   {}", id);
                    }
                    println!();
                }
            }

            let nothing_to_commit = changes.is_empty()
                && summary.runs == 0
                && summary.traces == 0
                && summary.prompts == 0
                && merge_progress.is_none();
            if nothing_to_commit {
                println!("nothing to commit, working tree clean");
            }

            if !changes.is_empty() {
                println!("Changes not staged for commit:");
                println!();
                for entry in &changes {
                    let tag = match entry.status {
                        morph_core::DiffStatus::Added => "new file",
                        morph_core::DiffStatus::Modified => "modified",
                        morph_core::DiffStatus::Deleted => "deleted",
                    };
                    println!("\t{:>12}:   {}", tag, entry.path);
                }
                println!();
            }

            if summary.runs > 0 || summary.traces > 0 || summary.prompts > 0 {
                let mut parts = Vec::new();
                if summary.runs > 0 {
                    parts.push(format!(
                        "{} run{}",
                        summary.runs,
                        if summary.runs == 1 { "" } else { "s" }
                    ));
                }
                if summary.traces > 0 {
                    parts.push(format!(
                        "{} trace{}",
                        summary.traces,
                        if summary.traces == 1 { "" } else { "s" }
                    ));
                }
                if summary.prompts > 0 {
                    parts.push(format!(
                        "{} prompt{}",
                        summary.prompts,
                        if summary.prompts == 1 { "" } else { "s" }
                    ));
                }
                println!("Morph activity: {}", parts.join(", "));
            }

            // Phase 1b: nudge when HEAD has no observed_metrics so the
            // gap is visible to anyone who runs `morph status`. Also
            // surfaces the empty default eval suite so users aren't
            // left wondering where their behavioral evidence lives.
            //
            // PR 1 (unified certification): a HEAD commit with empty
            // inline metrics but a passing certification annotation
            // is fine — the warning suppresses once `morph certify`
            // attaches evidence, matching the `morph eval gaps` and
            // merge-gate behavior.
            if let Some(head) = morph_core::resolve_head(&store)? {
                if matches!(store.get(&head)?, MorphObject::Commit(_)) {
                    let effective = morph_core::effective_metrics(&store, &head)?;
                    if effective.is_empty() {
                        println!("warning: HEAD has no observed_metrics");
                    }
                }
            }

            // The user's compass for the git ↔ morph relationship.
            // Surfaces:
            //   - drift count (git ahead of morph),
            //   - uncertified git-hook commits,
            //   - stale certifications from amend/rebase rewrites.
            // Reference mode is the only mode (v0.40+); the
            // `is_git_working_tree` guard exists so unit-test
            // tempdirs without a `.git/` skip the block.
            if morph_core::is_git_working_tree(&repo_root) {
                let drift = morph_core::drift_summary(store.as_ref(), &repo_root)?;
                println!("Reference mode (git ↔ morph)");
                if let Some(sha) = drift.git_head.as_deref() {
                    println!("  git HEAD:        {}", hex_prefix(sha, 12));
                }
                if drift.is_up_to_date() {
                    println!("  drift:           up to date");
                } else {
                    println!(
                        "  drift:           {} unmirrored git commit{} — run `morph reference-sync`",
                        drift.unmirrored_count,
                        if drift.unmirrored_count == 1 { "" } else { "s" },
                    );
                    if let Some(last) = drift.last_mirrored_git_sha.as_deref() {
                        println!("  last mirrored:   {}", hex_prefix(last, 12));
                    } else {
                        println!(
                            "  last mirrored:   (none — run `morph reference-sync --backfill`)"
                        );
                    }
                }
                let stale = morph_core::list_stale_certifications(store.as_ref())?;
                if !stale.is_empty() {
                    println!(
                        "  stale certification{}: {} (a rewritten commit had certification evidence — re-certify the successor)",
                        if stale.len() == 1 { "" } else { "s" },
                        stale.len(),
                    );
                }
                if let Some(head) = morph_core::resolve_head(&store)? {
                    let pending = morph_core::pending_certifications(store.as_ref(), &head)?;
                    if !pending.is_empty() {
                        println!(
                            "{} uncertified git-hook commit{} on this branch — run `morph certify` to attach evidence.",
                            pending.len(),
                            if pending.len() == 1 { "" } else { "s" },
                        );
                    }
                }
                // PR 11: surface a mid-merge state so the user knows
                // they need to resolve conflicts and run --continue
                // (or --abort to back out). The breadcrumb is written
                // by `morph merge` when `git merge` returns conflicts.
                if let Some(bc) = morph_core::read_merge_breadcrumb(&morph_dir)? {
                    println!(
                        "  merge in progress: '{}' → current branch (use `morph merge --continue` or `morph merge --abort`)",
                        bc.other_branch
                    );
                }
            }

            let policy = morph_core::read_policy(&morph_dir)?;
            let suite_cases = match policy.default_eval_suite.as_deref() {
                Some(suite_hex) => match Hash::from_hex(suite_hex)
                    .ok()
                    .and_then(|h| store.get(&h).ok())
                {
                    Some(MorphObject::EvalSuite(s)) => Some(s.cases.len()),
                    _ => None,
                },
                None => None,
            };
            match suite_cases {
                Some(0) | None => println!("Eval suite: 0 cases registered"),
                Some(n) => println!(
                    "Eval suite: {} case{} registered",
                    n,
                    if n == 1 { "" } else { "s" }
                ),
            }
        }

        Command::Files { json } => {
            let (repo_root, store) = get_store(verbose)?;
            let entries = morph_core::status(&store, &repo_root)?;
            if json {
                let items: Vec<_> = entries
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "path": e.path.display().to_string(),
                            "status": if e.in_store { "tracked" } else { "new" },
                            "hash": e.hash.as_ref().map(|h| h.to_string()),
                        })
                    })
                    .collect();
                let body = serde_json::json!({ "files": items, "count": entries.len() });
                println!("{}", serde_json::to_string_pretty(&body)?);
                return Ok(());
            }
            if entries.is_empty() {
                println!("No files to track");
                return Ok(());
            }
            for e in entries {
                let status = if e.in_store { "tracked" } else { "new" };
                let hash_str = e.hash.as_ref().map(|h| h.to_string()).unwrap_or_default();
                println!("{} {} {}", status, hash_str, e.path.display());
            }
        }

        Command::Add { paths } => {
            let (repo_root, store) = get_store(verbose)?;
            let any_dir = paths
                .iter()
                .any(|p| p.as_os_str() == "." || repo_root.join(p).is_dir());
            let hashes = morph_core::add_paths(&store, &repo_root, &paths)?;
            // Reference mode is the only mode (v0.40+); morph
            // commits ride on top of git commits. Mirror the user's
            // `morph add <paths>` to `git add <paths>` so a follow-up
            // `morph commit` (which wraps `git commit`) actually has
            // something to commit. Skip silently for the unit-test
            // tempdir case where there's no `.git/`.
            if morph_core::is_git_working_tree(&repo_root) {
                let mut cmd = std::process::Command::new("git");
                cmd.current_dir(&repo_root).arg("add").arg("--");
                for p in &paths {
                    cmd.arg(p);
                }
                let _ = cmd.status();
            }
            if any_dir && !verbose {
                if !hashes.is_empty() {
                    eprintln!(
                        "{} file{} staged",
                        hashes.len(),
                        if hashes.len() == 1 { "" } else { "s" }
                    );
                }
            } else {
                for h in &hashes {
                    println!("{}", h);
                }
            }
        }

        Command::Commit {
            message,
            pipeline,
            eval_suite,
            metrics,
            author,
            from_run,
            allow_empty_metrics,
            new_cases,
            no_auto_run,
            no_test,
            rerun,
            json,
            allow_empty_commit,
        } => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let version = read_repo_version(&morph_dir)?;
            // `morph commit` is a thin wrapper around `git commit`
            // with `MORPH_INTERNAL=1` to suppress the post-commit
            // hook, followed by an explicit morph mirror tagged
            // `morph_origin = "cli"`. Reference mode is the only
            // mode (v0.40+); the `is_git_working_tree` guard exists
            // so unit-test tempdirs without a `.git/` fall through
            // to the legacy structural commit path.
            if morph_core::is_git_working_tree(&repo_root) {
                run_reference_commit(
                    store.as_ref(),
                    &repo_root,
                    &morph_dir,
                    &version,
                    &message,
                    metrics.as_deref(),
                    from_run.as_deref(),
                    new_cases.as_deref(),
                    eval_suite.as_deref(),
                    pipeline.as_deref(),
                    author.as_deref(),
                    allow_empty_metrics,
                    allow_empty_commit,
                    no_auto_run,
                    no_test,
                    rerun,
                    json,
                )?;
                if let Err(e) = morph_core::clear_last_run(&morph_dir) {
                    eprintln!(
                        "warning: could not clear LAST_RUN breadcrumb after commit: {}",
                        e
                    );
                }
                return Ok(());
            }
            // Phase 2 (v0.44+): mirror the reference-mode auto-run on
            // the standalone path so spec tests (which run in
            // tempdirs without a `.git/`) and any remaining
            // standalone users get the same one-command commit
            // ergonomics.
            if !no_auto_run {
                maybe_run_configured_test(
                    store.as_ref(),
                    &repo_root,
                    &morph_dir,
                    no_test,
                    rerun,
                    from_run.is_some(),
                )?;
            }
            // Reference-mode-only flag: a no-op for standalone commits
            // since we don't shell out to git here.
            let _ = allow_empty_commit;
            let prog_hash = pipeline
                .as_deref()
                .map(|s| resolve_obj_hash(store.as_ref(), s))
                .transpose()?;
            let policy = morph_core::read_policy(&morph_dir)?;
            // Resolve --eval-suite, falling back to the policy default
            // when unset so commits inherit the latest registered
            // acceptance suite instead of pointing at an empty one.
            let suite_hash = match eval_suite.as_deref() {
                Some(s) => Some(resolve_obj_hash(store.as_ref(), s)?),
                None => match policy.default_eval_suite.as_deref() {
                    Some(s) => Some(resolve_obj_hash(store.as_ref(), s)?),
                    None => None,
                },
            };

            // Phase 7 step 1: when neither `--from-run` nor `--metrics`
            // is supplied (and `--no-auto-run` isn't set), pick up the
            // most recent `morph eval run` from `.morph/LAST_RUN.json`
            // — but only if HEAD and the staging index still match
            // what they were when the run was recorded. Stale
            // breadcrumbs are skipped with a stderr nudge so the user
            // can see why their evidence wasn't attached.
            let auto_run_hash: Option<Hash> = if no_auto_run || from_run.is_some() {
                None
            } else {
                match morph_core::resolve_fresh_last_run(store.as_ref(), &morph_dir) {
                    Ok((Some(last), _)) => match Hash::from_hex(&last.run) {
                        Ok(h) => Some(h),
                        Err(e) => {
                            eprintln!("warning: ignoring LAST_RUN breadcrumb (bad hash): {}", e);
                            None
                        }
                    },
                    Ok((None, Some(reason))) => {
                        eprintln!(
                            "warning: ignoring stale `morph eval run` evidence ({}). \
                             Re-run `morph eval run` to refresh, or pass \
                             --metrics / --from-run / --no-auto-run.",
                            reason.as_human(),
                        );
                        None
                    }
                    Ok((None, None)) => None,
                    Err(e) => {
                        eprintln!("warning: could not read LAST_RUN breadcrumb: {}", e);
                        None
                    }
                }
            };

            let mut observed_metrics: std::collections::BTreeMap<String, f64> = metrics
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?
                .unwrap_or_default();
            // Auto-attach the run's metrics only when the user didn't
            // pass `--metrics`. Explicit metrics always win so CI
            // scripts retain deterministic, repeatable input even
            // when a stale breadcrumb is sitting on disk.
            //
            // Precedence inside this branch:
            //   1. `--from-run <hash>` — the user explicitly nominated
            //      a run as evidence, so propagate its `metrics` map
            //      into observed_metrics. Without this, --from-run
            //      silently produced metrics-less commits even though
            //      the run had full evidence — the merge gate had no
            //      data to compare and `morph eval gaps` kept
            //      reporting `empty_head_metrics`.
            //   2. `LAST_RUN.json` breadcrumb (auto_run_hash) — the
            //      single-use bridge written by `morph eval run`,
            //      consumed silently by the next `morph commit`.
            if metrics.is_none() {
                if let Some(s) = from_run.as_deref() {
                    let run_hash = resolve_obj_hash(store.as_ref(), s)?;
                    match store.get(&run_hash) {
                        Ok(MorphObject::Run(run)) => {
                            if !run.metrics.is_empty() {
                                let preview: Vec<String> = run
                                    .metrics
                                    .iter()
                                    .map(|(k, v)| format!("{}={}", k, v))
                                    .collect();
                                eprintln!(
                                    "attaching evidence from run {}: {}",
                                    run_hash.short(),
                                    preview.join(", "),
                                );
                                observed_metrics = run.metrics.clone();
                            }
                        }
                        Ok(_) => {
                            eprintln!(
                                "warning: --from-run {} is not a Run object; metrics not attached",
                                run_hash.short(),
                            );
                        }
                        Err(e) => {
                            eprintln!(
                                "warning: could not load run from --from-run {}: {}",
                                run_hash.short(),
                                e,
                            );
                        }
                    }
                } else if let Some(ref run_hash) = auto_run_hash {
                    match store.get(run_hash) {
                        Ok(MorphObject::Run(run)) => {
                            if !run.metrics.is_empty() {
                                let preview: Vec<String> = run
                                    .metrics
                                    .iter()
                                    .map(|(k, v)| format!("{}={}", k, v))
                                    .collect();
                                eprintln!(
                                    "attaching evidence from run {}: {}",
                                    run_hash.short(),
                                    preview.join(", "),
                                );
                                observed_metrics = run.metrics.clone();
                            }
                        }
                        Ok(_) => {
                            eprintln!(
                                "warning: LAST_RUN breadcrumb points at non-Run object {}; ignoring",
                                run_hash.short(),
                            );
                        }
                        Err(e) => {
                            eprintln!(
                                "warning: could not load run from LAST_RUN breadcrumb: {}",
                                e
                            );
                        }
                    }
                }
            }

            // Phase 2: enforce required_metrics from policy unless escape hatch is set.
            if !allow_empty_metrics {
                let missing = morph_core::missing_required_metrics(&policy, &observed_metrics);
                if !missing.is_empty() {
                    return Err(anyhow::anyhow!(
                        "policy requires metrics that are missing: [{}]. \
                         Pass --metrics with these keys, run `morph eval record`, \
                         or override with --allow-empty-metrics. \
                         (Configured in .morph/config.json under policy.required_metrics.)",
                        missing.join(", ")
                    ));
                }
            }
            let provenance = match from_run
                .as_ref()
                .map(|s| resolve_obj_hash(store.as_ref(), s))
                .transpose()?
            {
                Some(run_hash) => Some(morph_core::resolve_provenance_from_run(&store, &run_hash)?),
                None => match &auto_run_hash {
                    Some(h) => Some(morph_core::resolve_provenance_from_run(&store, h)?),
                    None => None,
                },
            };

            let branch = morph_core::current_branch(&store)?.unwrap_or_else(|| "main".to_string());
            let is_root = morph_core::resolve_head(&store)?.is_none();
            let head_before_commit = morph_core::resolve_head(&store)?;
            let index = morph_core::read_index(&morph_dir)?;
            let file_count = index.entries.len();
            let metrics_were_empty = observed_metrics.is_empty();

            // Phase 7 step 3: when `--new-cases` is unset, auto-detect
            // newly introduced acceptance cases by diffing the
            // about-to-commit suite against HEAD's suite. Pass
            // `--new-cases ""` to opt out without listing manual ids.
            let auto_new_cases: Option<Vec<String>> = if new_cases.is_some() {
                None
            } else {
                let head_suite_hash: Option<Hash> = match head_before_commit.as_ref() {
                    Some(h) => match store.get(h) {
                        Ok(MorphObject::Commit(c)) => Hash::from_hex(&c.eval_contract.suite).ok(),
                        _ => None,
                    },
                    None => None,
                };
                let diff = morph_core::diff_suite_case_ids(
                    store.as_ref(),
                    suite_hash.as_ref(),
                    head_suite_hash.as_ref(),
                )?;
                if diff.is_empty() {
                    None
                } else {
                    Some(diff)
                }
            };

            let resolved_author =
                morph_core::resolve_author_for_repo(&morph_dir, author.as_deref())?;
            let hash = morph_core::create_tree_commit_with_provenance(
                &store,
                &repo_root,
                prog_hash.as_ref(),
                suite_hash.as_ref(),
                observed_metrics,
                message.clone(),
                Some(resolved_author),
                Some(&version),
                provenance.as_ref(),
            )?;

            // Phase 6b: record which acceptance cases this commit
            // introduces via an `introduces_cases` annotation. The
            // case ids are caller-defined; we just split, trim, and
            // store them so merge planning can show provenance.
            // Phase 7 step 3 extension: when `--new-cases` is unset
            // we substitute the suite-diff result.
            let cases_for_annotation: Option<Vec<String>> = match new_cases.as_deref() {
                Some(arg) => Some(morph_core::parse_introduces_cases_arg(arg)),
                None => auto_new_cases,
            };
            if let Some(cases) = cases_for_annotation {
                if let Some(ann) = morph_core::build_introduces_cases_annotation(
                    &hash,
                    &cases,
                    Some(branch.clone()),
                ) {
                    store.put(&ann)?;
                }
            }

            // Phase 7 step 1 cleanup: clear the breadcrumb after a
            // successful commit so a follow-up `morph commit` doesn't
            // silently re-attach the same run. The breadcrumb is
            // single-use by design — its job is to bridge the
            // `eval run` → `commit` boundary, not to persist evidence.
            if let Err(e) = morph_core::clear_last_run(&morph_dir) {
                eprintln!(
                    "warning: could not clear LAST_RUN breadcrumb after commit: {}",
                    e
                );
            }

            // Phase 1a: warn when committing without observed_metrics.
            // Morph cannot enforce behavioral merge gating without
            // evidence; print a stderr nudge so the leak is visible.
            if metrics_were_empty {
                eprintln!(
                    "warning: commit has no observed_metrics. Morph cannot enforce \
                     behavioral merge gating without evidence. Pass --metrics, \
                     run `morph eval record` / `morph eval run`, or set a policy \
                     via `morph policy init`."
                );
            }

            if json {
                let out = serde_json::json!({
                    "hash": hash.to_string(),
                    "branch": branch,
                    "root_commit": is_root,
                    "message": message,
                    "files_committed": file_count,
                });
                println!("{}", serde_json::to_string(&out)?);
            } else {
                let short = hash.short();
                let root_tag = if is_root { " (root-commit)" } else { "" };
                let first_line = message.lines().next().unwrap_or("");
                println!("[{}{} {}] {}", branch, root_tag, short, first_line);
                if file_count > 0 {
                    println!(
                        " {} file{} committed",
                        file_count,
                        if file_count == 1 { "" } else { "s" }
                    );
                }
            }
        }

        Command::Show { hash } => {
            let (_repo_root, store) = get_store(verbose)?;
            let obj = store.get(&resolve_obj_hash(store.as_ref(), &hash)?)?;
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }

        Command::Log {
            ref_name,
            max_count,
            oneline,
            full_hash,
            json,
        } => {
            let (_repo_root, store) = get_store(verbose)?;
            let mut hashes = morph_core::log_from(&store, &ref_name)?;
            if let Some(n) = max_count {
                hashes.truncate(n);
            }
            if json {
                let mut entries = Vec::with_capacity(hashes.len());
                for h in &hashes {
                    if let MorphObject::Commit(c) = store.get(h)? {
                        // PR 1: log JSON reports *effective* metrics
                        // so a late `morph certify` immediately shows
                        // up in `morph log --json` without rewriting
                        // history.
                        let metrics = morph_core::effective_metrics_for_commit(&store, h, &c)?;
                        entries.push(serde_json::json!({
                            "hash": h.to_string(),
                            "short": h.short(),
                            "message": c.message,
                            "author": c.author,
                            "timestamp": c.timestamp,
                            "parents": c.parents,
                            "morph_version": c.morph_version,
                            "has_tree": c.tree.is_some(),
                            "metrics": metrics,
                        }));
                    }
                }
                let body = serde_json::json!({ "commits": entries, "count": hashes.len() });
                println!("{}", serde_json::to_string_pretty(&body)?);
                return Ok(());
            }
            for h in &hashes {
                if let MorphObject::Commit(c) = store.get(h)? {
                    let h_str = h.to_string();
                    let display_hash = if full_hash { h_str.clone() } else { h.short() };
                    let subject = c.message.lines().next().unwrap_or("");
                    if oneline {
                        println!("{}  {}", display_hash, subject);
                    } else {
                        let ver_tag = c
                            .morph_version
                            .as_deref()
                            .map(|v| format!("[v{}]", v))
                            .unwrap_or_else(|| "[pre-tree]".into());
                        let tree_tag = if c.tree.is_some() {
                            ""
                        } else {
                            " (no file tree)"
                        };
                        println!(
                            "{} {} {}{} {}",
                            display_hash, ver_tag, subject, tree_tag, c.author
                        );
                    }
                }
            }
        }

        Command::Head { json } => {
            let (_repo_root, store) = get_store(verbose)?;
            let head_hash = morph_core::resolve_head(&store)?
                .ok_or_else(|| anyhow::anyhow!("no commit yet on this branch"))?;
            let branch = morph_core::current_branch(&store)?;
            let commit = match store.get(&head_hash)? {
                MorphObject::Commit(c) => c,
                _ => anyhow::bail!("HEAD does not point to a commit"),
            };
            let h_str = head_hash.to_string();
            if json {
                // PR 1: report *effective* metrics so late certifications
                // are reflected immediately. Inline values still show up
                // because they're the bedrock layer of effective_metrics.
                let metrics =
                    morph_core::effective_metrics_for_commit(&store, &head_hash, &commit)?;
                let body = serde_json::json!({
                    "hash": h_str,
                    "short": short_hash_str(&h_str),
                    "branch": branch,
                    "detached": branch.is_none(),
                    "message": commit.message,
                    "author": commit.author,
                    "timestamp": commit.timestamp,
                    "parents": commit.parents,
                    "metrics": metrics,
                    "morph_origin": commit.morph_origin,
                    "git_origin_sha": commit.git_origin_sha,
                });
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                let where_ = match &branch {
                    Some(b) => format!("on branch {}", b),
                    None => "in detached HEAD state".to_string(),
                };
                let subject = commit.message.lines().next().unwrap_or("");
                println!("HEAD {} ({})", short_hash_str(&h_str), where_);
                println!("    {}", subject);
                println!("    {}  {}", commit.author, commit.timestamp);
            }
        }

        Command::Identify { revision, json } => {
            let (_repo_root, store) = get_store(verbose)?;
            let resolved = resolve_obj_hash(store.as_ref(), &revision)?;
            let obj = store.get(&resolved)?;
            let kind = obj.kind_str();
            let h_str = resolved.to_string();
            if json {
                let mut body = serde_json::json!({
                    "input": revision,
                    "hash": h_str,
                    "short": short_hash_str(&h_str),
                    "type": kind,
                });
                if let MorphObject::Commit(c) = &obj {
                    body["message"] = serde_json::Value::String(c.message.clone());
                    body["author"] = serde_json::Value::String(c.author.clone());
                    body["timestamp"] = serde_json::Value::String(c.timestamp.clone());
                }
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                println!("{}\t{}", h_str, kind);
            }
        }

        Command::Branch {
            name,
            set_upstream,
            json,
        } => {
            let (repo_root, store) = get_store(verbose)?;
            // `branch --set-upstream <remote>/<branch>` works on
            // the named branch (or current if unspecified).
            if let Some(spec) = set_upstream {
                let target = match name.as_ref() {
                    Some(n) => n.clone(),
                    None => morph_core::current_branch(&store)?.ok_or_else(|| {
                        anyhow::anyhow!(
                            "no current branch (detached HEAD?); name a branch explicitly"
                        )
                    })?,
                };
                let (remote, upstream_branch) = spec
                    .split_once('/')
                    .ok_or_else(|| anyhow::anyhow!("expected <remote>/<branch>, got: {}", spec))?;
                morph_core::set_branch_upstream(
                    &repo_root.join(".morph"),
                    &target,
                    morph_core::BranchUpstream {
                        remote: remote.to_string(),
                        branch: upstream_branch.to_string(),
                    },
                )?;
                println!(
                    "Branch '{}' set up to track '{}/{}'",
                    target, remote, upstream_branch
                );
                return Ok(());
            }
            if let Some(branch_name) = name {
                let head = morph_core::resolve_head(&store)?
                    .ok_or_else(|| anyhow::anyhow!("no commit yet; make a commit first"))?;
                store.ref_write(&format!("heads/{}", branch_name), &head)?;
                // Reference mode (v0.40+): mirror the morph branch
                // ref into git so `morph checkout`, `morph merge`,
                // and the underlying `git merge` see the same set
                // of branches. Idempotent: if the branch already
                // exists in git we leave it alone.
                if morph_core::is_git_working_tree(&repo_root) {
                    let _ = std::process::Command::new("git")
                        .current_dir(&repo_root)
                        .args(["branch", "--quiet", &branch_name])
                        .status();
                }
                println!("Created branch {}", branch_name);
            } else {
                // Use the transport-neutral `list_branches` so the
                // same listing works against an SSH-backed remote
                // store (PR5 Stage D) without any code change here.
                let current = morph_core::current_branch(&store)?;
                let mut branches = store.list_branches()?;
                branches.sort_by(|a, b| a.0.cmp(&b.0));
                if json {
                    let entries: Vec<_> = branches
                        .iter()
                        .map(|(name, hash)| {
                            let h_str = hash.to_string();
                            serde_json::json!({
                                "name": name,
                                "hash": h_str,
                                "short": short_hash_str(&h_str),
                                "current": current.as_deref() == Some(name.as_str()),
                            })
                        })
                        .collect();
                    let body = serde_json::json!({
                        "current": current,
                        "branches": entries,
                        "count": branches.len(),
                    });
                    println!("{}", serde_json::to_string_pretty(&body)?);
                } else {
                    for (name, _hash) in branches {
                        let mark = if current.as_deref() == Some(&name) {
                            "* "
                        } else {
                            "  "
                        };
                        println!("{}{}", mark, name);
                    }
                }
            }
        }

        Command::Checkout { ref_name } => {
            let (repo_root, store) = get_store(verbose)?;
            // Reference mode (v0.40+): in a git working tree, the
            // git checkout is what actually moves files around and
            // updates `HEAD`. Drive git first; the post-checkout
            // hook (or the explicit morph step below) updates morph
            // refs to keep them aligned. Skip the git step for bare
            // hex hashes — git wouldn't know about a morph object
            // hash, and morph's detached-HEAD semantics still apply.
            let is_hex_hash =
                ref_name.len() == 64 && ref_name.chars().all(|c| c.is_ascii_hexdigit());
            if !is_hex_hash && morph_core::is_git_working_tree(&repo_root) {
                let status = std::process::Command::new("git")
                    .current_dir(&repo_root)
                    .args(["checkout", "--quiet", &ref_name])
                    .env("MORPH_INTERNAL", "1")
                    .status();
                if let Ok(s) = &status {
                    if !s.success() {
                        return Err(anyhow::anyhow!(
                            "git checkout {} failed (exit {})",
                            ref_name,
                            s.code().unwrap_or(-1)
                        ));
                    }
                }
            }
            let (hash, tree_restored) = morph_core::checkout_tree(&store, &repo_root, &ref_name)?;
            if is_hex_hash {
                println!("Detached HEAD at {}", hash);
            } else {
                println!(
                    "Switched to branch {}",
                    ref_name.trim_start_matches("heads/")
                );
            }
            if tree_restored {
                verbose_msg(verbose, "working tree restored from commit tree");
            }
        }

        Command::Session { sub } => do_session_dispatch(verbose, sub)?,

        Command::Inspect { sub } => inspect::run_inspect(verbose, sub)?,

        Command::Eval { sub } => match sub {
            EvalCmd::Record { file } => {
                let (repo_root, _store) = get_store(verbose)?;
                let full = if file.is_absolute() {
                    file
                } else {
                    repo_root.join(&file)
                };
                let metrics = morph_core::record_eval_metrics(&full)?;
                println!("{}", serde_json::to_string_pretty(&metrics)?);
            }
            EvalCmd::FromOutput {
                runner,
                file,
                record,
            } => {
                let stdout_text = if file.as_os_str() == "-" {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    buf
                } else if file.is_absolute() {
                    std::fs::read_to_string(&file)?
                } else {
                    // Resolve relative paths from the repo root when one
                    // is available, otherwise fall back to the cwd so
                    // the command works outside a repo (handy for one-off
                    // scripts piping CI output).
                    match get_store(verbose) {
                        Ok((repo_root, _store)) => std::fs::read_to_string(repo_root.join(&file))?,
                        Err(_) => std::fs::read_to_string(&file)?,
                    }
                };
                let metrics = morph_core::parse_with_runner(&runner, &stdout_text, None);
                if record {
                    let (repo_root, store) = get_store(verbose)?;
                    let hash = morph_core::record_eval_run(
                        store.as_ref(),
                        &metrics,
                        &runner,
                        None,
                        Some(&stdout_text),
                        None,
                    )?;
                    write_last_run_breadcrumb(&store, &repo_root, &hash);
                    println!("{}", hash);
                } else {
                    println!("{}", serde_json::to_string_pretty(&metrics)?);
                }
            }
            EvalCmd::Add {
                paths,
                suite,
                no_default,
                no_set_default,
            } => {
                do_eval_add(verbose, paths, suite, no_default, no_set_default)?;
            }
            EvalCmd::Rebuild {
                paths,
                no_set_default,
            } => {
                do_eval_rebuild(verbose, paths, no_set_default)?;
            }
            EvalCmd::Show { suite, json } => {
                do_eval_show(verbose, suite, json)?;
            }
            EvalCmd::Gaps { json, fail_on_gap } => {
                let (repo_root, store) = get_store(verbose)?;
                let morph_dir = repo_root.join(".morph");
                let changes = morph_core::working_status(&store, &repo_root)?;
                let gaps = morph_core::compute_eval_gaps(
                    &morph_dir,
                    store.as_ref(),
                    changes.len() as u64,
                )?;
                if json {
                    let body = serde_json::json!({
                        "gaps": gaps,
                        "count": gaps.len(),
                    });
                    println!("{}", serde_json::to_string_pretty(&body)?);
                } else if gaps.is_empty() {
                    println!("No behavioral evidence gaps detected.");
                } else {
                    println!("Found {} gap(s):", gaps.len());
                    for g in &gaps {
                        let kind = g.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
                        let hint = g.get("hint").and_then(|v| v.as_str()).unwrap_or("");
                        println!("  - {}: {}", kind, hint);
                    }
                }
                if fail_on_gap && !gaps.is_empty() {
                    std::process::exit(1);
                }
            }
            EvalCmd::Run {
                runner,
                cwd,
                command,
            } => {
                if command.is_empty() {
                    return Err(anyhow::anyhow!(
                        "no command supplied. Usage: morph eval run -- cargo test --workspace"
                    ));
                }
                let (repo_root, store) = get_store(verbose)?;
                let outcome = morph_core::run_test_command(
                    store.as_ref(),
                    &repo_root,
                    &command,
                    &runner,
                    cwd.as_deref(),
                )?;
                if outcome.metrics.is_empty() {
                    eprintln!(
                        "warning: no metrics extracted from `{}` (runner={}). \
                         Pass `--runner cargo|pytest|vitest|jest|go` explicitly \
                         if auto-detection failed.",
                        command.join(" "),
                        runner,
                    );
                }
                write_last_run_breadcrumb(&store, &repo_root, &outcome.run_hash);
                println!("{}", outcome.run_hash);
                if let Some(code) = outcome.exit_code {
                    if code != 0 {
                        std::process::exit(code);
                    }
                }
            }
        },

        Command::MergePlan { branch, retire } => {
            let (_repo_root, store) = get_store(verbose)?;
            let retired: Option<Vec<String>> =
                retire.map(|s| s.split(',').map(|m| m.trim().to_string()).collect());
            let plan = morph_core::prepare_merge(&store, &branch, None, retired.as_deref())?;
            print!("{}", plan.format_plan());
        }

        Command::Merge {
            branch,
            cont,
            abort,
            message,
            pipeline,
            eval_suite,
            metrics,
            author,
            retire,
            retire_reason,
            sub,
        } => {
            run_merge(
                verbose,
                branch,
                cont,
                abort,
                message,
                pipeline,
                eval_suite,
                metrics,
                author,
                retire,
                retire_reason,
                sub,
            )?;
        }

        Command::Rollup {
            base_ref,
            tip_ref,
            message,
        } => {
            let (_repo_root, store) = get_store(verbose)?;
            println!(
                "{}",
                morph_core::rollup(&store, &base_ref, &tip_ref, message)?
            );
        }

        Command::Remote { sub } => match sub {
            RemoteCmd::Add { name, path } => {
                let (repo_root, _store) = get_store(verbose)?;
                let morph_dir = repo_root.join(".morph");
                // PR5 Stage F: URL-shaped remotes (`ssh://...`,
                // `user@host:path`) are stored verbatim. Plain paths
                // are still resolved to absolute so the remote
                // works from any cwd.
                let raw = path.to_string_lossy().to_string();
                let stored =
                    if morph_core::ssh_store::SshUrl::parse(&raw).is_some() || path.is_absolute() {
                        raw
                    } else {
                        std::env::current_dir()?
                            .join(&path)
                            .to_string_lossy()
                            .to_string()
                    };
                morph_core::add_remote(&morph_dir, &name, &stored)?;
                println!("Remote '{}' added: {}", name, stored);
            }
            RemoteCmd::List { json } => {
                let (repo_root, _store) = get_store(verbose)?;
                let remotes = morph_core::read_remotes(&repo_root.join(".morph"))?;
                if json {
                    let entries: Vec<_> = remotes
                        .iter()
                        .map(|(name, spec)| {
                            serde_json::json!({
                                "name": name,
                                "path": spec.path,
                            })
                        })
                        .collect();
                    let body = serde_json::json!({ "remotes": entries, "count": remotes.len() });
                    println!("{}", serde_json::to_string_pretty(&body)?);
                } else {
                    for (name, spec) in &remotes {
                        println!("{}\t{}", name, spec.path);
                    }
                }
            }
        },

        Command::Push { remote, branch } => {
            let (repo_root, local_store) = get_store(verbose)?;
            let remotes = morph_core::read_remotes(&repo_root.join(".morph"))?;
            let spec = remotes
                .get(&remote)
                .ok_or_else(|| anyhow::anyhow!("remote '{}' not found", remote))?;
            let remote_store = morph_core::open_remote_store(&spec.path)?;
            let tip =
                morph_core::push_branch(local_store.as_ref(), remote_store.as_ref(), &branch)?;
            println!("Pushed {} -> {}/{} ({})", branch, remote, branch, tip);
        }

        Command::Fetch { remote } => {
            let (repo_root, local_store) = get_store(verbose)?;
            let remotes = morph_core::read_remotes(&repo_root.join(".morph"))?;
            let spec = remotes
                .get(&remote)
                .ok_or_else(|| anyhow::anyhow!("remote '{}' not found", remote))?;
            let remote_store = morph_core::open_remote_store(&spec.path)?;
            let updated =
                morph_core::fetch_remote(local_store.as_ref(), remote_store.as_ref(), &remote)?;
            if updated.is_empty() {
                println!("Already up to date.");
            } else {
                for (branch, hash) in &updated {
                    println!("{}/{} -> {}", remote, branch, hash);
                }
            }
        }

        Command::Pull {
            remote,
            branch,
            merge,
        } => {
            let (repo_root, local_store) = get_store(verbose)?;
            let remotes = morph_core::read_remotes(&repo_root.join(".morph"))?;
            let spec = remotes
                .get(&remote)
                .ok_or_else(|| anyhow::anyhow!("remote '{}' not found", remote))?;
            let remote_store = morph_core::open_remote_store(&spec.path)?;
            match morph_core::pull_branch(
                local_store.as_ref(),
                remote_store.as_ref(),
                &remote,
                &branch,
            ) {
                Ok(tip) => {
                    println!("Updated {} -> {} ({})", branch, tip, remote);
                }
                Err(morph_core::MorphError::Diverged {
                    branch: b,
                    local_tip,
                    remote_tip,
                }) if merge => {
                    // Fast-forward not possible. Kick off a structural
                    // merge against the remote-tracking ref so the user
                    // can resolve conflicts locally.
                    eprintln!(
                        "fast-forward not possible (local {} vs remote {}); starting merge",
                        local_tip, remote_tip
                    );
                    let other_ref = format!("remotes/{}/{}", remote, b);
                    let outcome = morph_core::start_merge(
                        local_store.as_ref(),
                        &repo_root,
                        morph_core::StartMergeOpts::new(&other_ref),
                    )?;
                    if outcome.needs_resolution {
                        println!(
                            "Merge needs resolution. Run `morph status` for details, then `morph merge --continue`."
                        );
                        std::process::exit(1);
                    } else {
                        // Auto-finalize the clean three-way merge.
                        let cont = morph_core::continue_merge(
                            local_store.as_ref(),
                            &repo_root,
                            morph_core::ContinueMergeOpts {
                                message: Some(format!(
                                    "Merge remote-tracking branch '{}/{}'",
                                    remote, b
                                )),
                                author: None,
                            },
                        )?;
                        println!("Merged {} -> {} ({})", b, cont.merge_commit, remote);
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }

        Command::Sync { branch } => {
            // PR5 Stage G: bring the configured upstream into the
            // current branch in one step. Equivalent to
            // `morph pull --merge <remote> <branch>` except the
            // remote/branch comes from the per-branch config.
            let (repo_root, local_store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let target = match branch {
                Some(n) => n,
                None => morph_core::current_branch(&local_store)?.ok_or_else(|| {
                    anyhow::anyhow!("no current branch (detached HEAD?); name a branch explicitly")
                })?,
            };
            let upstream = morph_core::get_branch_upstream(&morph_dir, &target)?
                .ok_or_else(|| anyhow::anyhow!(
                    "no upstream configured for '{}'; run `morph branch --set-upstream <remote>/<branch>`",
                    target
                ))?;
            let remotes = morph_core::read_remotes(&morph_dir)?;
            let spec = remotes.get(&upstream.remote).ok_or_else(|| {
                anyhow::anyhow!(
                    "upstream remote '{}' not found in remotes config",
                    upstream.remote
                )
            })?;
            let remote_store = morph_core::open_remote_store(&spec.path)?;
            match morph_core::pull_branch(
                local_store.as_ref(),
                remote_store.as_ref(),
                &upstream.remote,
                &upstream.branch,
            ) {
                Ok(tip) => {
                    println!(
                        "Synced {} -> {} ({}/{})",
                        target, tip, upstream.remote, upstream.branch
                    );
                }
                Err(morph_core::MorphError::Diverged {
                    branch: b,
                    local_tip,
                    remote_tip,
                }) => {
                    eprintln!(
                        "fast-forward not possible (local {} vs remote {}); starting merge",
                        local_tip, remote_tip
                    );
                    let other_ref = format!("remotes/{}/{}", upstream.remote, b);
                    let outcome = morph_core::start_merge(
                        local_store.as_ref(),
                        &repo_root,
                        morph_core::StartMergeOpts::new(&other_ref),
                    )?;
                    if outcome.needs_resolution {
                        println!(
                            "Merge needs resolution. Run `morph status` for details, then `morph merge --continue`."
                        );
                        std::process::exit(1);
                    } else {
                        let cont = morph_core::continue_merge(
                            local_store.as_ref(),
                            &repo_root,
                            morph_core::ContinueMergeOpts {
                                message: Some(format!(
                                    "Merge remote-tracking branch '{}/{}'",
                                    upstream.remote, b
                                )),
                                author: None,
                            },
                        )?;
                        println!(
                            "Merged {} -> {} ({}/{})",
                            b, cont.merge_commit, upstream.remote, upstream.branch
                        );
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }

        Command::Refs { json } => {
            let (_repo_root, store) = get_store(verbose)?;
            let refs = morph_core::list_refs(store.as_ref())?;
            if json {
                let entries: Vec<_> = refs
                    .iter()
                    .map(|(name, hash)| {
                        let h_str = hash.to_string();
                        serde_json::json!({
                            "name": name,
                            "hash": h_str,
                            "short": short_hash_str(&h_str),
                        })
                    })
                    .collect();
                let body = serde_json::json!({ "refs": entries, "count": refs.len() });
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                for (name, hash) in refs {
                    println!("{}\t{}", hash, name);
                }
            }
        }

        Command::Config { key, value, get } => {
            // `morph config` exposes a small, explicit set of keys
            // rather than a generic JSON tree. Each key has its own
            // typed reader/writer in `morph-core` so the on-disk
            // shape stays consistent across versions. Unknown keys
            // error out with the supported list rather than silently
            // writing.
            let (repo_root, _) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let getting = get || value.is_none();
            match key.as_str() {
                "user.name" => {
                    let (cfg_name, _) = morph_core::read_identity_config(&morph_dir)?;
                    if getting {
                        match cfg_name {
                            Some(v) => println!("{}", v),
                            None => std::process::exit(1),
                        }
                    } else {
                        morph_core::write_identity_config(&morph_dir, value.as_deref(), None)?;
                    }
                }
                "user.email" => {
                    let (_, cfg_email) = morph_core::read_identity_config(&morph_dir)?;
                    if getting {
                        match cfg_email {
                            Some(v) => println!("{}", v),
                            None => std::process::exit(1),
                        }
                    } else {
                        morph_core::write_identity_config(&morph_dir, None, value.as_deref())?;
                    }
                }
                "commit.test_command" => {
                    if getting {
                        match morph_core::read_commit_test_command(&morph_dir)? {
                            Some(v) => println!("{}", v),
                            None => std::process::exit(1),
                        }
                    } else {
                        morph_core::write_commit_test_command(
                            &morph_dir,
                            value.as_deref().unwrap_or(""),
                        )?;
                    }
                }
                other => {
                    return Err(anyhow::anyhow!(
                        "unsupported config key '{}'. Supported keys: user.name, user.email, commit.test_command",
                        other
                    ));
                }
            }
        }

        Command::Certify {
            metrics,
            metrics_file,
            commit,
            eval_suite,
            runner,
            author,
            json,
        } => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let commit_hash = match commit {
                Some(ref h) => resolve_obj_hash(store.as_ref(), h)?,
                None => morph_core::resolve_head(&store)?
                    .ok_or_else(|| anyhow::anyhow!("no HEAD commit; specify --commit"))?,
            };
            // PR 1: `--metrics` (inline JSON) and `--metrics-file`
            // (file path) are mutually exclusive. clap enforces the
            // conflict via `conflicts_with`; here we just pick which
            // source to read from. At least one must be provided.
            let metrics: std::collections::BTreeMap<String, f64> = match (metrics, metrics_file) {
                (Some(s), None) => serde_json::from_str(&s).map_err(|e| {
                    anyhow::anyhow!("--metrics is not a JSON object of metric → number: {}", e)
                })?,
                (None, Some(path)) => {
                    let full_path = if path.is_absolute() {
                        path
                    } else {
                        repo_root.join(&path)
                    };
                    serde_json::from_str(&std::fs::read_to_string(&full_path)?)?
                }
                (Some(_), Some(_)) => unreachable!("clap conflicts_with prevents this"),
                (None, None) => {
                    return Err(anyhow::anyhow!(
                        "morph certify requires --metrics '<json>' or --metrics-file <path>"
                    ));
                }
            };
            let result = morph_core::certify_commit(
                &store,
                &morph_dir,
                &commit_hash,
                &metrics,
                runner.as_deref().or(author.as_deref()),
                eval_suite.as_deref(),
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else if result.passed {
                println!("PASS: commit {} certified", commit_hash);
                for (k, v) in &result.metrics_provided {
                    println!("  {} = {}", k, v);
                }
            } else {
                eprintln!("FAIL: commit {} not certified", commit_hash);
                for f in &result.failures {
                    eprintln!("  {}", f);
                }
                std::process::exit(1);
            }
        }

        Command::Gate { commit, json } => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let commit_hash = match commit {
                Some(ref h) => resolve_obj_hash(store.as_ref(), h)?,
                None => morph_core::resolve_head(&store)?
                    .ok_or_else(|| anyhow::anyhow!("no HEAD commit; specify --commit"))?,
            };
            let result = morph_core::gate_check(&store, &morph_dir, &commit_hash)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
                if !result.passed {
                    std::process::exit(1);
                }
            } else if result.passed {
                println!("PASS: commit {} satisfies policy", commit_hash);
            } else {
                eprintln!("FAIL: commit {} does not satisfy policy", commit_hash);
                for r in &result.reasons {
                    eprintln!("  {}", r);
                }
                std::process::exit(1);
            }
        }

        Command::Policy { sub } => match sub {
            PolicyCmd::Init { force } => {
                let (repo_root, _store) = get_store(verbose)?;
                let morph_dir = repo_root.join(".morph");
                let existing = morph_core::read_policy(&morph_dir)?;
                let already_set = !existing.required_metrics.is_empty()
                    || !existing.thresholds.is_empty()
                    || existing.default_eval_suite.is_some();
                if already_set && !force {
                    println!("Policy already initialized; pass --force to overwrite");
                } else {
                    let default = morph_core::RepoPolicy {
                        required_metrics: vec![
                            "tests_total".to_string(),
                            "tests_passed".to_string(),
                        ],
                        default_eval_suite: existing.default_eval_suite,
                        ..Default::default()
                    };
                    morph_core::write_policy(&morph_dir, &default)?;
                    println!("Policy initialized: required_metrics=[tests_total, tests_passed]");
                }
            }
            PolicyCmd::Show => {
                let (repo_root, _store) = get_store(verbose)?;
                let policy = morph_core::read_policy(&repo_root.join(".morph"))?;
                println!("{}", serde_json::to_string_pretty(&policy)?);
            }
            PolicyCmd::Set { file } => {
                let (repo_root, _store) = get_store(verbose)?;
                let morph_dir = repo_root.join(".morph");
                let full = if file.is_absolute() {
                    file
                } else {
                    repo_root.join(&file)
                };
                let policy: morph_core::RepoPolicy =
                    serde_json::from_str(&std::fs::read_to_string(&full)?)?;
                morph_core::write_policy(&morph_dir, &policy)?;
                println!("Policy updated");
            }
            PolicyCmd::SetDefaultEval { hash } => {
                let (repo_root, store) = get_store(verbose)?;
                let morph_dir = repo_root.join(".morph");
                let resolved = resolve_obj_hash(store.as_ref(), &hash)?.to_string();
                let mut policy = morph_core::read_policy(&morph_dir)?;
                policy.default_eval_suite = Some(resolved.clone());
                morph_core::write_policy(&morph_dir, &policy)?;
                println!("Default eval suite set to {}", resolved);
            }
            PolicyCmd::RequireMetrics { metrics } => {
                let (repo_root, _store) = get_store(verbose)?;
                let morph_dir = repo_root.join(".morph");
                let mut policy = morph_core::read_policy(&morph_dir)?;
                policy.required_metrics = metrics.clone();
                morph_core::write_policy(&morph_dir, &policy)?;
                if metrics.is_empty() {
                    println!("Cleared required_metrics");
                } else {
                    println!("required_metrics = [{}]", metrics.join(", "));
                }
            }
        },

        Command::Annotate {
            target_hash,
            kind,
            data,
            sub,
            author,
        } => {
            let (_repo_root, store) = get_store(verbose)?;
            let target = resolve_obj_hash(store.as_ref(), &target_hash)?;
            let data_map: std::collections::BTreeMap<String, serde_json::Value> =
                serde_json::from_str(&data)
                    .map_err(|e| anyhow::anyhow!("invalid --data JSON: {}", e))?;
            let ann = morph_core::create_annotation(&target, sub, kind, data_map, author);
            println!("{}", store.put(&ann)?);
        }

        Command::Annotations {
            target_hash,
            sub,
            json,
        } => {
            let (_repo_root, store) = get_store(verbose)?;
            let target = resolve_obj_hash(store.as_ref(), &target_hash)?;
            let anns = morph_core::list_annotations(&store, &target, sub.as_deref())?;
            if json {
                let entries: Vec<_> = anns
                    .iter()
                    .map(|(h, a)| {
                        let h_str = h.to_string();
                        serde_json::json!({
                            "hash": h_str,
                            "short": short_hash_str(&h_str),
                            "kind": a.kind,
                            "author": a.author,
                            "target": a.target,
                            "target_sub": a.target_sub,
                            "data": a.data,
                        })
                    })
                    .collect();
                let body = serde_json::json!({
                    "target": target.to_string(),
                    "target_short": target.short(),
                    "annotations": entries,
                    "count": anns.len(),
                });
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                for (h, a) in &anns {
                    println!(
                        "{} {} {} {}",
                        h,
                        a.kind,
                        a.author,
                        serde_json::to_string(&a.data).unwrap_or_default()
                    );
                }
            }
        }

        Command::HashObject { path } => {
            let (repo_root, store) = get_store(verbose)?;
            let full = if path.is_absolute() {
                path
            } else {
                repo_root.join(&path)
            };
            let json = std::fs::read_to_string(&full)?;
            let obj: MorphObject = serde_json::from_str(&json)
                .map_err(|e| anyhow::anyhow!("invalid Morph object JSON: {}", e))?;
            println!("{}", store.put(&obj)?);
        }
    }
    Ok(())
}

fn cmd_prompt_show(verbose: bool, run_ref: &str, run_upgrade: bool) -> anyhow::Result<()> {
    let mut upgraded = false;
    loop {
        let (repo_root, store) = get_store(verbose)?;
        let morph_dir = repo_root.join(".morph");
        let runs_dir = morph_dir.join("runs");
        if !runs_dir.is_dir() {
            anyhow::bail!("no runs yet (missing or empty .morph/runs/)");
        }
        let mut run_files: Vec<_> = std::fs::read_dir(&runs_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
            .collect();
        run_files.sort_by(|a, b| {
            let at = a.metadata().and_then(|m| m.modified()).ok();
            let bt = b.metadata().and_then(|m| m.modified()).ok();
            bt.cmp(&at)
        });

        let run_path = if run_ref == "latest" || run_ref.is_empty() {
            run_files
                .first()
                .ok_or_else(|| anyhow::anyhow!("no runs in .morph/runs/"))?
                .path()
        } else if let Some(n_str) = run_ref
            .strip_prefix("latest~")
            .or_else(|| run_ref.strip_prefix("latest-"))
        {
            let n: usize = n_str
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid ref '{}': expected latest~N", run_ref))?;
            run_files
                .get(n)
                .ok_or_else(|| {
                    anyhow::anyhow!("no run at index {} (only {} run(s))", n, run_files.len())
                })?
                .path()
        } else if run_ref.len() == 64 && run_ref.chars().all(|c| c.is_ascii_hexdigit()) {
            let path = runs_dir.join(format!("{}.json", run_ref));
            if !path.exists() {
                anyhow::bail!("run not found: {}", run_ref);
            }
            path
        } else {
            anyhow::bail!(
                "invalid ref '{}': use 'latest', 'latest~N', or a 64-char run hash",
                run_ref
            );
        };

        let run_json = std::fs::read_to_string(&run_path)?;
        let run: morph_core::objects::Run = serde_json::from_str(&run_json)?;
        let trace_hash = parse_hash(&run.trace)?;

        match store.get(&trace_hash) {
            Ok(MorphObject::Trace(t)) => {
                let text = t
                    .events
                    .iter()
                    .rfind(|e| e.kind == "prompt" || e.kind == "user")
                    .and_then(|e| e.payload.get("text").and_then(|v| v.as_str()))
                    .unwrap_or("");
                print!("{}", text);
                return Ok(());
            }
            Ok(_) => anyhow::bail!("object {} is not a trace", run.trace),
            Err(_) => {
                let trace_path = morph_dir.join("traces").join(format!("{}.json", run.trace));
                if trace_path.exists() {
                    let obj: MorphObject =
                        serde_json::from_str(&std::fs::read_to_string(&trace_path)?)?;
                    if let MorphObject::Trace(t) = obj {
                        let text = t
                            .events
                            .iter()
                            .rfind(|e| e.kind == "prompt" || e.kind == "user")
                            .and_then(|e| e.payload.get("text").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        print!("{}", text);
                        return Ok(());
                    }
                }
                if run_upgrade && !upgraded {
                    let version = read_repo_version(&morph_dir)?;
                    if version == STORE_VERSION_0_4 || version == STORE_VERSION_0_3 {
                        eprintln!("Store already at {}.", version);
                    } else if version == STORE_VERSION_0_2 {
                        migrate_0_2_to_0_3(&morph_dir)?;
                        eprintln!("Ran migrate 0.2 → 0.3. Retrying...");
                    } else if version == STORE_VERSION_INIT {
                        migrate_0_0_to_0_2(&morph_dir)?;
                        migrate_0_2_to_0_3(&morph_dir)?;
                        eprintln!("Ran migrate 0.0 → 0.3. Retrying...");
                    }
                    upgraded = true;
                    continue;
                }
                let run_hash = run_path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
                anyhow::bail!(
                    "trace not found: {} (run {}). Run 'morph upgrade' and retry, or pass --run-upgrade.",
                    run.trace, run_hash
                );
            }
        }
    }
}

/// Resolve a user-supplied hash to a Run hash. Accepts either a Run
/// hash directly or a Trace hash (in which case we locate the
/// latest Run pointing at that trace). Thin wrapper over
/// `morph_core::resolve_run_or_trace_hash` that adds CLI-side
/// hash-prefix resolution and converts errors to `anyhow`.
pub(crate) fn resolve_run_hash(store: &dyn Store, hash_str: &str) -> anyhow::Result<Hash> {
    let h = resolve_obj_hash(store, hash_str)?;
    morph_core::resolve_run_or_trace_hash(store, &h).map_err(|e| anyhow::anyhow!("{}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PR 8: `default_clone_dest` mirrors `git clone`'s
    /// directory-naming heuristic: take the last URL segment, strip
    /// a trailing `.morph` if present.
    #[test]
    fn default_clone_dest_strips_morph_suffix() {
        assert_eq!(
            default_clone_dest("you@host:repos/myproject.morph"),
            "myproject"
        );
        assert_eq!(default_clone_dest("ssh://you@host/srv/proj.morph"), "proj");
        assert_eq!(default_clone_dest("/tmp/foo.morph"), "foo");
        assert_eq!(default_clone_dest("/tmp/bar/"), "bar");
        assert_eq!(default_clone_dest("plain"), "plain");
        assert_eq!(default_clone_dest(""), "morph-clone");
    }

    /// IPv6 SSH URLs land us in the bracketed-host branch of
    /// `SshUrl::parse`; we still want a sensible basename.
    #[test]
    fn default_clone_dest_handles_ipv6_ssh_url() {
        assert_eq!(
            default_clone_dest("ssh://you@[::1]:2222/srv/repo.morph"),
            "repo"
        );
        assert_eq!(default_clone_dest("ssh://[::1]/srv/proj"), "proj");
    }

    #[test]
    fn short_hash_str_truncates_to_eight_chars() {
        assert_eq!(
            short_hash_str("abcdef0123456789abcdef0123456789"),
            "abcdef01"
        );
        assert_eq!(short_hash_str("abc"), "abc");
        assert_eq!(short_hash_str(""), "");
    }

    /// PR 10: `morph version --json` is the documented machine-
    /// readable handshake for release pipelines and downstream
    /// tooling. The shape is stable (additive only) and pinned by
    /// this test — adding fields is fine, removing or renaming is
    /// a breaking change.
    #[test]
    fn version_json_has_stable_field_set() {
        let body = version_json();
        let value: serde_json::Value =
            serde_json::from_str(&body).expect("version_json must emit valid JSON");
        assert_eq!(value["name"], "morph");
        assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(value["build_date"], env!("MORPH_BUILD_DATE"));
        assert!(
            value["protocol_version"].is_number(),
            "protocol_version must be a number, got: {}",
            value["protocol_version"]
        );
        let supported = value["supported_repo_versions"]
            .as_array()
            .expect("supported_repo_versions must be an array");
        assert!(
            supported.iter().any(|v| v == "0.5"),
            "current schema 0.5 must be in supported_repo_versions: {:?}",
            supported
        );
        assert!(
            supported.iter().any(|v| v == "0.0"),
            "the legacy 0.0 schema must remain supported: {:?}",
            supported
        );
    }
}
