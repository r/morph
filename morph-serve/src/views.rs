//! Stable view models for the Morph hosted service API.
//!
//! These types are the public response shapes. They mirror the core object model
//! but add derived behavioral information and are decoupled from internal storage.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── Repository ──────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct RepoListResponse {
    pub repos: Vec<RepoSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepoSummary {
    pub name: String,
    pub head: Option<String>,
    pub current_branch: Option<String>,
    pub branch_count: usize,
    pub commit_count: usize,
    pub run_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BranchListResponse {
    pub branches: Vec<BranchInfo>,
    pub current: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub head: Option<String>,
}

// ── Commits ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct CommitListResponse {
    pub commits: Vec<CommitSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommitSummary {
    pub hash: String,
    pub message: String,
    pub author: String,
    pub timestamp: String,
    pub parents: Vec<String>,
    pub has_tree: bool,
    pub morph_version: Option<String>,
    pub metric_count: usize,
    pub is_merge: bool,
    pub certified: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommitDetailResponse {
    pub hash: String,
    pub message: String,
    pub author: String,
    pub timestamp: String,
    pub parents: Vec<String>,
    pub pipeline: String,
    pub tree: Option<String>,
    pub morph_version: Option<String>,
    pub eval_contract: EvalContractView,
    pub contributors: Option<Vec<ContributorView>>,
    pub evidence_refs: Option<Vec<String>>,
    pub env_constraints: Option<BTreeMap<String, serde_json::Value>>,
    pub behavioral_status: BehavioralStatus,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EvalContractView {
    pub suite: String,
    pub observed_metrics: BTreeMap<String, f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ContributorView {
    pub id: String,
    pub role: Option<String>,
}

// ── Behavioral status ───────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct BehavioralStatus {
    pub certified: bool,
    pub certification: Option<CertificationView>,
    pub gate_passed: Option<bool>,
    pub gate_reasons: Vec<String>,
    pub is_merge: bool,
    pub merge_status: Option<MergeStatusView>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CertificationView {
    pub passed: bool,
    pub runner: Option<String>,
    pub eval_suite: Option<String>,
    pub metrics: BTreeMap<String, f64>,
    pub failures: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MergeStatusView {
    pub parent_a: String,
    pub parent_b: String,
    pub merged_metrics: BTreeMap<String, f64>,
    pub parent_a_metrics: BTreeMap<String, f64>,
    pub parent_b_metrics: BTreeMap<String, f64>,
    pub dominates_a: Option<bool>,
    pub dominates_b: Option<bool>,
}

// ── Runs ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct RunListResponse {
    pub runs: Vec<RunSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunSummary {
    pub hash: String,
    pub trace: String,
    pub pipeline: String,
    pub agent: String,
    pub metric_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunDetailResponse {
    pub hash: String,
    pub pipeline: String,
    pub commit: Option<String>,
    pub trace: String,
    pub agent: AgentView,
    pub environment: EnvironmentView,
    pub metrics: BTreeMap<String, f64>,
    pub output_artifacts: Vec<String>,
    pub contributors: Option<Vec<RunContributorView>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentView {
    pub id: String,
    pub version: String,
    pub instance_id: Option<String>,
    pub policy: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EnvironmentView {
    pub model: String,
    pub version: String,
    pub parameters: BTreeMap<String, serde_json::Value>,
    pub toolchain: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunContributorView {
    pub id: String,
    pub version: String,
    pub role: Option<String>,
}

// ── Traces ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct TraceDetailResponse {
    pub hash: String,
    pub events: Vec<TraceEventView>,
    pub event_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TraceEventView {
    pub id: String,
    pub seq: u64,
    pub ts: String,
    pub kind: String,
    pub payload: BTreeMap<String, serde_json::Value>,
}

// ── Pipelines ───────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct PipelineDetailResponse {
    pub hash: String,
    pub node_count: usize,
    pub edge_count: usize,
    pub prompts: Vec<String>,
    pub eval_suite: Option<String>,
    pub attribution: Option<BTreeMap<String, serde_json::Value>>,
    pub provenance: Option<ProvenanceView>,
    pub graph: PipelineGraphView,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PipelineGraphView {
    pub nodes: Vec<PipelineNodeView>,
    pub edges: Vec<PipelineEdgeView>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PipelineNodeView {
    pub id: String,
    pub kind: String,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
    pub params: BTreeMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PipelineEdgeView {
    pub from: String,
    pub to: String,
    pub kind: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProvenanceView {
    pub derived_from_run: Option<String>,
    pub derived_from_trace: Option<String>,
    pub derived_from_event: Option<String>,
    pub method: String,
}

// ── Annotations ─────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct AnnotationsResponse {
    pub target: String,
    pub annotations: Vec<AnnotationView>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AnnotationView {
    pub hash: String,
    pub kind: String,
    pub data: BTreeMap<String, serde_json::Value>,
    pub author: String,
    pub timestamp: String,
    pub target_sub: Option<String>,
}

// ── Policy ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct PolicyResponse {
    pub repo_policy: RepoPolicyView,
    pub org_policy: Option<OrgPolicyView>,
    pub effective_required_metrics: Vec<String>,
    pub effective_thresholds: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoPolicyView {
    pub required_metrics: Vec<String>,
    pub thresholds: BTreeMap<String, f64>,
    pub directions: BTreeMap<String, String>,
    pub default_eval_suite: Option<String>,
    pub merge_policy: String,
    pub ci_defaults: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgPolicyView {
    pub required_metrics: Vec<String>,
    pub thresholds: BTreeMap<String, f64>,
    pub directions: BTreeMap<String, String>,
    pub presets: BTreeMap<String, PolicyPresetView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyPresetView {
    pub required_metrics: Vec<String>,
    pub thresholds: BTreeMap<String, f64>,
}

// ── Gate status ─────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct GateStatusResponse {
    pub passed: bool,
    pub commit: String,
    pub reasons: Vec<String>,
    pub metrics: BTreeMap<String, f64>,
    pub policy: RepoPolicyView,
}

// ── Error ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
}
