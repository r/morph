//! Morph CLI: read path and manual write operations.

mod cli;
#[cfg(feature = "cursor-setup")]
mod setup;

use clap::Parser;
use cli::*;
use morph_core::{
    find_repo, migrate_0_0_to_0_2, migrate_0_2_to_0_3, open_store, read_repo_version,
    require_store_version, Hash, MorphObject, ObjectType, Store,
    STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_INIT,
};
use std::path::PathBuf;

fn get_store(verbose: bool) -> anyhow::Result<(PathBuf, Box<dyn Store>)> {
    let cwd = std::env::current_dir()?;
    let repo_root = find_repo(&cwd)
        .ok_or_else(|| anyhow::anyhow!("not a morph repository (or any parent)"))?;
    let morph_dir = repo_root.join(".morph");
    let version = read_repo_version(&morph_dir)?;
    verbose_msg(verbose, &format!("repo {} (store version {})", repo_root.display(), version));
    require_store_version(&morph_dir, &[STORE_VERSION_INIT, STORE_VERSION_0_2, STORE_VERSION_0_3])?;
    let store = open_store(&morph_dir)?;
    Ok((repo_root, store))
}

fn parse_hash(s: &str) -> anyhow::Result<Hash> {
    Hash::from_hex(s).map_err(|e| anyhow::anyhow!("invalid hash: {}", e))
}

fn verbose_msg(on: bool, msg: &str) {
    if on {
        eprintln!("morph: {}", msg);
    }
}

fn print_trace_events(trace: &morph_core::objects::Trace) {
    for ev in &trace.events {
        let text = ev.payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
        match ev.kind.as_str() {
            "prompt" => println!("--- prompt ---\n{}", text),
            "response" => println!("--- response ---\n{}", text),
            _ => println!("--- {} ---\n{}", ev.kind, text),
        }
    }
}

fn resolve_ref_name(store: &dyn Store, r: &str) -> anyhow::Result<Hash> {
    if r.len() == 64 && r.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(Hash::from_hex(r)?)
    } else if r == "HEAD" {
        morph_core::resolve_head(store)?
            .ok_or_else(|| anyhow::anyhow!("HEAD has no commits"))
    } else {
        store.ref_read(&format!("heads/{}", r))?
            .or_else(|| store.ref_read(&format!("tags/{}", r)).ok().flatten())
            .ok_or_else(|| anyhow::anyhow!("unknown ref: {}", r))
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let verbose = cli.verbose;

    match cli.command {
        Command::Init { path } => {
            verbose_msg(verbose, &format!("initializing repo at {}", path.display()));
            morph_core::init_repo(&path)?;
            println!("Initialized Morph repository in {}", path.display());
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
        },

        Command::Upgrade => {
            let cwd = std::env::current_dir()?;
            let repo_root = find_repo(&cwd)
                .ok_or_else(|| anyhow::anyhow!("not a morph repository (or any parent)"))?;
            let morph_dir = repo_root.join(".morph");
            let version = read_repo_version(&morph_dir)?;
            verbose_msg(verbose, &format!("store version {}", version));
            if version == STORE_VERSION_0_3 {
                println!("Store version is {} (latest). No upgrade needed.", version);
            } else if version == STORE_VERSION_0_2 {
                migrate_0_2_to_0_3(&morph_dir)?;
                println!("Migrated store from {} to {}.", STORE_VERSION_0_2, STORE_VERSION_0_3);
            } else if version == STORE_VERSION_INIT {
                migrate_0_0_to_0_2(&morph_dir)?;
                migrate_0_2_to_0_3(&morph_dir)?;
                println!("Migrated store from {} to {}.", STORE_VERSION_INIT, STORE_VERSION_0_3);
            } else {
                println!("Store version is {}. No upgrade path.", version);
            }
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
            let hash = parse_hash(&commit)?;
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
                let h = parse_hash(&hash)?;
                let dest = output.unwrap_or_else(|| {
                    repo_root.join(".morph").join("prompts").join(format!("{}.prompt", hash))
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
                let obj = store.get(&parse_hash(&hash)?)?;
                println!("{}", serde_json::to_string_pretty(&obj)?);
            }
            PipelineCmd::Extract { from_run } => {
                let (_repo_root, store) = get_store(verbose)?;
                let run_hash = parse_hash(&from_run)?;
                println!("{}", morph_core::extract_pipeline_from_run(&store, &run_hash)?);
            }
        },

        Command::Status => {
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
            for h in morph_core::add_paths(&store, &repo_root, &paths)? {
                println!("{}", h);
            }
        }

        Command::Commit { message, pipeline, eval_suite, metrics, author, from_run } => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let version = read_repo_version(&morph_dir)?;
            let prog_hash = pipeline.as_deref().map(parse_hash).transpose()?;
            let suite_hash = eval_suite.as_deref().map(parse_hash).transpose()?;
            let observed_metrics = metrics.as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?
                .unwrap_or_default();
            let provenance = match from_run {
                Some(ref run_hash_str) => {
                    let run_hash = parse_hash(run_hash_str)?;
                    Some(morph_core::resolve_provenance_from_run(&store, &run_hash)?)
                }
                None => None,
            };
            let hash = morph_core::create_tree_commit_with_provenance(
                &store, &repo_root, prog_hash.as_ref(), suite_hash.as_ref(),
                observed_metrics, message, author, Some(&version), provenance.as_ref(),
            )?;
            println!("{}", hash);
        }

        Command::Show { hash } => {
            let (_repo_root, store) = get_store(verbose)?;
            let obj = store.get(&parse_hash(&hash)?)?;
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
                let hash = parse_hash(&hash)?;
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
            RunCmd::RecordSession { prompt, response, model_name, agent_id } => {
                let (_repo_root, store) = get_store(verbose)?;
                println!("{}", morph_core::record_session(&store, &prompt, &response, model_name.as_deref(), agent_id.as_deref())?);
            }
        },

        Command::Trace { sub } => match sub {
            TraceCmd::Show { hash } => {
                let (_repo_root, store) = get_store(verbose)?;
                match store.get(&parse_hash(&hash)?)? {
                    MorphObject::Trace(t) => print_trace_events(&t),
                    _ => anyhow::bail!("object {} is not a trace", hash),
                }
            }
        },

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
            let prog_hash = parse_hash(&pipeline)?;
            let suite_hash_opt = eval_suite.as_deref().map(parse_hash).transpose()?;
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

        Command::Pull { remote, branch } => {
            let (repo_root, local_store) = get_store(verbose)?;
            let remotes = morph_core::read_remotes(&repo_root.join(".morph"))?;
            let spec = remotes.get(&remote)
                .ok_or_else(|| anyhow::anyhow!("remote '{}' not found", remote))?;
            let remote_store = morph_core::open_remote_store(&spec.path)?;
            let tip = morph_core::pull_branch(local_store.as_ref(), remote_store.as_ref(), &remote, &branch)?;
            println!("Updated {} -> {} ({})", branch, tip, remote);
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
                Some(ref h) => parse_hash(h)?,
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
                Some(ref h) => parse_hash(h)?,
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
                let (repo_root, _store) = get_store(verbose)?;
                let morph_dir = repo_root.join(".morph");
                let _ = parse_hash(&hash)?;
                let mut policy = morph_core::read_policy(&morph_dir)?;
                policy.default_eval_suite = Some(hash.clone());
                morph_core::write_policy(&morph_dir, &policy)?;
                println!("Default eval suite set to {}", hash);
            }
        },

        Command::Annotate { target_hash, kind, data, sub, author } => {
            let (_repo_root, store) = get_store(verbose)?;
            let target = parse_hash(&target_hash)?;
            let data_map: std::collections::BTreeMap<String, serde_json::Value> =
                serde_json::from_str(&data).map_err(|e| anyhow::anyhow!("invalid --data JSON: {}", e))?;
            let ann = morph_core::create_annotation(&target, sub, kind, data_map, author);
            println!("{}", store.put(&ann)?);
        }

        Command::Annotations { target_hash, sub } => {
            let (_repo_root, store) = get_store(verbose)?;
            let target = parse_hash(&target_hash)?;
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
                let text = t.events.iter().filter(|e| e.kind == "prompt").last()
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
                        let text = t.events.iter().filter(|e| e.kind == "prompt").last()
                            .and_then(|e| e.payload.get("text").and_then(|v| v.as_str())).unwrap_or("");
                        print!("{}", text);
                        return Ok(());
                    }
                }
                if run_upgrade && !upgraded {
                    let version = read_repo_version(&morph_dir)?;
                    if version == STORE_VERSION_0_3 {
                        eprintln!("Store already at {}.", STORE_VERSION_0_3);
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
