//! Morph CLI: read path and manual write operations.

use clap::Parser;
use morph_core::{find_repo, migrate_0_0_to_0_2, migrate_0_2_to_0_3, open_store, read_repo_version, require_store_version, Hash, Store, STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_INIT};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "morph")]
#[command(about = "Version control for transformation programs")]
struct Cli {
    /// Print what the CLI is doing (to stderr)
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Initialize a Morph repository
    Init {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Prompt object operations
    Prompt {
        #[command(subcommand)]
        sub: PromptCmd,
    },
    /// Program operations
    Program {
        #[command(subcommand)]
        sub: ProgramCmd,
    },
    /// Show working space status
    Status,
    /// Stage working-space changes into the object store
    Add {
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
    },
    /// Create a commit (snapshots the staged file tree; program and eval suite are optional)
    Commit {
        #[arg(short, long)]
        message: String,
        #[arg(long)]
        program: Option<String>,
        #[arg(long)]
        eval_suite: Option<String>,
        #[arg(long)]
        metrics: Option<String>,
        #[arg(long)]
        author: Option<String>,
    },
    /// Show commit history
    Log {
        #[arg(default_value = "HEAD")]
        ref_name: String,
    },
    /// Create or list branches
    Branch {
        name: Option<String>,
    },
    /// Switch branch or detach to a commit
    Checkout {
        ref_name: String,
    },
    /// Ingest a run (execution receipt)
    Run {
        #[command(subcommand)]
        sub: RunCmd,
    },
    /// Ingest evaluation results
    Eval {
        #[command(subcommand)]
        sub: EvalCmd,
    },
    /// Merge a branch (behavioral dominance required)
    Merge {
        branch: String,
        #[arg(short, long)]
        message: String,
        #[arg(long)]
        program: String,
        #[arg(long)]
        eval_suite: String,
        #[arg(long)]
        metrics: String,
        #[arg(long)]
        author: Option<String>,
    },
    /// Rollup (squash) commits: one new commit from base to tip
    Rollup {
        base_ref: String,
        tip_ref: String,
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Attach an annotation to an object (or event)
    Annotate {
        target_hash: String,
        #[arg(short, long)]
        kind: String,
        #[arg(short, long)]
        data: String,
        #[arg(long)]
        sub: Option<String>,
        #[arg(long)]
        author: Option<String>,
    },
    /// List annotations on an object (optionally filtered by sub-target)
    Annotations {
        target_hash: String,
        #[arg(long)]
        sub: Option<String>,
    },
    /// Read a Morph object from a JSON file, store it, and print its content hash (for hook scripts)
    HashObject {
        path: PathBuf,
    },
    /// Upgrade the repo store to the latest version (required before using MCP on older repos).
    Upgrade,
    /// Browse repo in browser (commit strip, prompts, tree). Use --port and --interface to bind.
    #[cfg(feature = "visualize")]
    #[command(name = "visualize")]
    Visualize {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value = "8765")]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        interface: String,
    },
}

#[derive(clap::Subcommand)]
enum RunCmd {
    /// Record a Run object from JSON file
    Record {
        run_file: PathBuf,
        #[arg(long)]
        trace: Option<PathBuf>,
        #[arg(long)]
        artifact: Vec<PathBuf>,
    },
    /// Record a single prompt/response session (Run + Trace) into the store
    RecordSession {
        #[arg(long)]
        prompt: String,
        #[arg(long)]
        response: String,
        #[arg(long)]
        model_name: Option<String>,
        #[arg(long)]
        agent_id: Option<String>,
    },
}

#[derive(clap::Subcommand)]
enum EvalCmd {
    /// Record evaluation metrics from JSON file
    Record {
        file: PathBuf,
    },
}

#[derive(clap::Subcommand)]
enum PromptCmd {
    /// Create a prompt blob from a file
    Create {
        path: PathBuf,
    },
    /// Write a prompt blob to the working space
    Materialize {
        hash: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Print prompt text from a run. Ref: "latest" (default), "latest~N" / "latest-N" (N back), or run hash (64 hex chars). Like git show for runs.
    Latest {
        /// Run ref: latest (default), latest~N / latest-N, or run hash
        #[arg(default_value = "latest")]
        run_ref: String,
    },
}

#[derive(clap::Subcommand)]
enum ProgramCmd {
    /// Create a program object from a JSON file
    Create {
        path: PathBuf,
    },
    /// Show a program object
    Show {
        hash: String,
    },
    /// Print the identity program hash (and ensure it exists in the store). Use from repo root for hook scripts.
    IdentityHash,
}

fn get_store(verbose: bool) -> anyhow::Result<(PathBuf, Box<dyn Store>)> {
    let cwd = std::env::current_dir()?;
    let repo_root = find_repo(&cwd).ok_or_else(|| anyhow::anyhow!("not a morph repository (or any parent)"))?;
    let morph_dir = repo_root.join(".morph");
    let version = read_repo_version(&morph_dir)?;
    if verbose {
        verbose_msg(verbose, &format!("repo {} (store version {})", repo_root.display(), version));
    }
    require_store_version(&morph_dir, &[STORE_VERSION_INIT, STORE_VERSION_0_2, STORE_VERSION_0_3])?;
    let store = open_store(&morph_dir)?;
    Ok((repo_root, store))
}

fn parse_hash(s: &str) -> anyhow::Result<Hash> {
    Hash::from_hex(s).map_err(|e| anyhow::anyhow!("invalid hash: {}", e))
}

/// Print a verbose progress message to stderr (only when --verbose).
fn verbose_msg(on: bool, msg: &str) {
    if on {
        eprintln!("morph: {}", msg);
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let verbose = cli.verbose;
    match cli.command {
        Command::Init { path } => {
            verbose_msg(verbose, &format!("initializing repo at {}", path.display()));
            let _store = morph_core::init_repo(&path)?;
            println!("Initialized Morph repository in {}", path.display());
        }
        Command::Upgrade => {
            let cwd = std::env::current_dir()?;
            verbose_msg(verbose, &format!("looking for repo from {}", cwd.display()));
            let repo_root = find_repo(&cwd).ok_or_else(|| anyhow::anyhow!("not a morph repository (or any parent); run from project root"))?;
            let morph_dir = repo_root.join(".morph");
            let version = read_repo_version(&morph_dir)?;
            verbose_msg(verbose, &format!("store version {}", version));
            if version == STORE_VERSION_0_3 {
                println!("Store version is {} (latest). No upgrade needed.", version);
            } else if version == STORE_VERSION_0_2 {
                verbose_msg(verbose, "migrating 0.2 → 0.3 (adding tree commit support)");
                migrate_0_2_to_0_3(&morph_dir)?;
                println!("Migrated store from {} to {}. Old commits have no file tree; new commits will.", STORE_VERSION_0_2, STORE_VERSION_0_3);
            } else if version == STORE_VERSION_INIT {
                verbose_msg(verbose, "migrating 0.0 → 0.2 (rewriting object hashes)");
                migrate_0_0_to_0_2(&morph_dir)?;
                verbose_msg(verbose, "migrating 0.2 → 0.3 (adding tree commit support)");
                migrate_0_2_to_0_3(&morph_dir)?;
                println!("Migrated store from {} to {}. Hashes changed; old commits have no file tree.", STORE_VERSION_INIT, STORE_VERSION_0_3);
            } else {
                println!("Store version is {}. No upgrade path from this version.", version);
            }
        }
        #[cfg(feature = "visualize")]
        Command::Visualize { path, port, interface } => {
            let repo_root = path.canonicalize().unwrap_or(path);
            let morph_dir = if repo_root.join(".morph").exists() {
                repo_root.join(".morph")
            } else {
                find_repo(&repo_root)
                    .ok_or_else(|| anyhow::anyhow!("not a morph repository (no .morph found)"))?
                    .join(".morph")
            };
            let addr = format!("{}:{}", interface, port)
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid interface or port: {}:{}", interface, port))?;
            morph_serve::run_blocking(morph_dir, addr).map_err(|e| anyhow::anyhow!("{}", e))?;
        }
        Command::Prompt { sub } => match sub {
            PromptCmd::Create { path } => {
                let (repo_root, store) = get_store(verbose)?;
                let full = if path.is_absolute() { path } else { repo_root.join(&path) };
                verbose_msg(verbose, &format!("creating prompt blob from {}", full.display()));
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
                verbose_msg(verbose, &format!("materializing {} to {}", hash, dest.display()));
                morph_core::materialize_blob(&store, &h, &dest)?;
                println!("Materialized to {}", dest.display());
            }
            PromptCmd::Latest { run_ref } => {
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
                } else if let Some(n_str) = run_ref.strip_prefix("latest~").or_else(|| run_ref.strip_prefix("latest-")) {
                    let n: usize = n_str.parse().map_err(|_| anyhow::anyhow!("invalid ref '{}': expected latest~N or latest-N with N a non-negative integer", run_ref))?;
                    run_files
                        .get(n)
                        .ok_or_else(|| anyhow::anyhow!("no run at index {} (only {} run(s))", n, run_files.len()))?
                        .path()
                } else if run_ref.len() == 64 && run_ref.chars().all(|c| c.is_ascii_hexdigit()) {
                    let path = runs_dir.join(format!("{}.json", run_ref));
                    if !path.exists() {
                        anyhow::bail!("run not found: {}", run_ref);
                    }
                    path
                } else {
                    anyhow::bail!("invalid ref '{}': use 'latest', 'latest~N' / 'latest-N', or a 64-char run hash", run_ref);
                };
                let run_json = std::fs::read_to_string(&run_path)?;
                let run: morph_core::objects::Run =
                    serde_json::from_str(&run_json).map_err(|e| anyhow::anyhow!("invalid run JSON: {}", e))?;
                let trace_hash = parse_hash(&run.trace)?;
                let obj = store.get(&trace_hash)?;
                let trace = match &obj {
                    morph_core::MorphObject::Trace(t) => t,
                    _ => anyhow::bail!("object {} is not a trace", run.trace),
                };
                let prompt_text = trace
                    .events
                    .iter()
                    .filter(|e| e.kind == "prompt")
                    .last()
                    .and_then(|e| e.payload.get("text").and_then(|v| v.as_str()))
                    .unwrap_or("");
                print!("{}", prompt_text);
            }
        },
        Command::Program { sub } => match sub {
            ProgramCmd::Create { path } => {
                let (repo_root, store) = get_store(verbose)?;
                let full = if path.is_absolute() { path } else { repo_root.join(&path) };
                verbose_msg(verbose, &format!("creating program from {}", full.display()));
                let obj = morph_core::program_from_file(&full)?;
                let hash = store.put(&obj)?;
                println!("{}", hash);
            }
            ProgramCmd::IdentityHash => {
                let (_repo_root, store) = get_store(verbose)?;
                verbose_msg(verbose, "ensuring identity program in store");
                let identity = morph_core::identity_program();
                let hash = store.put(&identity)?;
                println!("{}", hash);
            }
            ProgramCmd::Show { hash } => {
                let (_repo_root, store) = get_store(verbose)?;
                verbose_msg(verbose, &format!("reading object {}", hash));
                let h = parse_hash(&hash)?;
                let obj = store.get(&h)?;
                let json = serde_json::to_string_pretty(&obj).map_err(|e| anyhow::anyhow!("{}", e))?;
                println!("{}", json);
            }
        },
        Command::Status => {
            let (repo_root, store) = get_store(verbose)?;
            verbose_msg(verbose, &format!("status in {}", repo_root.display()));
            let entries = morph_core::status(&store, &repo_root)?;
            if entries.is_empty() {
                println!("No files to track");
                return Ok(());
            }
            verbose_msg(verbose, &format!("{} entries", entries.len()));
            for e in entries {
                let status = if e.in_store { "tracked" } else { "new" };
                let hash_str = e.hash.as_ref().map(|h| h.to_string()).unwrap_or_default();
                println!("{} {} {}", status, hash_str, e.path.display());
            }
        }
        Command::Add { paths } => {
            let (repo_root, store) = get_store(verbose)?;
            verbose_msg(verbose, &format!("add paths in {}: {:?}", repo_root.display(), paths));
            let hashes = morph_core::add_paths(&store, &repo_root, &paths)?;
            verbose_msg(verbose, &format!("staged {} object(s)", hashes.len()));
            for h in hashes {
                println!("{}", h);
            }
        }
        Command::Commit { message, program, eval_suite, metrics, author } => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let version = read_repo_version(&morph_dir)?;
            let prog_hash = program
                .as_deref()
                .map(parse_hash)
                .transpose()?;
            let suite_hash = eval_suite
                .as_deref()
                .map(parse_hash)
                .transpose()?;
            verbose_msg(verbose, &format!("commit (program={}, eval_suite={})",
                program.as_deref().unwrap_or("identity"),
                eval_suite.as_deref().unwrap_or("empty")));
            let observed_metrics = metrics
                .as_deref()
                .map(|s| serde_json::from_str(s))
                .transpose()
                .map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?
                .unwrap_or_default();
            let hash = morph_core::create_tree_commit(
                &store,
                &repo_root,
                prog_hash.as_ref(),
                suite_hash.as_ref(),
                observed_metrics,
                message,
                author,
                Some(&version),
            )?;
            verbose_msg(verbose, &format!("created commit {}", hash));
            println!("{}", hash);
        }
        Command::Log { ref_name } => {
            let (_repo_root, store) = get_store(verbose)?;
            verbose_msg(verbose, &format!("log from {}", ref_name));
            let hashes = morph_core::log_from(&store, &ref_name)?;
            for h in hashes {
                let obj = store.get(&h)?;
                if let morph_core::MorphObject::Commit(c) = obj {
                    let ver_tag = match c.morph_version.as_deref() {
                        Some(v) => format!("[v{}]", v),
                        None => "[pre-tree]".to_string(),
                    };
                    let tree_tag = if c.tree.is_some() { "" } else { " (no file tree)" };
                    println!("{} {} {}{} {}", h, ver_tag, c.message.lines().next().unwrap_or(""), tree_tag, c.author);
                }
            }
        }
        Command::Branch { name } => {
            let (_repo_root, store) = get_store(verbose)?;
            verbose_msg(verbose, if name.is_some() { "creating branch" } else { "listing branches" });
            if let Some(branch_name) = name {
                let head = morph_core::resolve_head(&store)?.ok_or_else(|| anyhow::anyhow!("no commit yet; make a commit first"))?;
                store.ref_write(&format!("heads/{}", branch_name), &head)?;
                println!("Created branch {}", branch_name);
            } else {
                let refs_dir = store.refs_dir().join("heads");
                if refs_dir.exists() {
                    for e in std::fs::read_dir(&refs_dir)? {
                        let e = e?;
                        let n = e.file_name().to_string_lossy().into_owned();
                        let current = morph_core::current_branch(&store)?;
                        let mark = if current.as_deref() == Some(&n) { "* " } else { "  " };
                        println!("{}{}", mark, n);
                    }
                }
            }
        }
        Command::Checkout { ref_name } => {
            let (repo_root, store) = get_store(verbose)?;
            verbose_msg(verbose, &format!("checkout {}", ref_name));
            let (hash, tree_restored) = morph_core::checkout_tree(&store, &repo_root, &ref_name)?;
            if ref_name.len() == 64 && ref_name.chars().all(|c| c.is_ascii_hexdigit()) {
                println!("Detached HEAD at {}", hash);
            } else {
                println!("Switched to branch {}", ref_name.trim_start_matches("heads/"));
            }
            if tree_restored {
                verbose_msg(verbose, "working tree restored from commit tree");
            } else {
                verbose_msg(verbose, "commit has no file tree (pre-0.3); working tree unchanged");
            }
        }
        Command::Run { sub } => match sub {
            RunCmd::Record { run_file, trace, artifact } => {
                let (_repo_root, store) = get_store(verbose)?;
                verbose_msg(verbose, &format!("recording run from {}", run_file.display()));
                let full_run = if run_file.is_absolute() { run_file.clone() } else { _repo_root.join(&run_file) };
                let trace_opt = trace.map(|t| if t.is_absolute() { t } else { _repo_root.join(&t) });
                let artifact_paths: Vec<_> = artifact.iter().map(|a| if a.is_absolute() { a.clone() } else { _repo_root.join(a) }).collect();
                let refs: Vec<_> = artifact_paths.iter().map(|p| p.as_path()).collect();
                let hash = morph_core::record_run(
                    &store,
                    &full_run,
                    trace_opt.as_deref(),
                    &refs,
                )?;
                println!("{}", hash);
            }
            RunCmd::RecordSession { prompt, response, model_name, agent_id } => {
                let (_repo_root, store) = get_store(verbose)?;
                verbose_msg(verbose, "recording session (run + trace)");
                let hash = morph_core::record_session(
                    &store,
                    &prompt,
                    &response,
                    model_name.as_deref(),
                    agent_id.as_deref(),
                )?;
                println!("{}", hash);
            }
        },
        Command::Eval { sub } => match sub {
            EvalCmd::Record { file } => {
                let (_repo_root, _store) = get_store(verbose)?;
                let full = if file.is_absolute() { file } else { _repo_root.join(&file) };
                verbose_msg(verbose, &format!("reading eval metrics from {}", full.display()));
                let metrics = morph_core::record_eval_metrics(&full)?;
                let json = serde_json::to_string_pretty(&metrics).map_err(|e| anyhow::anyhow!("{}", e))?;
                println!("{}", json);
            }
        },
        Command::Merge { branch, message, program, eval_suite, metrics, author } => {
            let (_repo_root, store) = get_store(verbose)?;
            verbose_msg(verbose, &format!("merge branch {} (program={} suite={})", branch, program, eval_suite));
            let prog_hash = parse_hash(&program)?;
            let suite_hash = parse_hash(&eval_suite)?;
            let observed: std::collections::BTreeMap<String, f64> =
                serde_json::from_str(&metrics).map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?;
            let hash = morph_core::create_merge_commit(
                &store,
                &branch,
                &prog_hash,
                observed,
                &suite_hash,
                message,
                author,
            )?;
            println!("{}", hash);
        }
        Command::Rollup { base_ref, tip_ref, message } => {
            let (_repo_root, store) = get_store(verbose)?;
            verbose_msg(verbose, &format!("rollup {}..{}", base_ref, tip_ref));
            let hash = morph_core::rollup(&store, &base_ref, &tip_ref, message)?;
            println!("{}", hash);
        }
        Command::Annotate { target_hash, kind, data, sub, author } => {
            let (_repo_root, store) = get_store(verbose)?;
            verbose_msg(verbose, &format!("annotate {} kind={}", target_hash, kind));
            let target = parse_hash(&target_hash)?;
            let data_map: std::collections::BTreeMap<String, serde_json::Value> =
                serde_json::from_str(&data).map_err(|e| anyhow::anyhow!("invalid --data JSON: {}", e))?;
            let ann = morph_core::create_annotation(&target, sub, kind, data_map, author);
            let hash = store.put(&ann)?;
            println!("{}", hash);
        }
        Command::Annotations { target_hash, sub } => {
            let (_repo_root, store) = get_store(verbose)?;
            verbose_msg(verbose, &format!("annotations for {}", target_hash));
            let target = parse_hash(&target_hash)?;
            let list = morph_core::list_annotations(&store, &target, sub.as_deref())?;
            for (h, a) in list {
                println!("{} {} {} {}", h, a.kind, a.author, serde_json::to_string(&a.data).unwrap_or_default());
            }
        }
        Command::HashObject { path } => {
            let (repo_root, store) = get_store(verbose)?;
            let full = if path.is_absolute() { path } else { repo_root.join(&path) };
            verbose_msg(verbose, &format!("hash-object {}", full.display()));
            let json = std::fs::read_to_string(&full)?;
            let obj: morph_core::MorphObject = serde_json::from_str(&json).map_err(|e| anyhow::anyhow!("invalid Morph object JSON: {}", e))?;
            let hash = store.put(&obj)?;
            println!("{}", hash);
        }
    }
    Ok(())
}
