//! Morph CLI: read path and manual write operations.

#[cfg(feature = "cursor-setup")]
mod setup;

use clap::Parser;
use morph_core::{find_repo, migrate_0_0_to_0_2, migrate_0_2_to_0_3, open_store, read_repo_version, require_store_version, Hash, MorphObject, ObjectType, Store, STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_INIT};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "morph")]
#[command(about = "Version control for transformation pipelines")]
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
    /// Pipeline operations
    Pipeline {
        #[command(subcommand)]
        sub: PipelineCmd,
    },
    /// Show working space status
    Status,
    /// Stage working-space changes into the object store
    Add {
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
    },
    /// Create a commit (snapshots the staged file tree; pipeline and eval suite are optional)
    Commit {
        #[arg(short, long)]
        message: String,
        #[arg(long)]
        pipeline: Option<String>,
        #[arg(long)]
        eval_suite: Option<String>,
        #[arg(long)]
        metrics: Option<String>,
        #[arg(long)]
        author: Option<String>,
        /// Derive provenance (evidence_refs, env_constraints, contributors) from a recorded Run hash
        #[arg(long)]
        from_run: Option<String>,
    },
    /// Show a stored Morph object (commit, run, trace, etc.) as pretty JSON
    Show {
        /// Object hash (64 hex chars)
        hash: String,
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
    /// Preview merge requirements: inspect parents, union suite, and reference bar
    MergePlan {
        /// Branch to merge into the current branch
        branch: String,
        /// Retire metrics from the union suite (paper §5.3). Comma-separated names.
        #[arg(long)]
        retire: Option<String>,
    },
    /// Merge a branch (behavioral dominance required)
    Merge {
        branch: String,
        #[arg(short, long)]
        message: String,
        #[arg(long)]
        pipeline: String,
        /// Eval suite hash. If omitted, auto-computes from the union of both parents' suites.
        #[arg(long)]
        eval_suite: Option<String>,
        #[arg(long)]
        metrics: String,
        #[arg(long)]
        author: Option<String>,
        /// Retire metrics from the union suite (paper §5.3). Comma-separated names.
        #[arg(long)]
        retire: Option<String>,
    },
    /// Rollup (squash) commits: one new commit from base to tip
    Rollup {
        base_ref: String,
        tip_ref: String,
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Manage named remotes
    Remote {
        #[command(subcommand)]
        sub: RemoteCmd,
    },
    /// Push a branch to a remote repository
    Push {
        /// Remote name (e.g. "origin")
        remote: String,
        /// Branch to push
        branch: String,
    },
    /// Fetch branches from a remote into remote-tracking refs
    Fetch {
        /// Remote name
        remote: String,
    },
    /// Pull: fetch from remote + fast-forward local branch
    Pull {
        /// Remote name
        remote: String,
        /// Branch to pull
        branch: String,
    },
    /// List all refs (local branches and remote-tracking refs)
    Refs,
    /// Certify a commit using externally produced metrics (CI/team workflow)
    Certify {
        /// Path to a JSON file with metric name/value pairs (e.g. {"acc": 0.95, "f1": 0.9})
        #[arg(long)]
        metrics_file: PathBuf,
        /// Commit hash to certify (defaults to HEAD)
        #[arg(long)]
        commit: Option<String>,
        /// Eval suite hash used during evaluation
        #[arg(long)]
        eval_suite: Option<String>,
        /// CI runner or evaluator identity
        #[arg(long)]
        runner: Option<String>,
        /// Author identity
        #[arg(long)]
        author: Option<String>,
        /// Emit JSON output instead of human-readable text
        #[arg(long)]
        json: bool,
    },
    /// Check whether a commit satisfies the project's behavioral policy (CI gate)
    Gate {
        /// Commit hash to check (defaults to HEAD)
        #[arg(long)]
        commit: Option<String>,
        /// Emit JSON output instead of human-readable text
        #[arg(long)]
        json: bool,
    },
    /// Manage repository behavioral policy
    Policy {
        #[command(subcommand)]
        sub: PolicyCmd,
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
    /// Set up IDE integration (hooks, MCP config, rules)
    #[cfg(feature = "cursor-setup")]
    Setup {
        #[command(subcommand)]
        sub: SetupCmd,
    },
    /// Upgrade the repo store to the latest version (required before using MCP on older repos).
    Upgrade,
    /// Inspect traces (prompt/response events)
    Trace {
        #[command(subcommand)]
        sub: TraceCmd,
    },
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
    /// Run the Morph hosted service (multi-repo inspection, shared API, org policy).
    #[cfg(feature = "visualize")]
    Serve {
        /// Repository paths as name=path pairs. Defaults to current repo as "default".
        #[arg(long = "repo", value_name = "NAME=PATH")]
        repos: Vec<String>,
        #[arg(long, default_value = "8765")]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        interface: String,
        /// Path to an org-level policy JSON file
        #[arg(long)]
        org_policy: Option<PathBuf>,
    },
}

#[derive(clap::Subcommand)]
enum TraceCmd {
    /// Show a trace by hash (prompt and response text from events)
    Show {
        /// Trace hash (64 hex chars)
        hash: String,
    },
}

#[cfg(feature = "cursor-setup")]
#[derive(clap::Subcommand)]
enum SetupCmd {
    /// Install Cursor hooks, MCP config, and evaluation rules
    Cursor {
        /// Project root (defaults to current directory)
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
}

#[derive(clap::Subcommand)]
enum RunCmd {
    /// List recorded runs (run hashes)
    List,
    /// Show one run by hash (summary or full JSON)
    Show {
        /// Run hash (64 hex chars)
        hash: String,
        /// Emit full run JSON
        #[arg(long)]
        json: bool,
        /// Also print the trace (all prompt and response text)
        #[arg(long)]
        with_trace: bool,
    },
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
    Show {
        /// Run ref: latest (default), latest~N / latest-N, or 64-char run hash. E.g. morph prompt show latest~1
        #[arg(default_value = "latest")]
        run_ref: String,
        /// If trace is missing, run morph upgrade and retry once (may fix store version mismatches)
        #[arg(long)]
        run_upgrade: bool,
    },
}

#[derive(clap::Subcommand)]
enum RemoteCmd {
    /// Add a named remote
    Add {
        /// Remote name (e.g. "origin")
        name: String,
        /// Path to the remote Morph repository
        path: PathBuf,
    },
    /// List configured remotes
    List,
}

#[derive(clap::Subcommand)]
enum PolicyCmd {
    /// Show the current repository policy
    Show,
    /// Set the repository policy from a JSON file
    Set {
        /// Path to a JSON file containing the policy
        file: PathBuf,
    },
    /// Set the default eval suite for certification
    SetDefaultEval {
        /// Eval suite hash
        hash: String,
    },
}

#[derive(clap::Subcommand)]
enum PipelineCmd {
    /// Create a pipeline object from a JSON file
    Create {
        path: PathBuf,
    },
    /// Show a pipeline object
    Show {
        hash: String,
    },
    /// Print the identity pipeline hash (and ensure it exists in the store). Use from repo root for hook scripts.
    IdentityHash,
    /// Extract a Pipeline from a recorded Run (trace-backed pipeline extraction)
    Extract {
        /// Source Run hash to extract the Pipeline from
        #[arg(long)]
        from_run: String,
    },
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

/// Print prompt and response text from a trace's events to stdout.
fn print_trace_events(trace: &morph_core::objects::Trace) {
    for ev in &trace.events {
        let text = ev
            .payload
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        match ev.kind.as_str() {
            "prompt" => {
                println!("--- prompt ---");
                println!("{}", text);
            }
            "response" => {
                println!("--- response ---");
                println!("{}", text);
            }
            _ => {
                println!("--- {} ---", ev.kind);
                println!("{}", text);
            }
        }
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
        #[cfg(feature = "cursor-setup")]
        Command::Setup { sub } => match sub {
            SetupCmd::Cursor { path } => {
                let root = std::path::Path::new(&path)
                    .canonicalize()
                    .unwrap_or_else(|_| path.clone());
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
        #[cfg(feature = "visualize")]
        Command::Serve { repos, port, interface, org_policy } => {
            let repo_entries = if repos.is_empty() {
                let cwd = std::env::current_dir()?;
                let repo_root = find_repo(&cwd)
                    .ok_or_else(|| anyhow::anyhow!("not a morph repository (or any parent); specify --repo name=path"))?;
                vec![morph_serve::RepoEntry {
                    name: "default".to_string(),
                    morph_dir: repo_root.join(".morph"),
                }]
            } else {
                let mut entries = Vec::new();
                for spec in &repos {
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
                    entries.push(morph_serve::RepoEntry { name: name.to_string(), morph_dir });
                }
                entries
            };
            let addr: std::net::SocketAddr = format!("{}:{}", interface, port)
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid interface or port: {}:{}", interface, port))?;
            let config = morph_serve::ServiceConfig {
                repos: repo_entries,
                addr,
                org_policy_path: org_policy,
            };
            morph_serve::run_service(config).map_err(|e| anyhow::anyhow!("{}", e))?;
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
            PromptCmd::Show { run_ref, run_upgrade } => {
                let mut upgraded = false;
                let err_msg: String = loop {
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
                    match store.get(&trace_hash) {
                        Ok(morph_core::MorphObject::Trace(t)) => {
                            let prompt_text = t
                                .events
                                .iter()
                                .filter(|e| e.kind == "prompt")
                                .last()
                                .and_then(|e| e.payload.get("text").and_then(|v| v.as_str()))
                                .unwrap_or("");
                            print!("{}", prompt_text);
                            break String::new();
                        }
                        Ok(_) => anyhow::bail!("object {} is not a trace", run.trace),
                        Err(_) => {
                            let trace_path = morph_dir.join("traces").join(format!("{}.json", run.trace));
                            if trace_path.exists() {
                                let trace_json = std::fs::read_to_string(&trace_path)?;
                                let obj: morph_core::MorphObject =
                                    serde_json::from_str(&trace_json).map_err(|e| anyhow::anyhow!("invalid trace JSON in {}: {}", trace_path.display(), e))?;
                                if let morph_core::MorphObject::Trace(t) = obj {
                                    let prompt_text = t
                                        .events
                                        .iter()
                                        .filter(|e| e.kind == "prompt")
                                        .last()
                                        .and_then(|e| e.payload.get("text").and_then(|v| v.as_str()))
                                        .unwrap_or("");
                                    print!("{}", prompt_text);
                                    break String::new();
                                }
                            }
                            if run_upgrade && !upgraded {
                                let version = read_repo_version(&morph_dir)?;
                                verbose_msg(verbose, &format!("trace not found; running upgrade (store version {})", version));
                                if version == STORE_VERSION_0_3 {
                                    eprintln!("Store already at {}. Upgrade did not run.", STORE_VERSION_0_3);
                                } else if version == STORE_VERSION_0_2 {
                                    migrate_0_2_to_0_3(&morph_dir)?;
                                    eprintln!("Ran migrate 0.2 → 0.3. Retrying...");
                                } else if version == STORE_VERSION_INIT {
                                    migrate_0_0_to_0_2(&morph_dir)?;
                                    migrate_0_2_to_0_3(&morph_dir)?;
                                    eprintln!("Ran migrate 0.0 → 0.3. Retrying...");
                                } else {
                                    eprintln!("Store version {} has no upgrade path.", version);
                                }
                                upgraded = true;
                                continue;
                            }
                            let run_hash = run_path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
                            break format!(
                                "trace not found: {} (run {}). \
                                 Run 'morph upgrade' and retry, or try 'morph prompt show latest~2'. \
                                 Pass --run-upgrade to have the CLI run upgrade and retry. \
                                 See docs/CURSOR-SETUP.md#debugging-object-not-found.",
                                run.trace,
                                run_hash
                            );
                        }
                    }
                };
                if !err_msg.is_empty() {
                    anyhow::bail!("{}", err_msg);
                }
            }
        },
        Command::Pipeline { sub } => match sub {
            PipelineCmd::Create { path } => {
                let (repo_root, store) = get_store(verbose)?;
                let full = if path.is_absolute() { path } else { repo_root.join(&path) };
                verbose_msg(verbose, &format!("creating pipeline from {}", full.display()));
                let obj = morph_core::pipeline_from_file(&full)?;
                let hash = store.put(&obj)?;
                println!("{}", hash);
            }
            PipelineCmd::IdentityHash => {
                let (_repo_root, store) = get_store(verbose)?;
                verbose_msg(verbose, "ensuring identity pipeline in store");
                let identity = morph_core::identity_pipeline();
                let hash = store.put(&identity)?;
                println!("{}", hash);
            }
            PipelineCmd::Show { hash } => {
                let (_repo_root, store) = get_store(verbose)?;
                verbose_msg(verbose, &format!("reading object {}", hash));
                let h = parse_hash(&hash)?;
                let obj = store.get(&h)?;
                let json = serde_json::to_string_pretty(&obj).map_err(|e| anyhow::anyhow!("{}", e))?;
                println!("{}", json);
            }
            PipelineCmd::Extract { from_run } => {
                let (_repo_root, store) = get_store(verbose)?;
                verbose_msg(verbose, &format!("extracting pipeline from run {}", from_run));
                let run_hash = parse_hash(&from_run)?;
                let pipeline_hash = morph_core::extract_pipeline_from_run(&store, &run_hash)?;
                verbose_msg(verbose, &format!("created pipeline {}", pipeline_hash));
                println!("{}", pipeline_hash);
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
        Command::Commit { message, pipeline, eval_suite, metrics, author, from_run } => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let version = read_repo_version(&morph_dir)?;
            let prog_hash = pipeline
                .as_deref()
                .map(parse_hash)
                .transpose()?;
            let suite_hash = eval_suite
                .as_deref()
                .map(parse_hash)
                .transpose()?;
            verbose_msg(verbose, &format!("commit (pipeline={}, eval_suite={})",
                pipeline.as_deref().unwrap_or("identity"),
                eval_suite.as_deref().unwrap_or("empty")));
            let observed_metrics = metrics
                .as_deref()
                .map(|s| serde_json::from_str(s))
                .transpose()
                .map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?
                .unwrap_or_default();
            let provenance = match from_run {
                Some(ref run_hash_str) => {
                    let run_hash = parse_hash(run_hash_str)?;
                    verbose_msg(verbose, &format!("resolving provenance from run {}", run_hash));
                    Some(morph_core::resolve_provenance_from_run(&store, &run_hash)?)
                }
                None => None,
            };
            let hash = morph_core::create_tree_commit_with_provenance(
                &store,
                &repo_root,
                prog_hash.as_ref(),
                suite_hash.as_ref(),
                observed_metrics,
                message,
                author,
                Some(&version),
                provenance.as_ref(),
            )?;
            verbose_msg(verbose, &format!("created commit {}", hash));
            println!("{}", hash);
        }
        Command::Show { hash } => {
            let (_repo_root, store) = get_store(verbose)?;
            verbose_msg(verbose, &format!("show object {}", hash));
            let h = parse_hash(&hash)?;
            let obj = store.get(&h)?;
            let json = serde_json::to_string_pretty(&obj).map_err(|e| anyhow::anyhow!("{}", e))?;
            println!("{}", json);
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
            RunCmd::List => {
                let (_repo_root, store) = get_store(verbose)?;
                let hashes = store.list(ObjectType::Run)?;
                for h in hashes {
                    println!("{}", h);
                }
            }
            RunCmd::Show { hash, json, with_trace } => {
                let (_repo_root, store) = get_store(verbose)?;
                let hash = parse_hash(&hash)?;
                let obj = store.get(&hash).map_err(|e| anyhow::anyhow!("{}", e))?;
                match &obj {
                    MorphObject::Run(run) => {
                        if json {
                            let out = serde_json::to_string_pretty(run).map_err(|e| anyhow::anyhow!("{}", e))?;
                            println!("{}", out);
                        } else {
                            println!("run    {}", hash);
                            println!("trace  {}", run.trace);
                            println!("pipeline {}", run.pipeline);
                            println!("agent  {} {}", run.agent.id, run.agent.version);
                            if let Some(ref c) = run.commit {
                                println!("commit {}", c);
                            }
                            if !run.metrics.is_empty() {
                                println!("metrics {:?}", run.metrics);
                            }
                        }
                        if with_trace {
                            let trace_hash = parse_hash(&run.trace)?;
                            let trace_obj = store.get(&trace_hash).map_err(|e| anyhow::anyhow!("trace {}: {}", run.trace, e))?;
                            if let MorphObject::Trace(t) = &trace_obj {
                                println!();
                                print_trace_events(t);
                            } else {
                                return Err(anyhow::anyhow!("object {} is not a trace", run.trace));
                            }
                        }
                    }
                    _ => return Err(anyhow::anyhow!("object {} is not a run", hash)),
                }
            }
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
        Command::Trace { sub } => match sub {
            TraceCmd::Show { hash } => {
                let (_repo_root, store) = get_store(verbose)?;
                let hash = parse_hash(&hash)?;
                let obj = store.get(&hash).map_err(|e| anyhow::anyhow!("{}", e))?;
                match &obj {
                    MorphObject::Trace(t) => print_trace_events(t),
                    _ => return Err(anyhow::anyhow!("object {} is not a trace", hash)),
                }
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
        Command::MergePlan { branch, retire } => {
            let (_repo_root, store) = get_store(verbose)?;
            verbose_msg(verbose, &format!("merge-plan for branch {}", branch));
            let retired: Option<Vec<String>> = retire.map(|s| s.split(',').map(|m| m.trim().to_string()).collect());
            let plan = morph_core::prepare_merge(
                &store, &branch, None, retired.as_deref(),
            )?;
            print!("{}", plan.format_plan());
        }
        Command::Merge { branch, message, pipeline, eval_suite, metrics, author, retire } => {
            let (repo_root, store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let version = read_repo_version(&morph_dir)?;
            verbose_msg(verbose, &format!("merge branch {} (pipeline={})", branch, pipeline));
            let prog_hash = parse_hash(&pipeline)?;
            let suite_hash_opt = eval_suite.as_deref().map(parse_hash).transpose()?;
            let observed: std::collections::BTreeMap<String, f64> =
                serde_json::from_str(&metrics).map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?;
            let retired: Option<Vec<String>> = retire.map(|s| s.split(',').map(|m| m.trim().to_string()).collect());
            let plan = morph_core::prepare_merge(
                &store, &branch, suite_hash_opt.as_ref(), retired.as_deref(),
            )?;
            let hash = morph_core::execute_merge(
                &store, &plan, &prog_hash, observed, message, author,
                Some(&repo_root), Some(&version),
            )?;
            println!("{}", hash);
        }
        Command::Rollup { base_ref, tip_ref, message } => {
            let (_repo_root, store) = get_store(verbose)?;
            verbose_msg(verbose, &format!("rollup {}..{}", base_ref, tip_ref));
            let hash = morph_core::rollup(&store, &base_ref, &tip_ref, message)?;
            println!("{}", hash);
        }
        Command::Remote { sub } => match sub {
            RemoteCmd::Add { name, path } => {
                let (repo_root, _store) = get_store(verbose)?;
                let morph_dir = repo_root.join(".morph");
                let abs_path = if path.is_absolute() {
                    path.to_string_lossy().to_string()
                } else {
                    std::env::current_dir()?
                        .join(&path)
                        .to_string_lossy()
                        .to_string()
                };
                morph_core::add_remote(&morph_dir, &name, &abs_path)?;
                println!("Remote '{}' added: {}", name, abs_path);
            }
            RemoteCmd::List => {
                let (repo_root, _store) = get_store(verbose)?;
                let morph_dir = repo_root.join(".morph");
                let remotes = morph_core::read_remotes(&morph_dir)?;
                for (name, spec) in &remotes {
                    println!("{}\t{}", name, spec.path);
                }
            }
        },
        Command::Push { remote, branch } => {
            let (repo_root, local_store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let remotes = morph_core::read_remotes(&morph_dir)?;
            let spec = remotes
                .get(&remote)
                .ok_or_else(|| anyhow::anyhow!("remote '{}' not found. Run 'morph remote add {} <path>' first.", remote, remote))?;
            verbose_msg(verbose, &format!("opening remote store at {}", spec.path));
            let remote_store = morph_core::open_remote_store(&spec.path)?;
            let tip = morph_core::push_branch(local_store.as_ref(), remote_store.as_ref(), &branch)?;
            println!("Pushed {} -> {}/{} ({})", branch, remote, branch, tip);
        }
        Command::Fetch { remote } => {
            let (repo_root, local_store) = get_store(verbose)?;
            let morph_dir = repo_root.join(".morph");
            let remotes = morph_core::read_remotes(&morph_dir)?;
            let spec = remotes
                .get(&remote)
                .ok_or_else(|| anyhow::anyhow!("remote '{}' not found. Run 'morph remote add {} <path>' first.", remote, remote))?;
            verbose_msg(verbose, &format!("fetching from {}", spec.path));
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
            let morph_dir = repo_root.join(".morph");
            let remotes = morph_core::read_remotes(&morph_dir)?;
            let spec = remotes
                .get(&remote)
                .ok_or_else(|| anyhow::anyhow!("remote '{}' not found. Run 'morph remote add {} <path>' first.", remote, remote))?;
            verbose_msg(verbose, &format!("pulling {}/{} from {}", remote, branch, spec.path));
            let remote_store = morph_core::open_remote_store(&spec.path)?;
            let tip = morph_core::pull_branch(local_store.as_ref(), remote_store.as_ref(), &remote, &branch)?;
            println!("Updated {} -> {} ({})", branch, tip, remote);
        }
        Command::Refs => {
            let (_repo_root, store) = get_store(verbose)?;
            let refs = morph_core::list_refs(store.as_ref())?;
            for (name, hash) in &refs {
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
            verbose_msg(verbose, &format!("certifying commit {}", commit_hash));
            let full_path = if metrics_file.is_absolute() { metrics_file } else { repo_root.join(&metrics_file) };
            let metrics_str = std::fs::read_to_string(&full_path)?;
            let metrics: std::collections::BTreeMap<String, f64> =
                serde_json::from_str(&metrics_str).map_err(|e| anyhow::anyhow!("invalid metrics JSON: {}", e))?;
            let result = morph_core::certify_commit(
                &store, &morph_dir, &commit_hash, &metrics,
                runner.as_deref().or(author.as_deref()),
                eval_suite.as_deref(),
            )?;
            if json {
                let out = serde_json::to_string_pretty(&result).map_err(|e| anyhow::anyhow!("{}", e))?;
                println!("{}", out);
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
                Some(ref h) => parse_hash(h)?,
                None => morph_core::resolve_head(&store)?
                    .ok_or_else(|| anyhow::anyhow!("no HEAD commit; specify --commit"))?,
            };
            verbose_msg(verbose, &format!("gate check for commit {}", commit_hash));
            let result = morph_core::gate_check(&store, &morph_dir, &commit_hash)?;
            if json {
                let out = serde_json::to_string_pretty(&result).map_err(|e| anyhow::anyhow!("{}", e))?;
                println!("{}", out);
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
            PolicyCmd::Show => {
                let (repo_root, _store) = get_store(verbose)?;
                let morph_dir = repo_root.join(".morph");
                let policy = morph_core::read_policy(&morph_dir)?;
                let out = serde_json::to_string_pretty(&policy).map_err(|e| anyhow::anyhow!("{}", e))?;
                println!("{}", out);
            }
            PolicyCmd::Set { file } => {
                let (repo_root, _store) = get_store(verbose)?;
                let morph_dir = repo_root.join(".morph");
                let full = if file.is_absolute() { file } else { repo_root.join(&file) };
                let data = std::fs::read_to_string(&full)?;
                let policy: morph_core::RepoPolicy =
                    serde_json::from_str(&data).map_err(|e| anyhow::anyhow!("invalid policy JSON: {}", e))?;
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
