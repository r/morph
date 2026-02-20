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
}

// ---------- 4.3 Program ----------
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Program {
    pub graph: ProgramGraph,
    pub prompts: Vec<String>,
    #[serde(default)]
    pub eval_suite: Option<String>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramGraph {
    pub nodes: Vec<ProgramNode>,
    pub edges: Vec<ProgramEdge>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramNode {
    pub id: String,
    pub kind: String,
    pub ref_: Option<String>,
    pub params: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramEdge {
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvalMetric {
    pub name: String,
    pub aggregation: String,
    pub threshold: f64,
}

// ---------- 4.5 Commit ----------
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Commit {
    pub program: String,
    pub parents: Vec<String>,
    pub message: String,
    pub timestamp: String,
    pub author: String,
    pub eval_contract: EvalContract,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvalContract {
    pub suite: String,
    pub observed_metrics: BTreeMap<String, f64>,
}

// ---------- 4.6 Run ----------
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub program: String,
    #[serde(default)]
    pub commit: Option<String>,
    pub environment: RunEnvironment,
    pub input_state_hash: String,
    pub output_artifacts: Vec<String>,
    #[serde(default)]
    pub metrics: BTreeMap<String, f64>,
    pub trace: String,
    pub agent: AgentInfo,
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
    Program(Program),
    EvalSuite(EvalSuite),
    Commit(Commit),
    Run(Run),
    Artifact(Artifact),
    Trace(Trace),
    TraceRollup(TraceRollup),
    Annotation(Annotation),
}

// Serde: ProgramNode uses "ref" in JSON but "ref" is a Rust keyword. Use ref_ with rename.
impl Serialize for ProgramNode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct ProgramNodeOut<'a> {
            id: &'a str,
            kind: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            r#ref: Option<&'a String>,
            params: &'a BTreeMap<String, serde_json::Value>,
        }
        ProgramNodeOut {
            id: &self.id,
            kind: &self.kind,
            r#ref: self.ref_.as_ref(),
            params: &self.params,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProgramNode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct ProgramNodeIn {
            id: String,
            kind: String,
            #[serde(default)]
            r#ref: Option<String>,
            #[serde(default)]
            params: BTreeMap<String, serde_json::Value>,
        }
        let in_ = ProgramNodeIn::deserialize(deserializer)?;
        Ok(ProgramNode {
            id: in_.id,
            kind: in_.kind,
            ref_: in_.r#ref,
            params: in_.params,
        })
    }
}
