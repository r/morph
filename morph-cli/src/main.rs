//! Morph CLI: read path and manual write operations.

mod cli;
#[cfg(feature = "cursor-setup")]
mod setup;

use clap::Parser;
use cli::*;
use morph_core::{
    find_repo, migrate_0_0_to_0_2, migrate_0_2_to_0_3, migrate_0_3_to_0_4,
    migrate_0_4_to_0_5, open_store,
    read_repo_version, require_store_version, resolve_hash_prefix, Hash, MorphObject, ObjectType,
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

/// Resolve a user-supplied hash string against the store, accepting any
/// unambiguous prefix of ≥4 hex chars (Git-style). Full 64-char hashes are
/// parsed directly without scanning the store.
fn resolve_obj_hash(store: &dyn Store, s: &str) -> anyhow::Result<Hash> {
    resolve_hash_prefix(store, s).map_err(|e| anyhow::anyhow!("{}", e))
}

fn verbose_msg(on: bool, msg: &str) {
    if on {
        eprintln!("morph: {}", msg);
    }
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

fn resolve_ref_name(store: &dyn Store, r: &str) -> anyhow::Result<Hash> {
    if r.len() == 64 && r.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(Hash::from_hex(r)?);
    }
    if r == "HEAD" {
        return morph_core::resolve_head(store)?
            .ok_or_else(|| anyhow::anyhow!("HEAD has no commits"));
    }
    if let Some(h) = store.ref_read(&format!("heads/{}", r))? {
        return Ok(h);
    }
    if let Some(h) = store.ref_read(&format!("tags/{}", r)).ok().flatten() {
        return Ok(h);
    }
    // Fall back to Git-style hash-prefix lookup (≥4 hex chars).
    if r.len() >= 4 && r.chars().all(|c| c.is_ascii_hexdigit()) {
        return resolve_hash_prefix(store, r)
            .map_err(|e| anyhow::anyhow!("unknown ref '{}': {}", r, e));
    }
    anyhow::bail!("unknown ref: {}", r)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let verbose = cli.verbose;

    match cli.command {
        Command::Init { path } => {
            verbose_msg(verbose, &format!("initializing repo at {}", path.display()));
            morph_core::init_repo(&path)?;
            let abs_morph = path.canonicalize()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&path))
                .join(".morph");
            println!("Initialized empty Morph repository in {}/", abs_morph.display());
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

        Command::Diff { old_ref, new_ref } => {
            let (_repo_root, store) = get_store(verbose)?;
            let old_hash = resolve_ref_name(&store, &old_ref)?;
            let new_hash = resolve_ref_name(&store, &new_ref)?;
            let entries = morph_core::diff_commits(&store, &old_hash, &new_hash)?;
            for e in &entries {
                println!("{}  {}", e.status, e.path);
            }
        }

        Command::Tag { name, delete } => {
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
                for (name, hash) in morph_core::list_tags(&store)? {
                    println!("{}  {}", name, hash);
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

        Command::Status => {
            let (repo_root, store) = get_store(verbose)?;
            let changes = morph_core::working_status(&store, &repo_root)?;
            let summary = morph_core::activity_summary(&store, &repo_root)?;
            let merge_progress = morph_core::merge_progress_summary(&*store, &repo_root)?;

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

            if changes.is_empty() && summary.runs == 0 && summary.traces == 0 && summary.prompts == 0 && merge_progress.is_none() {
                println!("nothing to commit, working tree clean");
                return Ok(());
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
        }

        Command::Files => {
            let (repo_root, store) = get_store(verbose)?;
            let entries = morph_core::status(&store, &repo_root)?;
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

        Command::Commit { message, pipeline, eval_suite, metrics, author, from_run, json } => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let version = read_repo_version(&morph_dir)?;
            let prog_hash = pipeline.as_deref().map(|s| resolve_obj_hash(store.as_ref(), s)).transpose()?;
            let suite_hash = eval_suite.as_deref().map(|s| resolve_obj_hash(store.as_ref(), s)).transpose()?;
            let observed_metrics = metrics.as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?
                .unwrap_or_default();
            let provenance = match from_run {
                Some(ref run_hash_str) => {
                    let run_hash = resolve_obj_hash(store.as_ref(), run_hash_str)?;
                    Some(morph_core::resolve_provenance_from_run(&store, &run_hash)?)
                }
                None => None,
            };

            let branch = morph_core::current_branch(&store)?.unwrap_or_else(|| "main".to_string());
            let is_root = morph_core::resolve_head(&store)?.is_none();
            let index = morph_core::read_index(&morph_dir)?;
            let file_count = index.entries.len();

            let hash = morph_core::create_tree_commit_with_provenance(
                &store, &repo_root, prog_hash.as_ref(), suite_hash.as_ref(),
                observed_metrics, message.clone(), author, Some(&version), provenance.as_ref(),
            )?;

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

        Command::Log { ref_name } => {
            let (_repo_root, store) = get_store(verbose)?;
            for h in morph_core::log_from(&store, &ref_name)? {
                if let MorphObject::Commit(c) = store.get(&h)? {
                    let ver_tag = c.morph_version.as_deref().map(|v| format!("[v{}]", v)).unwrap_or_else(|| "[pre-tree]".into());
                    let tree_tag = if c.tree.is_some() { "" } else { " (no file tree)" };
                    println!("{} {} {}{} {}", h, ver_tag, c.message.lines().next().unwrap_or(""), tree_tag, c.author);
                }
            }
        }

        Command::Branch { name } => {
            let (_repo_root, store) = get_store(verbose)?;
            if let Some(branch_name) = name {
                let head = morph_core::resolve_head(&store)?
                    .ok_or_else(|| anyhow::anyhow!("no commit yet; make a commit first"))?;
                store.ref_write(&format!("heads/{}", branch_name), &head)?;
                println!("Created branch {}", branch_name);
            } else {
                let refs_dir = store.refs_dir().join("heads");
                if refs_dir.exists() {
                    let current = morph_core::current_branch(&store)?;
                    for e in std::fs::read_dir(&refs_dir)? {
                        let n = e?.file_name().to_string_lossy().into_owned();
                        let mark = if current.as_deref() == Some(&n) { "* " } else { "  " };
                        println!("{}{}", mark, n);
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
            RunCmd::List => {
                let (_repo_root, store) = get_store(verbose)?;
                for h in store.list(ObjectType::Run)? {
                    println!("{}", h);
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
            TapCmd::Summary => {
                let (_repo_root, store) = get_store(verbose)?;
                let summary = morph_core::summarize_repo(store.as_ref())?;
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
        },

        Command::MergePlan { branch, retire } => {
            let (_repo_root, store) = get_store(verbose)?;
            let retired: Option<Vec<String>> = retire.map(|s| s.split(',').map(|m| m.trim().to_string()).collect());
            let plan = morph_core::prepare_merge(&store, &branch, None, retired.as_deref())?;
            print!("{}", plan.format_plan());
        }

        Command::Merge { branch, message, pipeline, eval_suite, metrics, author, retire } => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let version = read_repo_version(&morph_dir)?;
            let prog_hash = resolve_obj_hash(store.as_ref(), &pipeline)?;
            let suite_hash_opt = eval_suite.as_deref().map(|s| resolve_obj_hash(store.as_ref(), s)).transpose()?;
            let observed: std::collections::BTreeMap<String, f64> =
                serde_json::from_str(&metrics).map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?;
            let retired: Option<Vec<String>> = retire.map(|s| s.split(',').map(|m| m.trim().to_string()).collect());
            let plan = morph_core::prepare_merge(&store, &branch, suite_hash_opt.as_ref(), retired.as_deref())?;
            let hash = morph_core::execute_merge(&store, &plan, &prog_hash, observed, message, author, Some(&repo_root), Some(&version))?;
            println!("{}", hash);
        }

        Command::Rollup { base_ref, tip_ref, message } => {
            let (_repo_root, store) = get_store(verbose)?;
            println!("{}", morph_core::rollup(&store, &base_ref, &tip_ref, message)?);
        }

        Command::Remote { sub } => match sub {
            RemoteCmd::Add { name, path } => {
                let (repo_root, _store) = get_store(verbose)?;
                let morph_dir = repo_root.join(".morph");
                let abs_path = if path.is_absolute() {
                    path.to_string_lossy().to_string()
                } else {
                    std::env::current_dir()?.join(&path).to_string_lossy().to_string()
                };
                morph_core::add_remote(&morph_dir, &name, &abs_path)?;
                println!("Remote '{}' added: {}", name, abs_path);
            }
            RemoteCmd::List => {
                let (repo_root, _store) = get_store(verbose)?;
                for (name, spec) in morph_core::read_remotes(&repo_root.join(".morph"))? {
                    println!("{}\t{}", name, spec.path);
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

        Command::Refs => {
            let (_repo_root, store) = get_store(verbose)?;
            for (name, hash) in morph_core::list_refs(store.as_ref())? {
                println!("{}\t{}", hash, name);
            }
        }

        Command::Certify { metrics_file, commit, eval_suite, runner, author, json } => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let commit_hash = match commit {
                Some(ref h) => resolve_obj_hash(store.as_ref(), h)?,
                None => morph_core::resolve_head(&store)?
                    .ok_or_else(|| anyhow::anyhow!("no HEAD commit; specify --commit"))?,
            };
            let full_path = if metrics_file.is_absolute() { metrics_file } else { repo_root.join(&metrics_file) };
            let metrics: std::collections::BTreeMap<String, f64> =
                serde_json::from_str(&std::fs::read_to_string(&full_path)?)?;
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
        },

        Command::Annotate { target_hash, kind, data, sub, author } => {
            let (_repo_root, store) = get_store(verbose)?;
            let target = resolve_obj_hash(store.as_ref(), &target_hash)?;
            let data_map: std::collections::BTreeMap<String, serde_json::Value> =
                serde_json::from_str(&data).map_err(|e| anyhow::anyhow!("invalid --data JSON: {}", e))?;
            let ann = morph_core::create_annotation(&target, sub, kind, data_map, author);
            println!("{}", store.put(&ann)?);
        }

        Command::Annotations { target_hash, sub } => {
            let (_repo_root, store) = get_store(verbose)?;
            let target = resolve_obj_hash(store.as_ref(), &target_hash)?;
            for (h, a) in morph_core::list_annotations(&store, &target, sub.as_deref())? {
                println!("{} {} {} {}", h, a.kind, a.author, serde_json::to_string(&a.data).unwrap_or_default());
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
