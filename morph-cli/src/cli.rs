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
        /// Create a bare repository at `path` (no working tree, no
        /// `.morph/` wrapper). Use this on a server you intend to
        /// `morph push` to via SSH.
        #[arg(long, conflicts_with = "reference")]
        bare: bool,
        /// Initialize in *reference mode*: Morph sits alongside an
        /// existing Git repository. Git owns file storage; Morph
        /// stores only behavioral metadata and mirrors every git
        /// commit into a Morph commit (via the post-commit hook
        /// installed automatically). Requires `path` to be a git
        /// working tree. Mutually exclusive with `--bare`.
        #[arg(long, conflicts_with = "bare")]
        reference: bool,
        /// Skip writing the opinionated default RepoPolicy. Used by
        /// the spec-test harness to keep pre-Phase-2a fixtures
        /// permissive; humans should leave this off so new repos
        /// require behavioral evidence by default.
        #[arg(long, hide = true)]
        no_default_policy: bool,
        /// Initialize reference mode in *Solo submode* — a stronger
        /// contract than the default Stowaway. Solo installs a
        /// `pre-merge-commit` git hook that blocks plain `git merge`
        /// when the merged result would regress on a parent's
        /// certified metrics. Use this only when every developer on
        /// the project uses morph; otherwise teammates' git workflows
        /// can be surprised by a sudden gate. Requires `--reference`.
        #[arg(long, requires = "reference")]
        solo: bool,
    },
    /// Mirror the current Git HEAD into a Morph commit. In reference
    /// mode this is invoked by the installed post-commit hook after
    /// every `git commit`; you can also run it manually to recover
    /// from a missed sync. Errors when the repo is not in reference
    /// mode. The created Morph commit has `morph_origin = "git-hook"`
    /// and empty inline metrics — late certification (`morph certify`)
    /// attaches evidence afterwards.
    #[command(name = "reference-sync")]
    ReferenceSync {
        /// Walk git log from `init_at_git_sha` (inclusive) to HEAD and
        /// mirror every git commit not yet represented in morph. Used
        /// by repos where the post-commit hook is missing or was
        /// disabled while git history grew.
        #[arg(long)]
        backfill: bool,
    },
    /// Idempotently (re-)install reference-mode git hooks. Skips
    /// hooks that already match the canonical script; refuses to
    /// clobber a hook with foreign content. The `--solo` /
    /// `--stowaway` flags also flip the repo's submode (PR 10):
    /// Stowaway (default) installs four passive observers, Solo adds
    /// the active `pre-merge-commit` gate.
    #[command(name = "install-hooks")]
    InstallHooks {
        /// Switch to Solo submode and install the `pre-merge-commit`
        /// hook so plain `git merge` is gated against dominance.
        /// Mutually exclusive with `--stowaway`.
        #[arg(long, conflicts_with = "stowaway")]
        solo: bool,
        /// Switch back to Stowaway submode (the default) and remove
        /// the `pre-merge-commit` hook so plain `git merge` is no
        /// longer gated. Mutually exclusive with `--solo`.
        #[arg(long, conflicts_with = "solo")]
        stowaway: bool,
    },
    /// Internal: dispatch a git hook event into the corresponding
    /// morph handler. Installed hook stubs in `.git/hooks/` exec
    /// `morph hook <event>` so the per-event logic lives in the
    /// binary, not in shell scripts. Hidden from `--help`; users
    /// shouldn't call it directly.
    ///
    /// Events:
    ///   - `post-commit`: mirror HEAD to morph (PR 2 behavior).
    ///   - `post-checkout`: advance morph HEAD when git switches
    ///     branch. Args: `<prev> <new> <flag>`.
    ///   - `post-rewrite`: re-mirror after `amend`/`rebase` and
    ///     flag old commits as rewritten. Args: `<command>`
    ///     (`rebase`|`amend`); reads stdin pairs.
    #[command(name = "hook", hide = true)]
    Hook {
        /// Hook event name (e.g. `post-commit`). Anything not on the
        /// supported list returns a non-zero exit so misbehaving
        /// stubs are caught instead of silently swallowed.
        event: String,
        /// Positional args git passes to the hook. Forwarded
        /// verbatim to the per-event handler.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Print version + build metadata. Like `--version` but also
    /// supports `--json` for scripts and CI smoke tests.
    Version {
        /// Emit a JSON object with `version`, `build_date`,
        /// `protocol_version`, and the supported repo schema
        /// versions. Useful for release pipelines that need to
        /// confirm the binary's identity programmatically.
        #[arg(long)]
        json: bool,
    },
    /// Clone a Morph repository from a local path or SSH URL.
    ///
    /// Initializes a fresh repo at `destination`, configures the
    /// source as `origin`, fetches every branch, and checks out
    /// the default branch. Equivalent to `morph init` +
    /// `morph remote add origin <url>` + `morph fetch origin` +
    /// `morph branch --set-upstream origin/<branch>` +
    /// `morph checkout <branch>` in one step.
    Clone {
        /// Source URL or path: `ssh://user@host/path`,
        /// `user@host:path`, or a local filesystem path.
        url: String,
        /// Destination directory. Defaults to the basename of `url`
        /// with any trailing `.morph` stripped.
        destination: Option<PathBuf>,
        /// Branch to check out. Defaults to the remote's HEAD when
        /// readable (filesystem remotes) or `main` otherwise.
        #[arg(long)]
        branch: Option<String>,
        /// Create a bare clone (no working tree, no `.morph/`
        /// wrapper). Useful for setting up a new server from an
        /// existing repo.
        #[arg(long)]
        bare: bool,
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
    Status {
        /// Emit a structured JSON envelope instead of the human summary.
        #[arg(long)]
        json: bool,
    },
    /// List all tracked and new files in the working directory
    Files {
        /// Emit a JSON array of `{path, status, hash}` entries.
        #[arg(long)]
        json: bool,
    },
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
        /// Bypass the policy.required_metrics gate. Pre-commit
        /// hook still warns; the commit is recorded without
        /// behavioral evidence. Use sparingly.
        #[arg(long)]
        allow_empty_metrics: bool,
        /// Comma-separated acceptance-case ids this commit
        /// introduces. Stored as an `introduces_cases`
        /// annotation; surfaced by merge plans for case
        /// provenance. Pass `""` to suppress auto-detection
        /// (which otherwise diffs the new suite against HEAD's).
        #[arg(long)]
        new_cases: Option<String>,
        /// Disable automatic pickup of the most recent
        /// `morph eval run` (the `.morph/LAST_RUN.json` breadcrumb).
        /// With this flag the commit behaves exactly as if no
        /// breadcrumb existed: metrics + evidence_refs are populated
        /// only from `--metrics` / `--from-run`. Use to record an
        /// audited metrics-less commit even when a fresh run is
        /// available.
        #[arg(long)]
        no_auto_run: bool,
        /// Output structured JSON instead of human-readable summary
        #[arg(long)]
        json: bool,
        /// Reference-mode only: pass `--allow-empty` to the underlying
        /// `git commit`. Lets you record an audit-only morph commit
        /// (e.g. a certification milestone) when there is no staged
        /// diff. Ignored in standalone mode.
        #[arg(long)]
        allow_empty_commit: bool,
    },
    /// Show a stored Morph object (commit, run, trace, etc.) as pretty JSON.
    /// Accepts a full hash, hash prefix (≥4 hex chars), `HEAD`, a branch
    /// name, or a tag name.
    Show {
        hash: String,
    },
    /// Show commit history. Accepts `HEAD`, branches, tags, or hashes.
    /// Defaults to short hashes so output stays scannable; pass
    /// `--full-hash` to restore the long form.
    Log {
        #[arg(default_value = "HEAD")]
        ref_name: String,
        /// Limit the number of commits shown (newest first).
        #[arg(short = 'n', long, value_name = "N")]
        max_count: Option<usize>,
        /// One commit per line: `<short>  <message subject>`.
        #[arg(long)]
        oneline: bool,
        /// Print full 64-char hashes instead of the 8-char short form.
        #[arg(long)]
        full_hash: bool,
        /// Emit a JSON array of commit objects.
        #[arg(long)]
        json: bool,
    },
    /// Show the current HEAD commit (branch, hash, message, author, timestamp).
    Head {
        /// Emit a JSON envelope instead of the human summary.
        #[arg(long)]
        json: bool,
    },
    /// Resolve any revision (ref / hash / prefix) to the full hash and
    /// the type of object it points at. Useful for scripts that need
    /// to know "what does HEAD mean right now?".
    Identify {
        /// Revision to resolve. Accepts the same forms as `morph show`.
        revision: String,
        /// Emit a JSON envelope instead of just the hash.
        #[arg(long)]
        json: bool,
    },
    /// Create or list branches
    Branch {
        name: Option<String>,
        /// Configure the branch's upstream tracking ref, e.g.
        /// `--set-upstream origin/main`. Used by `morph sync`.
        #[arg(long, value_name = "REMOTE/BRANCH")]
        set_upstream: Option<String>,
        /// Emit a JSON envelope listing every branch and the current one.
        #[arg(long)]
        json: bool,
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
    /// Merge a branch (behavioral dominance required by default).
    ///
    /// Flow:
    ///   morph merge <branch>             # start a merge
    ///   morph merge --continue           # finalize after resolving conflicts
    ///   morph merge --abort              # discard an in-progress merge
    ///   morph merge resolve-node <id> --pick ours|theirs|base
    ///                                    # pick a side for one pipeline-node conflict
    ///
    /// The single-shot form (start + finalize in one go) keeps the
    /// pre-PR4 ergonomics: when `<branch>` is supplied and the user
    /// also passes `--pipeline` and `--metrics`, a clean three-way
    /// merge is finalized immediately. Conflicting merges always
    /// drop into the state machine and require an explicit
    /// `--continue` once the user has resolved every conflict.
    Merge {
        /// Branch to merge into HEAD. Omit when using `--continue`
        /// or `--abort`.
        branch: Option<String>,

        /// Finalize an in-progress merge. Reads `MERGE_HEAD`,
        /// `MERGE_MSG`, and the staging index, then creates the
        /// merge commit. Errors out if any unmerged paths or
        /// pipeline-node conflicts remain.
        #[arg(long = "continue", conflicts_with_all = ["abort", "branch"])]
        cont: bool,

        /// Discard an in-progress merge. Restores the working tree
        /// to `ORIG_HEAD` and clears `MERGE_*` state. Errors when
        /// no merge is in progress.
        #[arg(long, conflicts_with_all = ["cont", "branch", "message", "pipeline", "metrics", "eval_suite", "retire", "retire_reason"])]
        abort: bool,

        /// Optional commit message. Required for the single-shot
        /// form; used as override for `--continue` (default reads
        /// `.morph/MERGE_MSG`).
        #[arg(short, long)]
        message: Option<String>,
        /// Pipeline hash. Required for the single-shot form.
        /// Optional on `--continue`; when absent, the pipeline
        /// stored in `.morph/MERGE_PIPELINE.json` (or, if missing,
        /// HEAD's pipeline) is used.
        #[arg(long)]
        pipeline: Option<String>,
        /// Eval suite hash override. Optional in every form.
        #[arg(long)]
        eval_suite: Option<String>,
        /// Observed metrics as JSON object. Required for the
        /// single-shot form. On `--continue` the merged metrics are
        /// synthesized from both parents.
        #[arg(long)]
        metrics: Option<String>,
        /// Override the commit author for this merge.
        #[arg(long)]
        author: Option<String>,
        /// Comma-separated list of metric names to retire from the
        /// dominance check. Useful when the pipeline's contract
        /// genuinely changed.
        #[arg(long)]
        retire: Option<String>,
        /// Reason for retiring metrics (paper §4.3 attribution). Recorded
        /// on the auto-injected `review` node. Ignored without `--retire`.
        /// When omitted, a generic placeholder is used; supplying a real
        /// reason makes the retirement auditable later.
        #[arg(long)]
        retire_reason: Option<String>,

        /// Subcommand-style operations on an in-progress merge.
        #[command(subcommand)]
        sub: Option<MergeCmd>,
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
    /// Fetch + pull --merge (or fast-forward) the current branch's
    /// configured upstream. Configure the upstream once with
    /// `morph branch --set-upstream origin/main`, then run
    /// `morph sync` from any session to bring the branch up to date.
    Sync {
        /// Optional branch name; defaults to the current branch.
        branch: Option<String>,
    },
    /// Read or write a configuration value in `.morph/config.json`.
    ///
    /// Supported keys (PR 6 stage A): `user.name`, `user.email`.
    /// Future PRs will add policy / agent / branch keys here.
    ///
    /// Forms:
    ///   morph config <key> <value>     # set
    ///   morph config <key>             # get (prints value, empty + exit 1 if unset)
    ///   morph config --get <key>       # explicit get (same as positional get)
    Config {
        /// Dotted key, e.g. `user.name`.
        key: String,
        /// New value. If absent, prints the current value.
        value: Option<String>,
        /// Explicit "get" form for parity with `git config --get`.
        #[arg(long)]
        get: bool,
    },
    /// List all refs (local branches and remote-tracking refs)
    Refs {
        /// Emit a JSON envelope instead of `<hash>\t<name>` lines.
        #[arg(long)]
        json: bool,
    },
    /// Certify a commit using externally produced metrics.
    ///
    /// Provide metrics via `--metrics-file <path>` (JSON file) or
    /// `--metrics '<json>'` (inline JSON object). Exactly one of
    /// the two is required; specifying both is an error.
    Certify {
        /// Inline JSON object of metric name → number, e.g.
        /// `--metrics '{"acc":0.95,"tests_passed":42}'`.
        #[arg(long, conflicts_with = "metrics_file")]
        metrics: Option<String>,
        /// Path to a JSON file containing a metric name → number map.
        #[arg(long)]
        metrics_file: Option<PathBuf>,
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
    /// List annotations on an object. The target accepts `HEAD`, branches,
    /// tags, full hashes, or hash prefixes.
    Annotations {
        target_hash: String,
        #[arg(long)]
        sub: Option<String>,
        /// Emit a JSON envelope listing every annotation.
        #[arg(long)]
        json: bool,
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
        /// Emit a JSON envelope: `{from, to, changes: [{status, path}]}`.
        #[arg(long)]
        json: bool,
    },
    /// Create, list, or delete tags
    Tag {
        name: Option<String>,
        #[arg(short, long)]
        delete: bool,
        /// When listing, emit a JSON envelope instead of `<name> <hash>` lines.
        #[arg(long)]
        json: bool,
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
    /// Hidden JSON-RPC server for SSH-driven sync. Spawned by
    /// `SshStore` over `ssh user@host morph remote-helper
    /// --repo-root <path>`. Reads one JSON request per line from
    /// stdin, writes one JSON response per line to stdout. Exits 0
    /// on EOF.
    #[command(name = "remote-helper", hide = true)]
    RemoteHelper {
        #[arg(long)]
        repo_root: PathBuf,
    },
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
pub enum MergeCmd {
    /// Pick a side for a single pipeline-node conflict surfaced by
    /// `morph merge <branch>`. `--pick` accepts `ours`, `theirs`,
    /// or `base`. The chosen node is written into
    /// `.morph/MERGE_PIPELINE.json`; once every conflict is
    /// resolved, finalize with `morph merge --continue`.
    ResolveNode {
        /// Pipeline-node id (matches `pipeline.graph.nodes[*].id`).
        node: String,
        /// Side to pick: `ours`, `theirs`, or `base`.
        #[arg(long)]
        pick: String,
    },
}

#[derive(clap::Subcommand)]
pub enum TapCmd {
    /// Show summary statistics for all traces in the repo
    Summary {
        /// Emit a JSON envelope with the summary fields.
        #[arg(long)]
        json: bool,
    },
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
    /// List recorded runs.
    List {
        /// Emit a JSON envelope listing every run hash.
        #[arg(long)]
        json: bool,
    },
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
    /// Record a metrics JSON file as an eval-result blob plus a Run.
    Record { file: PathBuf },
    /// Parse a captured stdout file from a test runner and emit the
    /// resulting metrics map. Composes with `morph eval record`,
    /// `morph commit --metrics`, and `morph_commit` MCP.
    FromOutput {
        /// Which runner produced the file. Defaults to `auto`,
        /// which sniffs based on content. Pass an explicit value
        /// when the output is ambiguous (e.g. mixed runners in CI).
        #[arg(long, default_value = "auto")]
        runner: String,
        /// Path to a file containing the runner's stdout (and
        /// optionally stderr). `-` reads from standard input.
        file: PathBuf,
        /// Also create a Run object pointing at HEAD with these
        /// metrics. The hash is printed on stdout so it composes
        /// with `morph commit --from-run <hash>`.
        #[arg(long)]
        record: bool,
    },
    /// Execute a test command, capture its output, parse metrics, and
    /// store a Run object linked to HEAD. Prints the run hash so the
    /// caller can `morph commit --from-run <hash>`.
    Run {
        /// Runner family to use for parsing. `auto` (default) sniffs
        /// from the command and output.
        #[arg(long, default_value = "auto")]
        runner: String,
        /// Working directory for the test command. Defaults to the
        /// repository root.
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// The test command and its arguments. Use `--` to separate
        /// from `morph eval run`'s own flags, e.g.
        /// `morph eval run -- cargo test --workspace`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Phase 4b: ingest one or more YAML specs / cucumber `.feature`
    /// files as acceptance cases. Updates the repo's default suite
    /// (or `--suite <hash>`) by appending and deduping by case id.
    ///
    /// Print the new suite hash on stdout so callers can pipe into
    /// `morph commit --eval-suite <hash>` if they don't want to use
    /// the policy default.
    AddCase {
        /// Files or directories to ingest. Directories are walked
        /// one level deep.
        paths: Vec<PathBuf>,
        /// Existing suite to extend. Defaults to
        /// `policy.default_eval_suite`. Pass `--no-default` to
        /// build a fresh suite.
        #[arg(long)]
        suite: Option<String>,
        /// Build a fresh suite even if the policy already has a
        /// default. Useful when starting over after a refactor.
        #[arg(long)]
        no_default: bool,
        /// Skip updating `policy.default_eval_suite`. By default
        /// the new suite hash is recorded so subsequent commits
        /// pick it up automatically.
        #[arg(long)]
        no_set_default: bool,
    },
    /// Convenience wrapper for `morph eval add-case`: walks the
    /// supplied directories, ingests every `*.yaml`/`*.yml`/`*.feature`,
    /// and replaces the default suite with the result.
    SuiteFromSpecs {
        /// One or more directories (or files) to ingest.
        paths: Vec<PathBuf>,
        /// Skip updating `policy.default_eval_suite`.
        #[arg(long)]
        no_set_default: bool,
    },
    /// Print the contents of the default suite (or `--suite <hash>`)
    /// in human-readable form.
    SuiteShow {
        /// Suite hash to inspect. Defaults to
        /// `policy.default_eval_suite`.
        #[arg(long)]
        suite: Option<String>,
        /// Emit JSON instead of the human summary.
        #[arg(long)]
        json: bool,
    },
    /// Phase 5b: report behavioral evidence gaps in this repo, in
    /// the same form as the `morph_eval_gaps` MCP tool. Use this in
    /// stop-hooks to short-circuit a session that is about to
    /// commit without evidence.
    Gaps {
        /// Emit JSON for downstream tooling.
        #[arg(long)]
        json: bool,
        /// Exit non-zero when at least one gap is reported. Useful
        /// in CI / git hooks.
        #[arg(long)]
        fail_on_gap: bool,
    },
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
    List {
        /// Emit a JSON envelope with every configured remote.
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Subcommand)]
pub enum PolicyCmd {
    /// Write a default RepoPolicy if one is not already present.
    /// New repos get this automatically; existing repos use this
    /// to opt into behavioral merge gating without breaking history.
    Init {
        /// Overwrite an existing policy. Without this flag, `init`
        /// is a no-op when a policy is already configured.
        #[arg(long)]
        force: bool,
    },
    Show,
    Set { file: PathBuf },
    SetDefaultEval { hash: String },
    /// Replace `policy.required_metrics` with the supplied list.
    /// Pass an empty list to disable the gate; existing thresholds
    /// and default-suite reference are preserved.
    RequireMetrics {
        metrics: Vec<String>,
    },
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
