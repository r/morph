//! MCP tool parameter structs.

use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct WorkspaceOnlyParams {
    #[serde(default)]
    pub workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LogParams {
    #[serde(default)]
    pub ref_name: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShowParams {
    pub hash: String,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiffParams {
    pub old_ref: String,
    pub new_ref: String,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MergeParams {
    pub branch: String,
    pub message: String,
    pub pipeline: String,
    #[serde(default)]
    pub metrics: std::collections::BTreeMap<String, f64>,
    #[serde(default)]
    pub eval_suite: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub retire: Option<String>,
    /// Human-readable reason for retiring metrics (paper §4.3
    /// attribution). Recorded on the auto-injected `review` node in the
    /// merged pipeline. Ignored when `retire` is empty; defaults to a
    /// generic placeholder when omitted alongside a non-empty `retire`.
    #[serde(default)]
    pub retire_reason: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct InitParams {
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecordRunParams {
    pub run_file: String,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub trace_file: Option<String>,
    #[serde(default)]
    pub artifact_files: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecordSessionParams {
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub response: Option<String>,
    /// Full conversation as array of {role, content} objects. When provided, prompt/response are ignored.
    #[serde(default)]
    pub messages: Option<Vec<MessageParam>>,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MessageParam {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub metadata: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    #[serde(default)]
    pub timestamp: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecordEvalParams {
    pub file: String,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

/// Phase 3b: parse a captured stdout buffer from a known test
/// runner. Used to attach behavioral evidence to a commit without
/// re-running the tests.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct EvalFromOutputParams {
    /// Captured stdout (and optionally stderr) from the test run.
    pub stdout: String,
    /// Optional runner family hint (cargo, pytest, vitest, jest,
    /// go, or auto). Defaults to auto.
    #[serde(default)]
    pub runner: Option<String>,
    /// Original CLI command that produced `stdout`. Used by the
    /// auto-detector and stored on the resulting Run for audit.
    #[serde(default)]
    pub command: Option<String>,
    /// When true, also persist a Run object linked to HEAD with
    /// the parsed metrics. The response is then the run hash,
    /// suitable for `morph_commit { from_run }`.
    #[serde(default)]
    pub record: Option<bool>,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

/// Phase 3b: execute a test command and persist a metrics-bearing
/// Run linked to HEAD.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct EvalRunParams {
    /// argv of the test command, e.g. `["cargo","test","--workspace"]`.
    pub command: Vec<String>,
    /// Optional runner family hint.
    #[serde(default)]
    pub runner: Option<String>,
    /// Working directory; relative paths resolve from the repo
    /// root. Defaults to the repo root.
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

/// Phase 4b: extend (or build) an EvalSuite from one or more YAML
/// specs / cucumber `.feature` files. Mirrors `morph eval add-case`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddEvalCaseParams {
    /// Files or directories to ingest (paths relative to the repo
    /// root unless absolute).
    pub paths: Vec<String>,
    /// Existing suite to extend. Defaults to
    /// `policy.default_eval_suite`.
    #[serde(default)]
    pub suite: Option<String>,
    /// Build a fresh suite, ignoring any `default_eval_suite`.
    #[serde(default)]
    pub no_default: Option<bool>,
    /// Skip updating `policy.default_eval_suite` with the new hash.
    #[serde(default)]
    pub no_set_default: Option<bool>,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

/// Phase 4b: bulk-ingest a directory tree into a fresh suite and
/// (by default) make it the policy's default. Mirrors
/// `morph eval suite-from-specs`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct EvalSuiteFromSpecsParams {
    /// Files or directories to ingest.
    pub paths: Vec<String>,
    #[serde(default)]
    pub no_set_default: Option<bool>,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

/// Phase 4b: introspect the current default suite (or `suite`).
#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct EvalSuiteShowParams {
    #[serde(default)]
    pub suite: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StageParams {
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub paths: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct CommitParams {
    #[serde(default)]
    pub message: String,
    #[serde(default, alias = "program")]
    pub pipeline: Option<String>,
    #[serde(default)]
    pub eval_suite: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub metrics: Option<std::collections::BTreeMap<String, f64>>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub from_run: Option<String>,
    /// When true, bypass the policy.required_metrics gate. The
    /// resulting response still includes a warning so the empty
    /// commit is visible to the caller.
    #[serde(default)]
    pub allow_empty_metrics: Option<bool>,
    /// Comma-separated acceptance-case ids this commit
    /// introduces. Recorded as an `introduces_cases` annotation.
    #[serde(default)]
    pub new_cases: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AnnotateParams {
    pub target_hash: String,
    pub kind: String,
    #[serde(default)]
    pub data: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    #[serde(default)]
    pub target_sub: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BranchParams {
    pub name: String,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckoutParams {
    pub ref_name: String,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct RecentTracesParams {
    /// Maximum number of traces to return (default 10).
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TraceHashParams {
    /// Run hash, or trace hash (will be resolved to the latest run
    /// pointing to that trace).
    pub hash: String,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

/// `morph_identify`: resolve a user-supplied revision to its full
/// hash and object type. Mirrors `morph identify <rev>` in the CLI.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct IdentifyParams {
    /// Any revision: full hash, prefix (>=4 hex), `HEAD`, branch
    /// name, or tag name.
    pub revision: String,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

/// `morph_reference_sync`: mirror git history into Morph commits.
/// Either appends one commit (HEAD-only sync) or walks the full
/// `init_at_git_sha..HEAD` range (`backfill: true`).
#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ReferenceSyncParams {
    /// When true, walk every git commit in `init_at_git_sha..HEAD`
    /// and synthesise any not yet mirrored. When false (the
    /// default), mirror only the current git HEAD.
    #[serde(default)]
    pub backfill: Option<bool>,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

/// `morph_annotations`: list every annotation attached to a target.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AnnotationsParams {
    /// Target hash or ref (`HEAD`, branch name, tag name, prefix).
    pub target_hash: String,
    /// Optional sub-id within the target (e.g. a pipeline node id).
    #[serde(default)]
    pub target_sub: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
}
