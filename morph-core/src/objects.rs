//! Immutable content-addressed object types (v0-spec §4).
//! All map-like fields use BTreeMap for deterministic canonical JSON.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------- 4.1 Blob ----------
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    pub kind: String,
    pub content: serde_json::Value,
}

// ---------- 4.2 Tree ----------
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tree {
    pub entries: Vec<TreeEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub name: String,
    pub hash: String,
    #[serde(default = "default_entry_type")]
    pub entry_type: String,
}

fn default_entry_type() -> String {
    "blob".to_string()
}

// ---------- 4.3 Pipeline ----------
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pipeline {
    pub graph: PipelineGraph,
    pub prompts: Vec<String>,
    #[serde(default)]
    pub eval_suite: Option<String>,
    #[serde(default)]
    pub attribution: Option<BTreeMap<String, AttributionEntry>>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
}

/// Attribution entry for a pipeline node. Maps to the paper's α : V → 2^A.
/// `actors` is a set of Actor IDs that contributed to this node.
/// The legacy `agent_id` field is kept for backward compatibility; when both
/// are present, `actors` takes precedence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributionEntry {
    pub agent_id: String,
    #[serde(default)]
    pub agent_version: Option<String>,
    #[serde(default)]
    pub instance_id: Option<String>,
    #[serde(default)]
    pub actors: Option<Vec<ActorRef>>,
}

/// Reference to an Actor (paper §3.2). Actors are humans, agents, or pairs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorRef {
    pub id: String,
    #[serde(default = "default_actor_type")]
    pub actor_type: String,
    #[serde(default)]
    pub env_config: Option<BTreeMap<String, serde_json::Value>>,
}

fn default_actor_type() -> String {
    "agent".to_string()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineGraph {
    pub nodes: Vec<PipelineNode>,
    pub edges: Vec<PipelineEdge>,
}

/// A node in the pipeline DAG. `kind` is one of: prompt_call, tool_call,
/// retrieval, transform, identity, review (paper §3.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineNode {
    pub id: String,
    pub kind: String,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none", default)]
    pub ref_: Option<String>,
    #[serde(default)]
    pub params: BTreeMap<String, serde_json::Value>,
    /// Per-node environment config (paper ε : V → EnvConfig ∪ {⊥}).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub env: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    #[serde(default)]
    pub derived_from_run: Option<String>,
    #[serde(default)]
    pub derived_from_trace: Option<String>,
    #[serde(default)]
    pub derived_from_event: Option<String>,
    pub method: String,
}

// ---------- 4.4 EvalSuite ----------
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvalSuite {
    pub cases: Vec<EvalCase>,
    pub metrics: Vec<EvalMetric>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvalCase {
    pub id: String,
    pub input: serde_json::Value,
    pub expected: serde_json::Value,
    pub metric: String,
    /// Where test data comes from: "candidate", "base", "pinned", or "external".
    /// See THEORY.md §10.3. Defaults to "candidate" (use the produced tree).
    #[serde(default = "default_fixture_source")]
    pub fixture_source: String,
}

fn default_fixture_source() -> String {
    "candidate".to_string()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvalMetric {
    pub name: String,
    pub aggregation: String,
    pub threshold: f64,
    /// Ordering direction: "maximize" (default) or "minimize". See THEORY.md §10.4.
    #[serde(default = "default_direction")]
    pub direction: String,
}

fn default_direction() -> String {
    "maximize".to_string()
}

impl EvalMetric {
    pub fn new(name: impl Into<String>, aggregation: impl Into<String>, threshold: f64) -> Self {
        EvalMetric {
            name: name.into(),
            aggregation: aggregation.into(),
            threshold,
            direction: default_direction(),
        }
    }
}

// ---------- 4.5 Commit ----------
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Commit {
    #[serde(default)]
    pub tree: Option<String>,
    #[serde(alias = "program")]
    pub pipeline: String,
    pub parents: Vec<String>,
    pub message: String,
    pub timestamp: String,
    pub author: String,
    #[serde(default)]
    pub contributors: Option<Vec<CommitContributor>>,
    pub eval_contract: EvalContract,
    /// Environment in which the scores were captured (paper Definition 5.1).
    #[serde(default)]
    pub env_constraints: Option<BTreeMap<String, serde_json::Value>>,
    /// Hashes of supporting Run and Trace objects (paper Definition 5.1).
    #[serde(default)]
    pub evidence_refs: Option<Vec<String>>,
    #[serde(default)]
    pub morph_version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitContributor {
    pub id: String,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvalContract {
    pub suite: String,
    pub observed_metrics: BTreeMap<String, f64>,
}

// ---------- 4.6 Run ----------
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    #[serde(alias = "program")]
    pub pipeline: String,
    #[serde(default)]
    pub commit: Option<String>,
    pub environment: RunEnvironment,
    pub input_state_hash: String,
    pub output_artifacts: Vec<String>,
    #[serde(default)]
    pub metrics: BTreeMap<String, f64>,
    pub trace: String,
    pub agent: AgentInfo,
    #[serde(default)]
    pub contributors: Option<Vec<ContributorInfo>>,
    #[serde(default)]
    pub morph_version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContributorInfo {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub instance_id: Option<String>,
    #[serde(default)]
    pub policy: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunEnvironment {
    pub model: String,
    pub version: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub toolchain: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub instance_id: Option<String>,
    #[serde(default)]
    pub policy: Option<String>,
}

// ---------- 4.7 Artifact ----------
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub kind: String,
    pub content: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

// ---------- 4.8 Trace ----------
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub events: Vec<TraceEvent>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraceEvent {
    pub id: String,
    pub seq: u64,
    pub ts: String,
    pub kind: String,
    #[serde(default)]
    pub payload: BTreeMap<String, serde_json::Value>,
}

// ---------- 4.9 TraceRollup ----------
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceRollup {
    pub trace: String,
    pub summary: String,
    pub key_events: Vec<String>,
}

// ---------- 4.10 Annotation ----------
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub target: String,
    #[serde(default)]
    pub target_sub: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub data: BTreeMap<String, serde_json::Value>,
    pub author: String,
    pub timestamp: String,
}

// ---------- Tagged enum for (de)serialization ----------
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MorphObject {
    Blob(Blob),
    Tree(Tree),
    #[serde(alias = "program")]
    Pipeline(Pipeline),
    EvalSuite(EvalSuite),
    Commit(Commit),
    Run(Run),
    Artifact(Artifact),
    Trace(Trace),
    TraceRollup(TraceRollup),
    Annotation(Annotation),
}

