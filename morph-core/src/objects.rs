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
    /// PR 6 stage B: stable per-machine `agent.instance_id` of the
    /// repo that produced this commit. Used by `morph log` /
    /// `morph show` to disambiguate two laptops driven by the same
    /// human (`user.name`). Always optional — pre-PR-6 commits and
    /// commits made via APIs that don't pass through `morph-cli`
    /// stay `None`.
    #[serde(default)]
    pub morph_instance: Option<String>,
    /// Reference-mode foundation: how this commit entered the morph
    /// DAG. Conventional values are `"cli"`, `"mcp"`, and
    /// `"git-hook"`; anything is accepted to keep the schema open.
    /// Absence is treated as `"cli"` for compatibility with all
    /// pre-existing commits — those were necessarily authored
    /// through `morph-cli`. The git-hook case is reserved for
    /// reference-mode auto-mirrored commits and lets the merge
    /// gate / certification flow distinguish "human said done"
    /// from "git committed and we caught up".
    #[serde(default)]
    pub morph_origin: Option<String>,
    /// Reference-mode: the git commit SHA this Morph commit was
    /// synthesized from. Set for `morph_origin == Some("git-hook")`
    /// commits; `None` for commits authored directly through Morph.
    /// Used by `morph reference-sync` to detect already-synced
    /// commits and by `morph status` to compute pending-certification
    /// counts.
    #[serde(default)]
    pub git_origin_sha: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// PR 6 stage B cycle 7: legacy commit JSON (no `morph_instance`
    /// field) must still deserialize, with the field defaulting to
    /// `None`. This is the back-compat guarantee — older repos
    /// pulled from across the wire keep working without any
    /// migration on the receiving end.
    #[test]
    fn legacy_commit_without_morph_instance_deserializes() {
        let json = r#"{
            "type": "commit",
            "tree": null,
            "pipeline": "0000000000000000000000000000000000000000000000000000000000000000",
            "parents": [],
            "message": "legacy commit from a pre-PR-6 repo",
            "timestamp": "2024-01-01T00:00:00Z",
            "author": "morph",
            "contributors": null,
            "eval_contract": {
                "suite": "0000000000000000000000000000000000000000000000000000000000000000",
                "observed_metrics": {}
            },
            "env_constraints": null,
            "evidence_refs": null,
            "morph_version": null
        }"#;
        let obj: MorphObject =
            serde_json::from_str(json).expect("legacy commit must deserialize cleanly");
        match obj {
            MorphObject::Commit(c) => {
                assert_eq!(c.morph_instance, None);
                assert_eq!(c.author, "morph");
            }
            _ => panic!("expected a Commit variant"),
        }
    }

    #[test]
    fn commit_with_morph_instance_round_trips() {
        let original = MorphObject::Commit(Commit {
            tree: None,
            pipeline: "0".repeat(64),
            parents: vec![],
            message: "with instance".into(),
            timestamp: "2026-04-26T00:00:00Z".into(),
            author: "Raffi <r@e.com>".into(),
            contributors: None,
            eval_contract: EvalContract {
                suite: "0".repeat(64),
                observed_metrics: BTreeMap::new(),
            },
            env_constraints: None,
            evidence_refs: None,
            morph_version: Some("0.10.0".into()),
            morph_instance: Some("morph-abc123".into()),
            morph_origin: None,
            git_origin_sha: None,
        });
        let s = serde_json::to_string(&original).unwrap();
        assert!(s.contains("\"morph_instance\":\"morph-abc123\""));
        let parsed: MorphObject = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, original);
    }

    /// PR 1 (reference-mode foundation): legacy commit JSON without
    /// the new `morph_origin` field still deserializes cleanly.
    /// Pre-PR-1 commits could only originate from morph-cli, so
    /// reading them back as `morph_origin: None` is the right
    /// default. Treating `None` as "cli" is a read-side concern
    /// owned by callers (e.g., the future git-hook policy gate).
    #[test]
    fn legacy_commit_without_morph_origin_deserializes() {
        let json = r#"{
            "type": "commit",
            "tree": null,
            "pipeline": "0000000000000000000000000000000000000000000000000000000000000000",
            "parents": [],
            "message": "legacy commit",
            "timestamp": "2024-01-01T00:00:00Z",
            "author": "morph",
            "contributors": null,
            "eval_contract": {
                "suite": "0000000000000000000000000000000000000000000000000000000000000000",
                "observed_metrics": {}
            },
            "env_constraints": null,
            "evidence_refs": null,
            "morph_version": null,
            "morph_instance": null
        }"#;
        let obj: MorphObject =
            serde_json::from_str(json).expect("legacy commit must deserialize");
        match obj {
            MorphObject::Commit(c) => assert_eq!(c.morph_origin, None),
            _ => panic!("expected a Commit variant"),
        }
    }

    /// PR 1: explicit `morph_origin` round-trips. The git-hook
    /// origin is the headline non-default case — when reference
    /// mode auto-mirrors a `git commit`, it stamps `"git-hook"`
    /// here so downstream gates can distinguish certified human
    /// work from passive mirroring.
    #[test]
    fn commit_with_morph_origin_round_trips() {
        let original = MorphObject::Commit(Commit {
            tree: None,
            pipeline: "0".repeat(64),
            parents: vec![],
            message: "git-mirrored".into(),
            timestamp: "2026-04-27T00:00:00Z".into(),
            author: "Raffi <r@e.com>".into(),
            contributors: None,
            eval_contract: EvalContract {
                suite: "0".repeat(64),
                observed_metrics: BTreeMap::new(),
            },
            env_constraints: None,
            evidence_refs: None,
            morph_version: Some("0.24.0".into()),
            morph_instance: None,
            morph_origin: Some("git-hook".into()),
            git_origin_sha: Some("a".repeat(40)),
        });
        let s = serde_json::to_string(&original).unwrap();
        assert!(s.contains("\"morph_origin\":\"git-hook\""));
        assert!(s.contains("\"git_origin_sha\":\""));
        let parsed: MorphObject = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, original);
    }

    /// PR 2 (reference mode): legacy commits without the
    /// `git_origin_sha` field still deserialize. Combined with
    /// `legacy_commit_without_morph_origin_deserializes`, this
    /// proves the schema is fully back-compatible across both
    /// reference-mode foundation fields.
    #[test]
    fn legacy_commit_without_git_origin_sha_deserializes() {
        let json = r#"{
            "type": "commit",
            "tree": null,
            "pipeline": "0000000000000000000000000000000000000000000000000000000000000000",
            "parents": [],
            "message": "legacy commit",
            "timestamp": "2024-01-01T00:00:00Z",
            "author": "morph",
            "contributors": null,
            "eval_contract": {
                "suite": "0000000000000000000000000000000000000000000000000000000000000000",
                "observed_metrics": {}
            },
            "env_constraints": null,
            "evidence_refs": null,
            "morph_version": null,
            "morph_instance": null,
            "morph_origin": "git-hook"
        }"#;
        let obj: MorphObject =
            serde_json::from_str(json).expect("legacy commit must deserialize");
        match obj {
            MorphObject::Commit(c) => {
                assert_eq!(c.morph_origin.as_deref(), Some("git-hook"));
                assert_eq!(c.git_origin_sha, None);
            }
            _ => panic!("expected a Commit variant"),
        }
    }
}

