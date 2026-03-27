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
    pub prompt: String,
    pub response: String,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecordEvalParams {
    pub file: String,
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CommitParams {
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
