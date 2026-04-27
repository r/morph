//! Morph CLI: read path and manual write operations.

mod cli;
mod remote_helper;
#[cfg(feature = "cursor-setup")]
mod setup;

use clap::Parser;
use cli::*;
use morph_core::{
    find_repo, migrate_0_0_to_0_2, migrate_0_2_to_0_3, migrate_0_3_to_0_4,
    migrate_0_4_to_0_5, open_store,
    read_repo_version, require_store_version, resolve_revision, Hash, MorphObject, ObjectType,
    Store, STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_0_4, STORE_VERSION_0_5, STORE_VERSION_INIT,
};
use std::path::PathBuf;

fn get_store(verbose: bool) -> anyhow::Result<(PathBuf, Box<dyn Store>)> {
    let cwd = std::env::current_dir()?;
    let repo_root = find_repo(&cwd)
        .ok_or_else(|| anyhow::anyhow!("not a morph repository (or any parent)"))?;
    let morph_dir = repo_root.join(".morph");
    let version = read_repo_version(&morph_dir)?;
    verbose_msg(verbose, &format!("repo {} (store version {})", repo_root.display(), version));
    require_store_version(&morph_dir, &[STORE_VERSION_INIT, STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_0_4, STORE_VERSION_0_5])?;
    let store = open_store(&morph_dir)?;
    Ok((repo_root, store))
}

fn parse_hash(s: &str) -> anyhow::Result<Hash> {
    Hash::from_hex(s).map_err(|e| anyhow::anyhow!("invalid hash: {}", e))
}

/// Resolve a user-supplied identifier (hash, ref, prefix) against the
/// store. Delegates to [`morph_core::resolve_revision`] so HEAD,
/// branches, tags, and short prefixes all work uniformly.
fn resolve_obj_hash(store: &dyn Store, s: &str) -> anyhow::Result<Hash> {
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
fn default_clone_dest(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let after_colon = trimmed.rsplit_once(':').map(|(_, p)| p).unwrap_or(trimmed);
    let after_slash = after_colon.rsplit('/').next().unwrap_or(after_colon);
    let base = after_slash.trim_end_matches(".morph");
    if base.is_empty() {
        "morph-clone".to_string()
    } else {
        base.to_string()
    }
}

/// Truncate a hex hash to its 8-character prefix for display.
fn short_hash(h: &str) -> String {
    h.chars().take(8).collect()
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

/// Structured version output for `morph version --json`. Stable
/// shape (additive only): release pipelines and downstream tooling
/// rely on the field names below to verify the binary's identity
/// without parsing the human-readable line.
/// Compact, stable string label for a `MorphObject` variant. Used by
/// `morph identify`, `morph head`, and JSON envelopes that need to
/// surface object kinds without leaking the internal serde tag layout.
fn morph_object_type_str(obj: &MorphObject) -> &'static str {
    match obj {
        MorphObject::Blob(_) => "blob",
        MorphObject::Tree(_) => "tree",
        MorphObject::Pipeline(_) => "pipeline",
        MorphObject::EvalSuite(_) => "eval_suite",
        MorphObject::Commit(_) => "commit",
        MorphObject::Run(_) => "run",
        MorphObject::Artifact(_) => "artifact",
        MorphObject::Trace(_) => "trace",
        MorphObject::TraceRollup(_) => "trace_rollup",
        MorphObject::Annotation(_) => "annotation",
    }
}

/// Build the JSON envelope returned by `morph status --json`. Stable
/// shape: agents pin field names like `branch`, `head`, `working_tree`,
/// `staging`, and `eval_suite`. Additive only.

fn version_json() -> String {
    let supported: Vec<&str> = vec![
        STORE_VERSION_INIT,
        STORE_VERSION_0_2,
        STORE_VERSION_0_3,
        STORE_VERSION_0_4,
        STORE_VERSION_0_5,
    ];
    let value = serde_json::json!({
        "name": "morph",
        "version": env!("CARGO_PKG_VERSION"),
        "build_date": env!("MORPH_BUILD_DATE"),
        "protocol_version": morph_core::ssh_proto::MORPH_PROTOCOL_VERSION,
        "supported_repo_versions": supported,
    });
    serde_json::to_string(&value).expect("version json serializes")
}

fn print_tap_task(task: &morph_core::TapTask) {
    println!("=== Run {} ===", &task.run_hash[..12]);
    println!("  model: {}  agent: {}  events: {}  steps: {}",
        task.model, task.agent, task.event_count, task.step_count);
    for (i, step) in task.steps.iter().enumerate() {
        println!("\n  --- Step {} ---", i + 1);
        let prompt_preview = if step.prompt.len() > 120 {
            format!("{}...", &step.prompt[..step.prompt.floor_char_boundary(120)])
        } else {
            step.prompt.clone()
        };
        println!("  Prompt: {}", prompt_preview);
        if !step.tool_calls.is_empty() {
            println!("  Tool calls: {}", step.tool_calls.len());
            for tc in &step.tool_calls {
                println!("    - {}{}", tc.name.as_deref().unwrap_or("(unnamed)"),
                    if tc.output.is_some() { " [has output]" } else { "" });
            }
        }
        if !step.file_reads.is_empty() {
            println!("  File reads: {}", step.file_reads.len());
        }
        if !step.file_edits.is_empty() {
            println!("  File edits: {}", step.file_edits.len());
        }
        let resp_preview = if step.response.len() > 200 {
            format!("{}...", &step.response[..step.response.floor_char_boundary(200)])
        } else {
            step.response.clone()
        };
        if resp_preview.is_empty() {
            println!("  Response: (empty)");
        } else {
            println!("  Response: {}", resp_preview);
        }
    }
    println!();
}

fn print_trace_events(trace: &morph_core::objects::Trace) {
    for ev in &trace.events {
        let text = ev.payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
        match ev.kind.as_str() {
            "prompt" | "user" => println!("--- prompt ---\n{}", text),
            "response" | "assistant" => println!("--- response ---\n{}", text),
            "tool_call" | "tool_use" | "function_call" => {
                let name = ev.payload.get("name").and_then(|v| v.as_str()).unwrap_or("(unnamed)");
                println!("--- tool_call: {} ---\n{}", name, text);
            }
            "tool_result" | "tool_output" | "function_result" => {
                let output = ev.payload.get("output").and_then(|v| v.as_str()).unwrap_or(text);
                let err = ev.payload.get("error").and_then(|v| v.as_str());
                println!("--- tool_result ---\n{}", output);
                if let Some(e) = err { println!("  error: {}", e); }
            }
            "file_read" | "read_file" => {
                let path = ev.payload.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                println!("--- file_read: {} ---", path);
            }
            "file_edit" | "edit_file" | "write_file" | "file_write" => {
                let path = ev.payload.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                println!("--- file_edit: {} ---", path);
            }
            _ => println!("--- {} ---\n{}", ev.kind, text),
        }
    }
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

    if abort {
        morph_core::abort_merge(store.as_ref(), &repo_root)?;
        println!("Merge aborted; working tree restored to ORIG_HEAD.");
        return Ok(());
    }

    if cont {
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

    let single_shot = pipeline.is_some() && metrics.is_some() && message.is_some();
    if single_shot {
        let version = read_repo_version(&morph_dir)?;
        let prog_hash = resolve_obj_hash(store.as_ref(), pipeline.as_deref().unwrap())?;
        let suite_hash_opt = eval_suite
            .as_deref()
            .map(|s| resolve_obj_hash(store.as_ref(), s))
            .transpose()?;
        let observed: std::collections::BTreeMap<String, f64> = serde_json::from_str(
            metrics.as_deref().unwrap(),
        )
        .map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?;
        let retired: Option<Vec<String>> = retire
            .map(|s| s.split(',').map(|m| m.trim().to_string()).collect());
        let mut plan = morph_core::prepare_merge(
            store.as_ref(),
            &branch,
            suite_hash_opt.as_ref(),
            retired.as_deref(),
        )?;
        plan.retire_reason = retire_reason;
        let resolved_author =
            morph_core::resolve_author_for_repo(&morph_dir, author.as_deref())?;
        let hash = morph_core::execute_merge(
            store.as_ref(),
            &plan,
            &prog_hash,
            observed,
            message.unwrap(),
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
                if outcome.textual_conflicts.len() == 1 { "" } else { "s" }
            );
            for p in &outcome.textual_conflicts {
                println!("  CONFLICT (content): {}", p);
            }
        }
        if !outcome.pipeline_node_conflicts.is_empty() {
            println!(
                "Pipeline has {} node-level conflict{}:",
                outcome.pipeline_node_conflicts.len(),
                if outcome.pipeline_node_conflicts.len() == 1 { "" } else { "s" }
            );
            for c in &outcome.pipeline_node_conflicts {
                println!("  CONFLICT (pipeline node): {}", c.id);
            }
            println!(
                "  resolve with: morph merge resolve-node <id> --pick ours|theirs|base"
            );
        }
        println!("Run `morph status` for details, then `morph merge --continue`.");
        std::process::exit(1);
    }

    if matches!(outcome.trivial, morph_core::TrivialOutcome::AlreadyMerged | morph_core::TrivialOutcome::AlreadyAhead) {
        println!("Already up to date.");
        return Ok(());
    }

    if matches!(outcome.trivial, morph_core::TrivialOutcome::FastForward) {
        let branch_ref = morph_core::current_branch(store.as_ref())?
            .unwrap_or_else(|| "main".to_string());
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

        Command::Init { path, bare, reference, no_default_policy } => {
            verbose_msg(
                verbose,
                &format!(
                    "initializing {} repo at {}",
                    if bare {
                        "bare"
                    } else if reference {
                        "reference-mode"
                    } else {
                        "working"
                    },
                    path.display()
                ),
            );
            if reference {
                if !morph_core::is_git_working_tree(&path) {
                    eprintln!(
                        "morph init --reference: {} is not a git repository (no .git directory found). \
Run `git init` first or pass a path that is already a git working tree.",
                        path.display()
                    );
                    std::process::exit(1);
                }
                morph_core::init_repo(&path)?;
                let init_at = morph_core::git_head_sha(&path)?;
                let morph_dir = path.join(".morph");
                morph_core::write_reference_mode(&morph_dir, init_at.as_deref())?;
                let hooks_dir = path.join(".git").join("hooks");
                std::fs::create_dir_all(&hooks_dir)?;
                let hook_path = hooks_dir.join("post-commit");
                std::fs::write(&hook_path, morph_core::POST_COMMIT_HOOK_SCRIPT)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = std::fs::metadata(&hook_path)?.permissions();
                    perms.set_mode(0o755);
                    std::fs::set_permissions(&hook_path, perms)?;
                }
                let abs_morph = path
                    .canonicalize()
                    .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&path))
                    .join(".morph");
                println!(
                    "Initialized Morph repository in reference mode in {}/",
                    abs_morph.display()
                );
                if let Some(sha) = init_at.as_deref() {
                    println!("  bound to git HEAD {}", &sha[..sha.len().min(12)]);
                } else {
                    println!("  bound to empty git repository (no commits yet)");
                }
                println!("  installed post-commit hook at .git/hooks/post-commit");
            } else if bare {
                morph_core::init_bare(&path)?;
                let abs = path
                    .canonicalize()
                    .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&path));
                println!("Initialized bare Morph repository in {}/", abs.display());
            } else {
                morph_core::init_repo(&path)?;
                let abs_morph = path
                    .canonicalize()
                    .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&path))
                    .join(".morph");
                println!("Initialized empty Morph repository in {}/", abs_morph.display());
            }
            // Phase 2a: clear the default policy when the test harness
            // asks for a permissive repo. Production users never pass
            // this flag; spec fixtures from before Phase 2a do.
            if no_default_policy {
                let morph_dir = if bare {
                    path.clone()
                } else {
                    path.join(".morph")
                };
                let permissive = morph_core::RepoPolicy::default();
                morph_core::write_policy(&morph_dir, &permissive)?;
            }
        }

        Command::ReferenceSync => {
            let cwd = std::env::current_dir()?;
            let repo_root = morph_core::find_repo(&cwd)
                .ok_or_else(|| anyhow::anyhow!("not in a morph repository"))?;
            let morph_dir = repo_root.join(".morph");
            let mode = morph_core::read_repo_mode(&morph_dir)?;
            if mode != morph_core::RepoMode::Reference {
                eprintln!(
                    "morph reference-sync: not in reference mode. Run `morph init --reference` \
in a git repository first."
                );
                std::process::exit(1);
            }
            let store = morph_core::open_store(&morph_dir)?;
            let outcome = morph_core::sync_to_head(
                store.as_ref(),
                &repo_root,
                Some(env!("CARGO_PKG_VERSION")),
            )?;
            if outcome.already_synced {
                println!("Already up to date.");
            } else {
                let short = outcome
                    .git_sha
                    .as_deref()
                    .map(|s| &s[..s.len().min(8)])
                    .unwrap_or("?");
                println!("Synced 1 git commit ({}).", short);
            }
        }

        Command::Clone { url, destination, branch, bare } => {
            let dest = destination.unwrap_or_else(|| std::path::PathBuf::from(default_clone_dest(&url)));
            verbose_msg(
                verbose,
                &format!(
                    "cloning {} -> {} ({} clone)",
                    url,
                    dest.display(),
                    if bare { "bare" } else { "working" }
                ),
            );
            let outcome = morph_core::clone_repo(
                &url,
                &dest,
                morph_core::CloneOpts { branch, bare },
            )?;
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
            println!(
                "  branch:  {} ({})",
                outcome.branch,
                short_hash(&outcome.tip.to_string())
            );
            println!("  fetched: {} branch(es)", outcome.fetched.len());
        }

        #[cfg(feature = "cursor-setup")]
        Command::Setup { sub } => match sub {
            SetupCmd::Cursor { path } => {
                let root = std::path::Path::new(&path).canonicalize().unwrap_or_else(|_| path.clone());
                verbose_msg(verbose, &format!("setting up Cursor integration at {}", root.display()));
                let report = setup::setup_cursor(&root)?;
                println!("Cursor integration installed in {}", root.display());
                println!("  Hook scripts: {}", report.hooks_written.join(", "));
                println!("  Rules: {}", report.rules_written.join(", "));
                println!("  .cursor/hooks.json: {}", if report.hooks_json_updated { "updated" } else { "unchanged" });
                println!("  .cursor/mcp.json: {}", if report.mcp_json_updated { "updated" } else { "unchanged" });
            }
            SetupCmd::Opencode { path } => {
                let root = std::path::Path::new(&path).canonicalize().unwrap_or_else(|_| path.clone());
                verbose_msg(verbose, &format!("setting up OpenCode integration at {}", root.display()));
                let report = setup::setup_opencode(&root)?;
                println!("OpenCode integration installed in {}", root.display());
                println!("  opencode.json: {}", if report.opencode_json_updated { "updated" } else { "unchanged" });
                println!("  AGENTS.md: {}", if report.agents_md_written { "written" } else { "unchanged" });
                println!("  .opencode/plugins/morph-record.ts: {}", if report.plugin_written { "written" } else { "unchanged" });
            }
        },

        Command::Upgrade => {
            let cwd = std::env::current_dir()?;
            let repo_root = find_repo(&cwd)
                .ok_or_else(|| anyhow::anyhow!("not a morph repository (or any parent)"))?;
            let morph_dir = repo_root.join(".morph");
            let version = read_repo_version(&morph_dir)?;
            verbose_msg(verbose, &format!("store version {}", version));
            if version == STORE_VERSION_0_5 {
                println!("Store version is {} (latest). No upgrade needed.", version);
            } else if version == STORE_VERSION_0_4 {
                migrate_0_4_to_0_5(&morph_dir)?;
                println!("Migrated store from {} to {} (merge state).", STORE_VERSION_0_4, STORE_VERSION_0_5);
            } else if version == STORE_VERSION_0_3 {
                migrate_0_3_to_0_4(&morph_dir)?;
                migrate_0_4_to_0_5(&morph_dir)?;
                println!("Migrated store from {} to {}.", STORE_VERSION_0_3, STORE_VERSION_0_5);
            } else if version == STORE_VERSION_0_2 {
                migrate_0_2_to_0_3(&morph_dir)?;
                migrate_0_3_to_0_4(&morph_dir)?;
                migrate_0_4_to_0_5(&morph_dir)?;
                println!("Migrated store from {} to {}.", STORE_VERSION_0_2, STORE_VERSION_0_5);
            } else if version == STORE_VERSION_INIT {
                migrate_0_0_to_0_2(&morph_dir)?;
                migrate_0_2_to_0_3(&morph_dir)?;
                migrate_0_3_to_0_4(&morph_dir)?;
                migrate_0_4_to_0_5(&morph_dir)?;
                println!("Migrated store from {} to {}.", STORE_VERSION_INIT, STORE_VERSION_0_5);
            } else {
                println!("Store version is {}. No upgrade path.", version);
            }
        }

        Command::RemoteHelper { repo_root } => {
            // The helper intentionally bypasses `get_store` so it
            // can produce a clear "not a morph repository" message
            // for the SSH client instead of inheriting the CLI's
            // discover-via-cwd behavior.
            remote_helper::run(&repo_root)?;
        }

        Command::Gc => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let fs_store = morph_core::FsStore::from_store_version(&morph_dir)?;
            let result = morph_core::gc(&fs_store, &morph_dir)?;
            if result.objects_removed == 0 {
                println!("Nothing to clean up. {} objects total.", result.objects_before);
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
        Command::Visualize { path, port, interface } => {
            let repo_root = path.canonicalize().unwrap_or(path);
            let morph_dir = if repo_root.join(".morph").exists() {
                repo_root.join(".morph")
            } else {
                find_repo(&repo_root)
                    .ok_or_else(|| anyhow::anyhow!("not a morph repository"))?
                    .join(".morph")
            };
            let addr = format!("{}:{}", interface, port).parse()
                .map_err(|_| anyhow::anyhow!("invalid address: {}:{}", interface, port))?;
            morph_serve::run_blocking(morph_dir, addr).map_err(|e| anyhow::anyhow!("{}", e))?;
        }

        #[cfg(feature = "visualize")]
        Command::Serve { repos, port, interface, org_policy } => {
            let repo_entries = if repos.is_empty() {
                let cwd = std::env::current_dir()?;
                let repo_root = find_repo(&cwd)
                    .ok_or_else(|| anyhow::anyhow!("not a morph repository; specify --repo name=path"))?;
                vec![morph_serve::RepoEntry { name: "default".into(), morph_dir: repo_root.join(".morph") }]
            } else {
                repos.iter().map(|spec| {
                    let (name, path_str) = spec.split_once('=')
                        .ok_or_else(|| anyhow::anyhow!("repo spec must be name=path, got: {}", spec))?;
                    let path = PathBuf::from(path_str);
                    let morph_dir = if path.join(".morph").exists() {
                        path.join(".morph")
                    } else {
                        find_repo(&path)
                            .ok_or_else(|| anyhow::anyhow!("not a morph repository: {}", path_str))?
                            .join(".morph")
                    };
                    Ok(morph_serve::RepoEntry { name: name.to_string(), morph_dir })
                }).collect::<anyhow::Result<Vec<_>>>()?
            };
            let addr: std::net::SocketAddr = format!("{}:{}", interface, port).parse()
                .map_err(|_| anyhow::anyhow!("invalid address: {}:{}", interface, port))?;
            morph_serve::run_service(morph_serve::ServiceConfig { repos: repo_entries, addr, org_policy_path: org_policy })
                .map_err(|e| anyhow::anyhow!("{}", e))?;
        }

        Command::Diff { old_ref, new_ref, json } => {
            let (_repo_root, store) = get_store(verbose)?;
            let old_hash = resolve_ref_name(&store, &old_ref)?;
            let new_hash = resolve_ref_name(&store, &new_ref)?;
            let entries = morph_core::diff_commits(&store, &old_hash, &new_hash)?;
            if json {
                let changes: Vec<_> = entries.iter().map(|e| serde_json::json!({
                    "status": e.status.to_string(),
                    "path": e.path,
                })).collect();
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
                    let entries: Vec<_> = tags.iter().map(|(name, hash)| serde_json::json!({
                        "name": name,
                        "hash": hash.to_string(),
                        "short": short_hash(&hash.to_string()),
                    })).collect();
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
                    println!("Saved stash {}{}", entry.id, entry.message.as_ref().map(|m| format!(": {}", m)).unwrap_or_default());
                }
                StashCmd::Pop => {
                    let entry = morph_core::stash_pop(&morph_dir)?;
                    println!("Restored stash {}{}", entry.id, entry.message.as_ref().map(|m| format!(": {}", m)).unwrap_or_default());
                }
                StashCmd::List => {
                    for (i, entry) in morph_core::stash_list(&morph_dir)?.iter().enumerate() {
                        println!("stash@{{{}}}: {}", i, entry.message.as_deref().unwrap_or("(no message)"));
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
                let full = if path.is_absolute() { path } else { repo_root.join(&path) };
                let obj = morph_core::blob_from_prompt_file(&full)?;
                let hash = store.put(&obj)?;
                println!("{}", hash);
            }
            PromptCmd::Materialize { hash, output } => {
                let (repo_root, store) = get_store(verbose)?;
                let h = resolve_obj_hash(store.as_ref(), &hash)?;
                let dest = output.unwrap_or_else(|| {
                    repo_root.join(".morph").join("prompts").join(format!("{}.prompt", h))
                });
                morph_core::materialize_blob(&store, &h, &dest)?;
                println!("Materialized to {}", dest.display());
            }
            PromptCmd::Show { run_ref, run_upgrade } => {
                cmd_prompt_show(verbose, &run_ref, run_upgrade)?;
            }
        },

        Command::Pipeline { sub } => match sub {
            PipelineCmd::Create { path } => {
                let (repo_root, store) = get_store(verbose)?;
                let full = if path.is_absolute() { path } else { repo_root.join(&path) };
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
                println!("{}", morph_core::extract_pipeline_from_run(&store, &run_hash)?);
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
                if summary.runs > 0 { parts.push(format!("{} run{}", summary.runs, if summary.runs == 1 { "" } else { "s" })); }
                if summary.traces > 0 { parts.push(format!("{} trace{}", summary.traces, if summary.traces == 1 { "" } else { "s" })); }
                if summary.prompts > 0 { parts.push(format!("{} prompt{}", summary.prompts, if summary.prompts == 1 { "" } else { "s" })); }
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

            // PR 2 (reference mode): surface the count of git-hook
            // commits that haven't been certified yet. Standalone
            // repos never produce git-hook commits, so this branch
            // is a no-op there.
            if morph_core::read_repo_mode(&morph_dir)? == morph_core::RepoMode::Reference {
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
                let items: Vec<_> = entries.iter().map(|e| serde_json::json!({
                    "path": e.path.display().to_string(),
                    "status": if e.in_store { "tracked" } else { "new" },
                    "hash": e.hash.as_ref().map(|h| h.to_string()),
                })).collect();
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
            let any_dir = paths.iter().any(|p| {
                p.as_os_str() == "." || repo_root.join(p).is_dir()
            });
            let hashes = morph_core::add_paths(&store, &repo_root, &paths)?;
            if any_dir && !verbose {
                if !hashes.is_empty() {
                    eprintln!("{} file{} staged", hashes.len(), if hashes.len() == 1 { "" } else { "s" });
                }
            } else {
                for h in &hashes {
                    println!("{}", h);
                }
            }
        }

        Command::Commit { message, pipeline, eval_suite, metrics, author, from_run, allow_empty_metrics, new_cases, no_auto_run, json } => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let version = read_repo_version(&morph_dir)?;
            let prog_hash = pipeline.as_deref().map(|s| resolve_obj_hash(store.as_ref(), s)).transpose()?;
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

            let mut observed_metrics: std::collections::BTreeMap<String, f64> = metrics.as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?
                .unwrap_or_default();
            // Auto-attach the run's metrics only when the user didn't
            // pass `--metrics`. Explicit metrics always win so CI
            // scripts retain deterministic, repeatable input even
            // when a stale breadcrumb is sitting on disk.
            if metrics.is_none() {
                if let Some(ref run_hash) = auto_run_hash {
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
                                    short_hash(&run_hash.to_string()),
                                    preview.join(", "),
                                );
                                observed_metrics = run.metrics.clone();
                            }
                        }
                        Ok(_) => {
                            eprintln!(
                                "warning: LAST_RUN breadcrumb points at non-Run object {}; ignoring",
                                short_hash(&run_hash.to_string()),
                            );
                        }
                        Err(e) => {
                            eprintln!("warning: could not load run from LAST_RUN breadcrumb: {}", e);
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
            let provenance = match from_run.as_ref().map(|s| resolve_obj_hash(store.as_ref(), s)).transpose()? {
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
                if diff.is_empty() { None } else { Some(diff) }
            };

            let resolved_author = morph_core::resolve_author_for_repo(
                &morph_dir,
                author.as_deref(),
            )?;
            let hash = morph_core::create_tree_commit_with_provenance(
                &store, &repo_root, prog_hash.as_ref(), suite_hash.as_ref(),
                observed_metrics, message.clone(), Some(resolved_author),
                Some(&version), provenance.as_ref(),
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
                    &hash, &cases, Some(branch.clone()),
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
                eprintln!("warning: could not clear LAST_RUN breadcrumb after commit: {}", e);
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
                println!("{}", serde_json::to_string(&out).unwrap());
            } else {
                let short = &hash.to_string()[..8];
                let root_tag = if is_root { " (root-commit)" } else { "" };
                let first_line = message.lines().next().unwrap_or("");
                println!("[{}{} {}] {}", branch, root_tag, short, first_line);
                if file_count > 0 {
                    println!(" {} file{} committed", file_count, if file_count == 1 { "" } else { "s" });
                }
            }
        }

        Command::Show { hash } => {
            let (_repo_root, store) = get_store(verbose)?;
            let obj = store.get(&resolve_obj_hash(store.as_ref(), &hash)?)?;
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }

        Command::Log { ref_name, max_count, oneline, full_hash, json } => {
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
                            "short": short_hash(&h.to_string()),
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
                    let display_hash = if full_hash { h_str.clone() } else { short_hash(&h_str) };
                    let subject = c.message.lines().next().unwrap_or("");
                    if oneline {
                        println!("{}  {}", display_hash, subject);
                    } else {
                        let ver_tag = c.morph_version.as_deref()
                            .map(|v| format!("[v{}]", v))
                            .unwrap_or_else(|| "[pre-tree]".into());
                        let tree_tag = if c.tree.is_some() { "" } else { " (no file tree)" };
                        println!("{} {} {}{} {}", display_hash, ver_tag, subject, tree_tag, c.author);
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
                    "short": short_hash(&h_str),
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
                println!("HEAD {} ({})", short_hash(&h_str), where_);
                println!("    {}", subject);
                println!("    {}  {}", commit.author, commit.timestamp);
            }
        }

        Command::Identify { revision, json } => {
            let (_repo_root, store) = get_store(verbose)?;
            let resolved = resolve_obj_hash(store.as_ref(), &revision)?;
            let obj = store.get(&resolved)?;
            let kind = morph_object_type_str(&obj);
            let h_str = resolved.to_string();
            if json {
                let mut body = serde_json::json!({
                    "input": revision,
                    "hash": h_str,
                    "short": short_hash(&h_str),
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

        Command::Branch { name, set_upstream, json } => {
            let (repo_root, store) = get_store(verbose)?;
            // `branch --set-upstream <remote>/<branch>` works on
            // the named branch (or current if unspecified).
            if let Some(spec) = set_upstream {
                let target = match name.as_ref() {
                    Some(n) => n.clone(),
                    None => morph_core::current_branch(&store)?
                        .ok_or_else(|| anyhow::anyhow!(
                            "no current branch (detached HEAD?); name a branch explicitly"
                        ))?,
                };
                let (remote, upstream_branch) = spec
                    .split_once('/')
                    .ok_or_else(|| anyhow::anyhow!(
                        "expected <remote>/<branch>, got: {}", spec
                    ))?;
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
                println!("Created branch {}", branch_name);
            } else {
                // Use the transport-neutral `list_branches` so the
                // same listing works against an SSH-backed remote
                // store (PR5 Stage D) without any code change here.
                let current = morph_core::current_branch(&store)?;
                let mut branches = store.list_branches()?;
                branches.sort_by(|a, b| a.0.cmp(&b.0));
                if json {
                    let entries: Vec<_> = branches.iter().map(|(name, hash)| {
                        let h_str = hash.to_string();
                        serde_json::json!({
                            "name": name,
                            "hash": h_str,
                            "short": short_hash(&h_str),
                            "current": current.as_deref() == Some(name.as_str()),
                        })
                    }).collect();
                    let body = serde_json::json!({
                        "current": current,
                        "branches": entries,
                        "count": branches.len(),
                    });
                    println!("{}", serde_json::to_string_pretty(&body)?);
                } else {
                    for (name, _hash) in branches {
                        let mark = if current.as_deref() == Some(&name) { "* " } else { "  " };
                        println!("{}{}", mark, name);
                    }
                }
            }
        }

        Command::Checkout { ref_name } => {
            let (repo_root, store) = get_store(verbose)?;
            let (hash, tree_restored) = morph_core::checkout_tree(&store, &repo_root, &ref_name)?;
            if ref_name.len() == 64 && ref_name.chars().all(|c| c.is_ascii_hexdigit()) {
                println!("Detached HEAD at {}", hash);
            } else {
                println!("Switched to branch {}", ref_name.trim_start_matches("heads/"));
            }
            if tree_restored {
                verbose_msg(verbose, "working tree restored from commit tree");
            }
        }

        Command::Run { sub } => match sub {
            RunCmd::List { json } => {
                let (_repo_root, store) = get_store(verbose)?;
                let runs = store.list(ObjectType::Run)?;
                if json {
                    let entries: Vec<_> = runs.iter().map(|h| {
                        let h_str = h.to_string();
                        let mut entry = serde_json::json!({
                            "hash": h_str,
                            "short": short_hash(&h_str),
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
                    }).collect();
                    let body = serde_json::json!({ "runs": entries, "count": runs.len() });
                    println!("{}", serde_json::to_string_pretty(&body)?);
                } else {
                    for h in runs {
                        println!("{}", h);
                    }
                }
            }
            RunCmd::Show { hash, json, with_trace } => {
                let (_repo_root, store) = get_store(verbose)?;
                let hash = resolve_obj_hash(store.as_ref(), &hash)?;
                let obj = store.get(&hash)?;
                match &obj {
                    MorphObject::Run(run) => {
                        if json {
                            println!("{}", serde_json::to_string_pretty(run)?);
                        } else {
                            println!("run    {}\ntrace  {}\npipeline {}\nagent  {} {}", hash, run.trace, run.pipeline, run.agent.id, run.agent.version);
                            if let Some(ref c) = run.commit { println!("commit {}", c); }
                            if !run.metrics.is_empty() { println!("metrics {:?}", run.metrics); }
                        }
                        if with_trace {
                            let trace_obj = store.get(&parse_hash(&run.trace)?)?;
                            if let MorphObject::Trace(t) = &trace_obj {
                                println!();
                                print_trace_events(t);
                            } else {
                                anyhow::bail!("object {} is not a trace", run.trace);
                            }
                        }
                    }
                    _ => anyhow::bail!("object {} is not a run", hash),
                }
            }
            RunCmd::Record { run_file, trace, artifact } => {
                let (repo_root, store) = get_store(verbose)?;
                let full_run = if run_file.is_absolute() { run_file } else { repo_root.join(&run_file) };
                let trace_opt = trace.map(|t| if t.is_absolute() { t } else { repo_root.join(&t) });
                let artifact_paths: Vec<_> = artifact.iter().map(|a| if a.is_absolute() { a.clone() } else { repo_root.join(a) }).collect();
                let refs: Vec<_> = artifact_paths.iter().map(|p| p.as_path()).collect();
                println!("{}", morph_core::record_run(&store, &full_run, trace_opt.as_deref(), &refs)?);
            }
            RunCmd::RecordSession { prompt, response, messages, model_name, agent_id } => {
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
            }
        },

        Command::Trace { sub } => match sub {
            TraceCmd::Show { hash } => {
                let (_repo_root, store) = get_store(verbose)?;
                let h = resolve_obj_hash(store.as_ref(), &hash)?;
                match store.get(&h)? {
                    MorphObject::Trace(t) => print_trace_events(&t),
                    _ => anyhow::bail!("object {} is not a trace", hash),
                }
            }
        },

        Command::Tap { sub } => match sub {
            TapCmd::Summary { json } => {
                let (_repo_root, store) = get_store(verbose)?;
                let summary = morph_core::summarize_repo(store.as_ref())?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&summary)?);
                    return Ok(());
                }
                println!("=== Tap Repository Summary ===\n");
                println!("Runs:           {}", summary.total_runs);
                println!("Traces:         {}", summary.total_traces);
                println!("Total events:   {}", summary.total_events);
                println!("Multi-step:     {}", summary.multi_step_runs);
                println!("Empty response: {}", summary.empty_response_runs);
                println!("With metrics:   {}", summary.runs_with_metrics);
                println!("\nEvent kinds:");
                for (kind, count) in &summary.event_kind_counts {
                    println!("  {:<16} {}", kind, count);
                }
                println!("\nModels:");
                for (model, count) in &summary.model_counts {
                    println!("  {:<30} {}", model, count);
                }
                println!("\nAgents:");
                for (agent, count) in &summary.agent_counts {
                    println!("  {:<20} {}", agent, count);
                }
                if !summary.issues.is_empty() {
                    println!("\nIssues:");
                    for issue in &summary.issues {
                        println!("  ⚠ {}", issue);
                    }
                }
            }
            TapCmd::Inspect { run_hash } => {
                let (_repo_root, store) = get_store(verbose)?;
                if run_hash == "all" {
                    let hashes = store.list(ObjectType::Run)?;
                    for h in &hashes {
                        match morph_core::extract_task(store.as_ref(), h) {
                            Ok(task) => print_tap_task(&task),
                            Err(e) => eprintln!("run {}: {}", h, e),
                        }
                    }
                } else {
                    let h = resolve_obj_hash(store.as_ref(), &run_hash)?;
                    let task = morph_core::extract_task(store.as_ref(), &h)?;
                    print_tap_task(&task);
                }
            }
            TapCmd::Diagnose { run_hash } => {
                let (_repo_root, store) = get_store(verbose)?;
                if run_hash == "all" {
                    let hashes = store.list(ObjectType::Run)?;
                    let mut total_issues = 0;
                    let mut issue_counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
                    for h in &hashes {
                        match morph_core::diagnose_run(store.as_ref(), h) {
                            Ok(diag) => {
                                for issue in &diag.issues {
                                    total_issues += 1;
                                    let key = issue.split(" — ").next().unwrap_or(issue).to_string();
                                    *issue_counts.entry(key).or_insert(0) += 1;
                                }
                            }
                            Err(e) => eprintln!("run {}: {}", h, e),
                        }
                    }
                    println!("=== Tap Diagnostic Summary ({} runs) ===\n", hashes.len());
                    println!("Total issues: {}\n", total_issues);
                    for (issue, count) in &issue_counts {
                        println!("  [{:>3}x] {}", count, issue);
                    }
                } else {
                    let h = resolve_obj_hash(store.as_ref(), &run_hash)?;
                    let diag = morph_core::diagnose_run(store.as_ref(), &h)?;
                    println!("{}", serde_json::to_string_pretty(&diag)?);
                }
            }
            TapCmd::Export { mode, output, model, agent, min_steps } => {
                let (_repo_root, store) = get_store(verbose)?;
                let export_mode = match mode.as_str() {
                    "prompt-only" => morph_core::ExportMode::PromptOnly,
                    "with-context" => morph_core::ExportMode::WithContext,
                    "agentic" => morph_core::ExportMode::Agentic,
                    other => anyhow::bail!("unknown export mode '{}' (use: prompt-only, with-context, agentic)", other),
                };

                let cases = if model.is_some() || agent.is_some() || min_steps.is_some() {
                    let filter = morph_core::TapFilter {
                        model,
                        agent,
                        min_steps,
                        has_tool_calls: None,
                    };
                    let run_hashes = morph_core::filter_runs(store.as_ref(), &filter)?;
                    let mut all_cases = Vec::new();
                    for run_hash in &run_hashes {
                        if let Ok(task) = morph_core::extract_task(store.as_ref(), run_hash) {
                            let task_cases = morph_core::task_to_eval_cases(&task, &export_mode);
                            all_cases.extend(task_cases);
                        }
                    }
                    all_cases
                } else {
                    morph_core::export_eval_cases(store.as_ref(), &export_mode)?
                };

                let json = serde_json::to_string_pretty(&cases)?;
                if let Some(path) = output {
                    std::fs::write(&path, &json)?;
                    println!("Exported {} eval cases to {}", cases.len(), path.display());
                } else {
                    println!("{}", json);
                }
            }
            TapCmd::TraceStats { trace_hash } => {
                let (_repo_root, store) = get_store(verbose)?;
                let h = resolve_obj_hash(store.as_ref(), &trace_hash)?;
                let stats = morph_core::trace_stats(store.as_ref(), &h)?;
                println!("=== Trace {} ===\n", &trace_hash[..12.min(trace_hash.len())]);
                println!("Events:             {}", stats.event_count);
                println!("Structured events:  {}", if stats.has_structured_events { "yes" } else { "no" });
                if let Some((first, last)) = &stats.timestamp_range {
                    println!("Time range:         {} .. {}", first, last);
                }
                println!("\nEvent kinds (raw):");
                for (kind, count) in &stats.event_kinds {
                    println!("  {:<20} {}", kind, count);
                }
                println!("\nEvent kinds (normalized):");
                for (kind, count) in &stats.normalized_kinds {
                    println!("  {:<20} {}", kind, count);
                }
                println!("\nPayload keys:");
                for (key, count) in &stats.payload_keys {
                    println!("  {:<20} {}", key, count);
                }
                if !stats.prompt_lengths.is_empty() {
                    let avg: f64 = stats.prompt_lengths.iter().sum::<usize>() as f64 / stats.prompt_lengths.len() as f64;
                    println!("\nPrompt lengths:     {} prompts, avg {:.0} chars", stats.prompt_lengths.len(), avg);
                }
                if !stats.response_lengths.is_empty() {
                    let avg: f64 = stats.response_lengths.iter().sum::<usize>() as f64 / stats.response_lengths.len() as f64;
                    println!("Response lengths:   {} responses, avg {:.0} chars", stats.response_lengths.len(), avg);
                }
            }
            TapCmd::Preview { run_hash, mode } => {
                let (_repo_root, store) = get_store(verbose)?;
                let h = resolve_obj_hash(store.as_ref(), &run_hash)?;
                let task = morph_core::extract_task(store.as_ref(), &h)?;
                let export_mode = match mode.as_str() {
                    "prompt-only" => morph_core::ExportMode::PromptOnly,
                    "with-context" => morph_core::ExportMode::WithContext,
                    "agentic" => morph_core::ExportMode::Agentic,
                    other => anyhow::bail!("unknown export mode '{}' (use: prompt-only, with-context, agentic)", other),
                };
                let cases = morph_core::task_to_eval_cases(&task, &export_mode);

                println!("=== Preview: {} ({} steps, mode: {}) ===\n", &run_hash[..12.min(run_hash.len())], task.step_count, mode);
                println!("Model: {}  Agent: {}", task.model, task.agent);
                println!();

                for case in &cases {
                    println!("--- Step {}/{} ---", case.step_index + 1, case.total_steps);
                    println!("[PROMPT] ({} chars)", case.prompt.len());
                    let prompt_preview = if case.prompt.len() > 300 {
                        format!("{}...", &case.prompt[..case.prompt.floor_char_boundary(300)])
                    } else {
                        case.prompt.clone()
                    };
                    println!("{}", prompt_preview);

                    if let Some(ref ctx) = case.context {
                        println!("\n[CONTEXT] ({} chars)", ctx.len());
                        let ctx_preview = if ctx.len() > 500 {
                            format!("{}...", &ctx[..ctx.floor_char_boundary(500)])
                        } else {
                            ctx.clone()
                        };
                        println!("{}", ctx_preview);
                    }

                    if !case.file_reads.is_empty() {
                        println!("\n[FILE READS] {}", case.file_reads.len());
                        for fr in &case.file_reads {
                            let has_content = fr.content.is_some();
                            println!("  {} {}", fr.path.as_deref().unwrap_or("?"),
                                if has_content { "(has content)" } else { "(path only)" });
                        }
                    }
                    if !case.file_edits.is_empty() {
                        println!("\n[FILE EDITS] {}", case.file_edits.len());
                        for fe in &case.file_edits {
                            let has_content = fe.content.is_some();
                            println!("  {} {}", fe.path.as_deref().unwrap_or("?"),
                                if has_content { "(has content)" } else { "(path only)" });
                        }
                    }
                    if !case.tool_calls.is_empty() {
                        println!("\n[TOOL CALLS] {}", case.tool_calls.len());
                        for tc in &case.tool_calls {
                            println!("  {} {}{}", tc.name.as_deref().unwrap_or("(unnamed)"),
                                if tc.output.is_some() { "[has output]" } else { "" },
                                if tc.error.is_some() { " [has error]" } else { "" });
                        }
                    }

                    println!("\n[EXPECTED RESPONSE] ({} chars)", case.expected_response.len());
                    let resp_preview = if case.expected_response.len() > 300 {
                        format!("{}...", &case.expected_response[..case.expected_response.floor_char_boundary(300)])
                    } else {
                        case.expected_response.clone()
                    };
                    println!("{}\n", resp_preview);
                }
            }
        },

        Command::Traces { sub } => handle_traces_command(verbose, sub)?,

        Command::Eval { sub } => match sub {
            EvalCmd::Record { file } => {
                let (repo_root, _store) = get_store(verbose)?;
                let full = if file.is_absolute() { file } else { repo_root.join(&file) };
                let metrics = morph_core::record_eval_metrics(&full)?;
                println!("{}", serde_json::to_string_pretty(&metrics)?);
            }
            EvalCmd::FromOutput { runner, file, record } => {
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
                        Ok((repo_root, _store)) => {
                            std::fs::read_to_string(repo_root.join(&file))?
                        }
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
            EvalCmd::AddCase {
                paths,
                suite,
                no_default,
                no_set_default,
            } => {
                if paths.is_empty() {
                    return Err(anyhow::anyhow!(
                        "no paths supplied. Usage: morph eval add-case <file_or_dir>..."
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
            }
            EvalCmd::SuiteFromSpecs { paths, no_set_default } => {
                if paths.is_empty() {
                    return Err(anyhow::anyhow!(
                        "no paths supplied. Usage: morph eval suite-from-specs <dir>..."
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
                let new_hash =
                    morph_core::build_or_extend_suite(store.as_ref(), None, &cases)?;
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
            }
            EvalCmd::SuiteShow { suite, json } => {
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
                                 Run `morph eval add-case <spec>` first or pass `--suite <hash>`."
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
                        let kind = c
                            .input
                            .get("kind")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?");
                        println!("    - {}  [{}]  metric={}", c.id, kind, c.metric);
                    }
                    for m in &suite_obj.metrics {
                        println!(
                            "    metric: {} agg={} threshold={} dir={}",
                            m.name, m.aggregation, m.threshold, m.direction
                        );
                    }
                }
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
            EvalCmd::Run { runner, cwd, command } => {
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
            let retired: Option<Vec<String>> = retire.map(|s| s.split(',').map(|m| m.trim().to_string()).collect());
            let plan = morph_core::prepare_merge(&store, &branch, None, retired.as_deref())?;
            print!("{}", plan.format_plan());
        }

        Command::Merge { branch, cont, abort, message, pipeline, eval_suite, metrics, author, retire, retire_reason, sub } => {
            run_merge(verbose, branch, cont, abort, message, pipeline, eval_suite, metrics, author, retire, retire_reason, sub)?;
        }

        Command::Rollup { base_ref, tip_ref, message } => {
            let (_repo_root, store) = get_store(verbose)?;
            println!("{}", morph_core::rollup(&store, &base_ref, &tip_ref, message)?);
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
                let stored = if morph_core::ssh_store::SshUrl::parse(&raw).is_some()
                    || path.is_absolute()
                {
                    raw
                } else {
                    std::env::current_dir()?.join(&path).to_string_lossy().to_string()
                };
                morph_core::add_remote(&morph_dir, &name, &stored)?;
                println!("Remote '{}' added: {}", name, stored);
            }
            RemoteCmd::List { json } => {
                let (repo_root, _store) = get_store(verbose)?;
                let remotes = morph_core::read_remotes(&repo_root.join(".morph"))?;
                if json {
                    let entries: Vec<_> = remotes.iter().map(|(name, spec)| serde_json::json!({
                        "name": name,
                        "path": spec.path,
                    })).collect();
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
            let spec = remotes.get(&remote)
                .ok_or_else(|| anyhow::anyhow!("remote '{}' not found", remote))?;
            let remote_store = morph_core::open_remote_store(&spec.path)?;
            let tip = morph_core::push_branch(local_store.as_ref(), remote_store.as_ref(), &branch)?;
            println!("Pushed {} -> {}/{} ({})", branch, remote, branch, tip);
        }

        Command::Fetch { remote } => {
            let (repo_root, local_store) = get_store(verbose)?;
            let remotes = morph_core::read_remotes(&repo_root.join(".morph"))?;
            let spec = remotes.get(&remote)
                .ok_or_else(|| anyhow::anyhow!("remote '{}' not found", remote))?;
            let remote_store = morph_core::open_remote_store(&spec.path)?;
            let updated = morph_core::fetch_remote(local_store.as_ref(), remote_store.as_ref(), &remote)?;
            if updated.is_empty() {
                println!("Already up to date.");
            } else {
                for (branch, hash) in &updated {
                    println!("{}/{} -> {}", remote, branch, hash);
                }
            }
        }

        Command::Pull { remote, branch, merge } => {
            let (repo_root, local_store) = get_store(verbose)?;
            let remotes = morph_core::read_remotes(&repo_root.join(".morph"))?;
            let spec = remotes.get(&remote)
                .ok_or_else(|| anyhow::anyhow!("remote '{}' not found", remote))?;
            let remote_store = morph_core::open_remote_store(&spec.path)?;
            match morph_core::pull_branch(local_store.as_ref(), remote_store.as_ref(), &remote, &branch) {
                Ok(tip) => {
                    println!("Updated {} -> {} ({})", branch, tip, remote);
                }
                Err(morph_core::MorphError::Diverged { branch: b, local_tip, remote_tip })
                    if merge =>
                {
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
                        println!(
                            "Merged {} -> {} ({})",
                            b, cont.merge_commit, remote
                        );
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
                None => morph_core::current_branch(&local_store)?
                    .ok_or_else(|| anyhow::anyhow!(
                        "no current branch (detached HEAD?); name a branch explicitly"
                    ))?,
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
                Err(morph_core::MorphError::Diverged { branch: b, local_tip, remote_tip }) => {
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
                let entries: Vec<_> = refs.iter().map(|(name, hash)| {
                    let h_str = hash.to_string();
                    serde_json::json!({
                        "name": name,
                        "hash": h_str,
                        "short": short_hash(&h_str),
                    })
                }).collect();
                let body = serde_json::json!({ "refs": entries, "count": refs.len() });
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                for (name, hash) in refs {
                    println!("{}\t{}", hash, name);
                }
            }
        }

        Command::Config { key, value, get } => {
            // PR 6 stage A: a minimal `morph config` subcommand.
            // Today only `user.name` and `user.email` are first-class;
            // unknown keys produce a helpful error rather than
            // silently writing to a generic JSON tree.
            let (repo_root, _) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let getting = get || value.is_none();
            let (cfg_name, cfg_email) = morph_core::read_identity_config(&morph_dir)?;
            match key.as_str() {
                "user.name" => {
                    if getting {
                        match cfg_name {
                            Some(v) => println!("{}", v),
                            None => std::process::exit(1),
                        }
                    } else {
                        morph_core::write_identity_config(
                            &morph_dir,
                            value.as_deref(),
                            None,
                        )?;
                    }
                }
                "user.email" => {
                    if getting {
                        match cfg_email {
                            Some(v) => println!("{}", v),
                            None => std::process::exit(1),
                        }
                    } else {
                        morph_core::write_identity_config(
                            &morph_dir,
                            None,
                            value.as_deref(),
                        )?;
                    }
                }
                other => {
                    return Err(anyhow::anyhow!(
                        "unsupported config key '{}'. Supported keys: user.name, user.email",
                        other
                    ));
                }
            }
        }

        Command::Certify { metrics, metrics_file, commit, eval_suite, runner, author, json } => {
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
                (Some(s), None) => serde_json::from_str(&s)
                    .map_err(|e| anyhow::anyhow!("--metrics is not a JSON object of metric → number: {}", e))?,
                (None, Some(path)) => {
                    let full_path = if path.is_absolute() { path } else { repo_root.join(&path) };
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
                &store, &morph_dir, &commit_hash, &metrics,
                runner.as_deref().or(author.as_deref()), eval_suite.as_deref(),
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else if result.passed {
                println!("PASS: commit {} certified", commit_hash);
                for (k, v) in &result.metrics_provided { println!("  {} = {}", k, v); }
            } else {
                eprintln!("FAIL: commit {} not certified", commit_hash);
                for f in &result.failures { eprintln!("  {}", f); }
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
                if !result.passed { std::process::exit(1); }
            } else if result.passed {
                println!("PASS: commit {} satisfies policy", commit_hash);
            } else {
                eprintln!("FAIL: commit {} does not satisfy policy", commit_hash);
                for r in &result.reasons { eprintln!("  {}", r); }
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
                let full = if file.is_absolute() { file } else { repo_root.join(&file) };
                let policy: morph_core::RepoPolicy = serde_json::from_str(&std::fs::read_to_string(&full)?)?;
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

        Command::Annotate { target_hash, kind, data, sub, author } => {
            let (_repo_root, store) = get_store(verbose)?;
            let target = resolve_obj_hash(store.as_ref(), &target_hash)?;
            let data_map: std::collections::BTreeMap<String, serde_json::Value> =
                serde_json::from_str(&data).map_err(|e| anyhow::anyhow!("invalid --data JSON: {}", e))?;
            let ann = morph_core::create_annotation(&target, sub, kind, data_map, author);
            println!("{}", store.put(&ann)?);
        }

        Command::Annotations { target_hash, sub, json } => {
            let (_repo_root, store) = get_store(verbose)?;
            let target = resolve_obj_hash(store.as_ref(), &target_hash)?;
            let anns = morph_core::list_annotations(&store, &target, sub.as_deref())?;
            if json {
                let entries: Vec<_> = anns.iter().map(|(h, a)| {
                    let h_str = h.to_string();
                    serde_json::json!({
                        "hash": h_str,
                        "short": short_hash(&h_str),
                        "kind": a.kind,
                        "author": a.author,
                        "target": a.target,
                        "target_sub": a.target_sub,
                        "data": a.data,
                    })
                }).collect();
                let body = serde_json::json!({
                    "target": target.to_string(),
                    "target_short": short_hash(&target.to_string()),
                    "annotations": entries,
                    "count": anns.len(),
                });
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                for (h, a) in &anns {
                    println!("{} {} {} {}", h, a.kind, a.author, serde_json::to_string(&a.data).unwrap_or_default());
                }
            }
        }

        Command::HashObject { path } => {
            let (repo_root, store) = get_store(verbose)?;
            let full = if path.is_absolute() { path } else { repo_root.join(&path) };
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
            run_files.first()
                .ok_or_else(|| anyhow::anyhow!("no runs in .morph/runs/"))?.path()
        } else if let Some(n_str) = run_ref.strip_prefix("latest~").or_else(|| run_ref.strip_prefix("latest-")) {
            let n: usize = n_str.parse().map_err(|_| anyhow::anyhow!("invalid ref '{}': expected latest~N", run_ref))?;
            run_files.get(n)
                .ok_or_else(|| anyhow::anyhow!("no run at index {} (only {} run(s))", n, run_files.len()))?.path()
        } else if run_ref.len() == 64 && run_ref.chars().all(|c| c.is_ascii_hexdigit()) {
            let path = runs_dir.join(format!("{}.json", run_ref));
            if !path.exists() { anyhow::bail!("run not found: {}", run_ref); }
            path
        } else {
            anyhow::bail!("invalid ref '{}': use 'latest', 'latest~N', or a 64-char run hash", run_ref);
        };

        let run_json = std::fs::read_to_string(&run_path)?;
        let run: morph_core::objects::Run = serde_json::from_str(&run_json)?;
        let trace_hash = parse_hash(&run.trace)?;

        match store.get(&trace_hash) {
            Ok(MorphObject::Trace(t)) => {
                let text = t.events.iter().rfind(|e| e.kind == "prompt" || e.kind == "user")
                    .and_then(|e| e.payload.get("text").and_then(|v| v.as_str())).unwrap_or("");
                print!("{}", text);
                return Ok(());
            }
            Ok(_) => anyhow::bail!("object {} is not a trace", run.trace),
            Err(_) => {
                let trace_path = morph_dir.join("traces").join(format!("{}.json", run.trace));
                if trace_path.exists() {
                    let obj: MorphObject = serde_json::from_str(&std::fs::read_to_string(&trace_path)?)?;
                    if let MorphObject::Trace(t) = obj {
                        let text = t.events.iter().rfind(|e| e.kind == "prompt" || e.kind == "user")
                            .and_then(|e| e.payload.get("text").and_then(|v| v.as_str())).unwrap_or("");
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

/// Resolve a user-supplied hash to a Run hash. Accepts either a Run hash
/// directly or a Trace hash (in which case we locate the latest Run
/// pointing at that trace).
fn resolve_run_hash(store: &dyn Store, hash_str: &str) -> anyhow::Result<Hash> {
    let h = resolve_obj_hash(store, hash_str)?;
    match store.get(&h)? {
        MorphObject::Run(_) => Ok(h),
        MorphObject::Trace(_) => morph_core::find_run_by_trace(store, &h)?
            .ok_or_else(|| anyhow::anyhow!("no run points to trace {}", hash_str)),
        _ => anyhow::bail!("hash {} is neither a Run nor a Trace", hash_str),
    }
}

fn handle_traces_command(verbose: bool, sub: TracesCmd) -> anyhow::Result<()> {
    let (_repo_root, store) = get_store(verbose)?;
    match sub {
        TracesCmd::Summary { limit, json } => {
            let summaries = morph_core::recent_trace_summaries(store.as_ref(), limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&summaries)?);
            } else {
                println!("=== Recent Traces ({} shown) ===\n", summaries.len());
                for s in &summaries {
                    let short = &s.run_hash[..12.min(s.run_hash.len())];
                    let phase = serde_json::to_string(&s.task_phase).unwrap_or_default();
                    let scope = serde_json::to_string(&s.task_scope).unwrap_or_default();
                    println!("{} {}  phase={}  scope={}", short, s.timestamp, phase.trim_matches('"'), scope.trim_matches('"'));
                    if !s.target_files.is_empty() {
                        println!("  files:   {}", s.target_files.join(", "));
                    }
                    if !s.target_symbols.is_empty() {
                        println!("  symbols: {}", s.target_symbols.join(", "));
                    }
                    println!("  prompt:  {}\n", s.prompt_preview);
                }
            }
        }
        TracesCmd::TaskStructure { hash } => {
            let run_hash = resolve_run_hash(store.as_ref(), &hash)?;
            let out = morph_core::task_structure(store.as_ref(), &run_hash)?;
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        TracesCmd::TargetContext { hash } => {
            let run_hash = resolve_run_hash(store.as_ref(), &hash)?;
            let out = morph_core::target_context(store.as_ref(), &run_hash)?;
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        TracesCmd::FinalArtifact { hash } => {
            let run_hash = resolve_run_hash(store.as_ref(), &hash)?;
            let out = morph_core::final_artifact(store.as_ref(), &run_hash)?;
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        TracesCmd::Semantics { hash } => {
            let run_hash = resolve_run_hash(store.as_ref(), &hash)?;
            let out = morph_core::change_semantics(store.as_ref(), &run_hash)?;
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        TracesCmd::Verification { hash } => {
            let run_hash = resolve_run_hash(store.as_ref(), &hash)?;
            let out = morph_core::verification_steps(store.as_ref(), &run_hash)?;
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PR 8: `default_clone_dest` mirrors `git clone`'s
    /// directory-naming heuristic: take the last URL segment, strip
    /// a trailing `.morph` if present.
    #[test]
    fn default_clone_dest_strips_morph_suffix() {
        assert_eq!(default_clone_dest("you@host:repos/myproject.morph"), "myproject");
        assert_eq!(default_clone_dest("ssh://you@host/srv/proj.morph"), "proj");
        assert_eq!(default_clone_dest("/tmp/foo.morph"), "foo");
        assert_eq!(default_clone_dest("/tmp/bar/"), "bar");
        assert_eq!(default_clone_dest("plain"), "plain");
        assert_eq!(default_clone_dest(""), "morph-clone");
    }

    #[test]
    fn short_hash_truncates_to_eight_chars() {
        assert_eq!(short_hash("abcdef0123456789abcdef0123456789"), "abcdef01");
        assert_eq!(short_hash("abc"), "abc");
        assert_eq!(short_hash(""), "");
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
