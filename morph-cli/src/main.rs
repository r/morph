//! Morph CLI: read path and manual write operations.

use clap::Parser;
use morph_core::{find_repo, FsStore, Hash, Store};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "morph")]
#[command(about = "Version control for transformation programs")]
struct Cli {
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
    /// Create a commit with program and eval contract
    Commit {
        #[arg(short, long)]
        message: String,
        #[arg(long)]
        program: String,
        #[arg(long)]
        eval_suite: String,
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
}

fn get_store() -> anyhow::Result<(PathBuf, FsStore)> {
    let cwd = std::env::current_dir()?;
    let repo_root = find_repo(&cwd).ok_or_else(|| anyhow::anyhow!("not a morph repository (or any parent)"))?;
    let store = FsStore::new(repo_root.join(".morph"));
    Ok((repo_root, store))
}

fn parse_hash(s: &str) -> anyhow::Result<Hash> {
    Hash::from_hex(s).map_err(|e| anyhow::anyhow!("invalid hash: {}", e))
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { path } => {
            let _store = morph_core::init_repo(&path)?;
            println!("Initialized Morph repository in {}", path.display());
        }
        Command::Prompt { sub } => match sub {
            PromptCmd::Create { path } => {
                let (repo_root, store) = get_store()?;
                let full = if path.is_absolute() { path } else { repo_root.join(&path) };
                let obj = morph_core::blob_from_prompt_file(&full)?;
                let hash = store.put(&obj)?;
                println!("{}", hash);
            }
            PromptCmd::Materialize { hash, output } => {
                let (repo_root, store) = get_store()?;
                let h = parse_hash(&hash)?;
                let dest = output.unwrap_or_else(|| {
                    repo_root.join("prompts").join(format!("{}.prompt", hash))
                });
                morph_core::materialize_blob(&store, &h, &dest)?;
                println!("Materialized to {}", dest.display());
            }
        },
        Command::Program { sub } => match sub {
            ProgramCmd::Create { path } => {
                let (repo_root, store) = get_store()?;
                let full = if path.is_absolute() { path } else { repo_root.join(&path) };
                let obj = morph_core::program_from_file(&full)?;
                let hash = store.put(&obj)?;
                println!("{}", hash);
            }
            ProgramCmd::Show { hash } => {
                let (_repo_root, store) = get_store()?;
                let h = parse_hash(&hash)?;
                let obj = store.get(&h)?;
                let json = serde_json::to_string_pretty(&obj).map_err(|e| anyhow::anyhow!("{}", e))?;
                println!("{}", json);
            }
        },
        Command::Status => {
            let (repo_root, store) = get_store()?;
            let entries = morph_core::status(&store, &repo_root)?;
            if entries.is_empty() {
                println!("No working-space files in prompts/, programs/, or evals/");
                return Ok(());
            }
            for e in entries {
                let status = if e.in_store { "tracked" } else { "new" };
                let hash_str = e.hash.as_ref().map(|h| h.to_string()).unwrap_or_default();
                println!("{} {} {}", status, hash_str, e.path.display());
            }
        }
        Command::Add { paths } => {
            let (repo_root, store) = get_store()?;
            let hashes = morph_core::add_paths(&store, &repo_root, &paths)?;
            for h in hashes {
                println!("{}", h);
            }
        }
        Command::Commit { message, program, eval_suite, metrics, author } => {
            let (_repo_root, store) = get_store()?;
            let store_fs = morph_core::FsStore::new(_repo_root.join(".morph"));
            let prog_hash = parse_hash(&program)?;
            let suite_hash = parse_hash(&eval_suite)?;
            let observed_metrics = metrics
                .as_deref()
                .map(|s| serde_json::from_str(s))
                .transpose()
                .map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?
                .unwrap_or_default();
            let hash = morph_core::create_commit(
                &store,
                &store_fs,
                &prog_hash,
                &suite_hash,
                observed_metrics,
                message,
                author,
            )?;
            println!("{}", hash);
        }
        Command::Log { ref_name } => {
            let (_repo_root, store) = get_store()?;
            let store_fs = morph_core::FsStore::new(_repo_root.join(".morph"));
            let hashes = morph_core::log_from(&store, &store_fs, &ref_name)?;
            for h in hashes {
                let obj = store.get(&h)?;
                if let morph_core::MorphObject::Commit(c) = obj {
                    println!("{} {} {}", h, c.message.lines().next().unwrap_or(""), c.author);
                }
            }
        }
        Command::Branch { name } => {
            let (_repo_root, store) = get_store()?;
            let store_fs = morph_core::FsStore::new(_repo_root.join(".morph"));
            if let Some(branch_name) = name {
                let head = morph_core::resolve_head(&store_fs)?.ok_or_else(|| anyhow::anyhow!("no commit yet; make a commit first"))?;
                store.ref_write(&format!("heads/{}", branch_name), &head)?;
                println!("Created branch {}", branch_name);
            } else {
                let refs_dir = store_fs.refs_dir().join("heads");
                if refs_dir.exists() {
                    for e in std::fs::read_dir(&refs_dir)? {
                        let e = e?;
                        let n = e.file_name().to_string_lossy().into_owned();
                        let current = morph_core::current_branch(&store_fs)?;
                        let mark = if current.as_deref() == Some(&n) { "* " } else { "  " };
                        println!("{}{}", mark, n);
                    }
                }
            }
        }
        Command::Checkout { ref_name } => {
            let (_repo_root, store) = get_store()?;
            let store_fs = morph_core::FsStore::new(_repo_root.join(".morph"));
            if ref_name.len() == 64 && ref_name.chars().all(|c| c.is_ascii_hexdigit()) {
                let hash = parse_hash(&ref_name)?;
                morph_core::set_head_detached(&store_fs, &hash)?;
                println!("Detached HEAD at {}", hash);
            } else {
                let ref_path = if ref_name.starts_with("heads/") { ref_name.clone() } else { format!("heads/{}", ref_name) };
                let _hash = store.ref_read(&ref_path)?.ok_or_else(|| anyhow::anyhow!("branch or ref not found: {}", ref_name))?;
                let branch_name = ref_name.trim_start_matches("heads/").to_string();
                morph_core::set_head_branch(&store_fs, &branch_name)?;
                println!("Switched to branch {}", branch_name);
            }
        }
        Command::Run { sub } => match sub {
            RunCmd::Record { run_file, trace, artifact } => {
                let (_repo_root, store) = get_store()?;
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
        },
        Command::Eval { sub } => match sub {
            EvalCmd::Record { file } => {
                let (_repo_root, _store) = get_store()?;
                let full = if file.is_absolute() { file } else { _repo_root.join(&file) };
                let metrics = morph_core::record_eval_metrics(&full)?;
                let json = serde_json::to_string_pretty(&metrics).map_err(|e| anyhow::anyhow!("{}", e))?;
                println!("{}", json);
            }
        },
        Command::Merge { branch, message, program, eval_suite, metrics, author } => {
            let (_repo_root, store) = get_store()?;
            let store_fs = morph_core::FsStore::new(_repo_root.join(".morph"));
            let prog_hash = parse_hash(&program)?;
            let suite_hash = parse_hash(&eval_suite)?;
            let observed: std::collections::BTreeMap<String, f64> =
                serde_json::from_str(&metrics).map_err(|e| anyhow::anyhow!("invalid --metrics JSON: {}", e))?;
            let hash = morph_core::create_merge_commit(
                &store,
                &store_fs,
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
            let (_repo_root, store) = get_store()?;
            let store_fs = morph_core::FsStore::new(_repo_root.join(".morph"));
            let hash = morph_core::rollup(&store, &store_fs, &base_ref, &tip_ref, message)?;
            println!("{}", hash);
        }
        Command::Annotate { target_hash, kind, data, sub, author } => {
            let (_repo_root, store) = get_store()?;
            let target = parse_hash(&target_hash)?;
            let data_map: std::collections::BTreeMap<String, serde_json::Value> =
                serde_json::from_str(&data).map_err(|e| anyhow::anyhow!("invalid --data JSON: {}", e))?;
            let ann = morph_core::create_annotation(&target, sub, kind, data_map, author);
            let hash = store.put(&ann)?;
            println!("{}", hash);
        }
        Command::Annotations { target_hash, sub } => {
            let (_repo_root, store) = get_store()?;
            let target = parse_hash(&target_hash)?;
            let list = morph_core::list_annotations(&store, &target, sub.as_deref())?;
            for (h, a) in list {
                println!("{} {} {} {}", h, a.kind, a.author, serde_json::to_string(&a.data).unwrap_or_default());
            }
        }
    }
    Ok(())
}
