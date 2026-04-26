//! CLI command definitions (clap derive).

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "morph")]
#[command(about = "Version control for transformation pipelines")]
#[command(version = long_version())]
pub struct Cli {
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

fn long_version() -> &'static str {
    const V: &str = concat!(
        env!("CARGO_PKG_VERSION"),
        " (built ",
        env!("MORPH_BUILD_DATE"),
        ")"
    );
    V
}

#[derive(clap::Subcommand)]
pub enum Command {
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
    /// Show changes relative to last commit (git-style status)
    Status,
    /// List all tracked and new files in the working directory
    Files,
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
        #[arg(long)]
        from_run: Option<String>,
        /// Output structured JSON instead of human-readable summary
        #[arg(long)]
        json: bool,
    },
    /// Show a stored Morph object (commit, run, trace, etc.) as pretty JSON
    Show {
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
    /// Preview merge requirements
    MergePlan {
        branch: String,
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
        #[arg(long)]
        eval_suite: Option<String>,
        #[arg(long)]
        metrics: String,
        #[arg(long)]
        author: Option<String>,
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
        remote: String,
        branch: String,
    },
    /// Fetch branches from a remote into remote-tracking refs
    Fetch {
        remote: String,
    },
    /// Pull: fetch from remote + fast-forward local branch
    Pull {
        remote: String,
        branch: String,
        /// On divergence, start a merge instead of erroring out.
        /// Mirrors `git pull --no-ff` ergonomics — you'll resolve any
        /// conflicts and run `morph merge --continue`.
        #[arg(long)]
        merge: bool,
    },
    /// List all refs (local branches and remote-tracking refs)
    Refs,
    /// Certify a commit using externally produced metrics
    Certify {
        #[arg(long)]
        metrics_file: PathBuf,
        #[arg(long)]
        commit: Option<String>,
        #[arg(long)]
        eval_suite: Option<String>,
        #[arg(long)]
        runner: Option<String>,
        #[arg(long)]
        author: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Check whether a commit satisfies the project's behavioral policy
    Gate {
        #[arg(long)]
        commit: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Manage repository behavioral policy
    Policy {
        #[command(subcommand)]
        sub: PolicyCmd,
    },
    /// Attach an annotation to an object
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
    /// List annotations on an object
    Annotations {
        target_hash: String,
        #[arg(long)]
        sub: Option<String>,
    },
    /// Read a Morph object from JSON, store it, print its content hash
    HashObject {
        path: PathBuf,
    },
    /// Set up IDE integration
    #[cfg(feature = "cursor-setup")]
    Setup {
        #[command(subcommand)]
        sub: SetupCmd,
    },
    /// Compare two commits and show file-level changes
    Diff {
        old_ref: String,
        #[arg(default_value = "HEAD")]
        new_ref: String,
    },
    /// Create, list, or delete tags
    Tag {
        name: Option<String>,
        #[arg(short, long)]
        delete: bool,
    },
    /// Stash staged changes
    Stash {
        #[command(subcommand)]
        sub: StashCmd,
    },
    /// Create a new commit that undoes a previous commit's changes
    Revert {
        commit: String,
        #[arg(long)]
        author: Option<String>,
    },
    /// Upgrade the repo store to the latest version
    Upgrade,
    /// Remove unreachable objects from the store
    Gc,
    /// Inspect traces
    Trace {
        #[command(subcommand)]
        sub: TraceCmd,
    },
    /// Extract and analyze traces for evaluation
    Tap {
        #[command(subcommand)]
        sub: TapCmd,
    },
    /// Structured trace views for replay / eval generation
    Traces {
        #[command(subcommand)]
        sub: TracesCmd,
    },
    /// Browse repo in browser
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
    /// Run the Morph hosted service
    #[cfg(feature = "visualize")]
    Serve {
        #[arg(long = "repo", value_name = "NAME=PATH")]
        repos: Vec<String>,
        #[arg(long, default_value = "8765")]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        interface: String,
        #[arg(long)]
        org_policy: Option<PathBuf>,
    },
}

#[derive(clap::Subcommand)]
pub enum TraceCmd {
    Show { hash: String },
}

#[derive(clap::Subcommand)]
pub enum TapCmd {
    /// Show summary statistics for all traces in the repo
    Summary,
    /// Inspect a single run/trace and show extracted steps
    Inspect {
        /// Run hash to inspect (or "all" for every run)
        run_hash: String,
    },
    /// Diagnose recording quality for a run or all runs
    Diagnose {
        /// Run hash to diagnose (or "all" for every run)
        #[arg(default_value = "all")]
        run_hash: String,
    },
    /// Export traces as evaluation cases (JSON)
    Export {
        /// Export mode: prompt-only, with-context, agentic
        #[arg(long, default_value = "with-context")]
        mode: String,
        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
        /// Filter by model name (substring match)
        #[arg(long)]
        model: Option<String>,
        /// Filter by agent id (substring match)
        #[arg(long)]
        agent: Option<String>,
        /// Only include runs with at least N steps
        #[arg(long)]
        min_steps: Option<usize>,
    },
    /// Show detailed statistics for a single trace
    TraceStats {
        /// Trace hash to inspect
        trace_hash: String,
    },
    /// Preview how a run would be exported (labeled sections)
    Preview {
        /// Run hash to preview
        run_hash: String,
        /// Export mode to preview: prompt-only, with-context, agentic
        #[arg(long, default_value = "agentic")]
        mode: String,
    },
}

#[derive(clap::Subcommand)]
pub enum TracesCmd {
    /// Browse recent traces with structured summaries (newest first)
    Summary {
        /// Maximum number of traces to show
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Output JSON instead of human-readable
        #[arg(long)]
        json: bool,
    },
    /// Show the task structure (phase, scope, target files/symbols, task_goal)
    TaskStructure {
        /// Run hash (or trace hash)
        hash: String,
    },
    /// Show the target file/function context for replay or eval
    TargetContext {
        hash: String,
    },
    /// Show the final artifact produced by the agent
    FinalArtifact {
        hash: String,
    },
    /// Show change / preserved / restored semantic summaries
    Semantics {
        hash: String,
    },
    /// Show verification commands/tests/demo steps
    Verification {
        hash: String,
    },
}

#[derive(clap::Subcommand)]
pub enum StashCmd {
    Save {
        #[arg(short, long)]
        message: Option<String>,
    },
    Pop,
    List,
}

#[cfg(feature = "cursor-setup")]
#[derive(clap::Subcommand)]
pub enum SetupCmd {
    /// Install Cursor hooks, MCP config, and rules
    Cursor {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// Install OpenCode MCP config, AGENTS.md, and recording plugin
    Opencode {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
}

#[derive(clap::Subcommand)]
pub enum RunCmd {
    List,
    Show {
        hash: String,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        with_trace: bool,
    },
    Record {
        run_file: PathBuf,
        #[arg(long)]
        trace: Option<PathBuf>,
        #[arg(long)]
        artifact: Vec<PathBuf>,
    },
    RecordSession {
        #[arg(long, required_unless_present = "messages")]
        prompt: Option<String>,
        #[arg(long, required_unless_present = "messages")]
        response: Option<String>,
        /// JSON array of messages: [{"role":"user","content":"..."},{"role":"assistant","content":"..."},...]
        #[arg(long, conflicts_with_all = ["prompt", "response"])]
        messages: Option<String>,
        #[arg(long)]
        model_name: Option<String>,
        #[arg(long)]
        agent_id: Option<String>,
    },
}

#[derive(clap::Subcommand)]
pub enum EvalCmd {
    Record { file: PathBuf },
}

#[derive(clap::Subcommand)]
pub enum PromptCmd {
    Create { path: PathBuf },
    Materialize {
        hash: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    Show {
        #[arg(default_value = "latest")]
        run_ref: String,
        #[arg(long)]
        run_upgrade: bool,
    },
}

#[derive(clap::Subcommand)]
pub enum RemoteCmd {
    Add { name: String, path: PathBuf },
    List,
}

#[derive(clap::Subcommand)]
pub enum PolicyCmd {
    Show,
    Set { file: PathBuf },
    SetDefaultEval { hash: String },
}

#[derive(clap::Subcommand)]
pub enum PipelineCmd {
    Create { path: PathBuf },
    Show { hash: String },
    IdentityHash,
    Extract {
        #[arg(long)]
        from_run: String,
    },
}
