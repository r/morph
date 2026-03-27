//! Pipeline extraction from recorded Runs (v0-spec Pipeline provenance, Phase 3).
//!
//! Turns a recorded Run + Trace into a first-class Pipeline object with
//! deterministic graph structure, attribution, and provenance.

use crate::objects::{
    ActorRef, AttributionEntry, Blob, MorphObject, Pipeline, PipelineEdge, PipelineGraph,
    PipelineNode, Provenance,
};
use crate::store::{MorphError, Store};
use crate::Hash;
use std::collections::BTreeMap;

/// Extract a Pipeline from a stored Run.
///
/// For session-backed Runs (canonical `record_session` shape: 2 events, prompt + response),
/// produces a deterministic minimal graph:
///   - `generate` (prompt_call) → `review` (review)
///
/// Stores the Pipeline and all supporting objects (prompt blob) in the store.
/// Returns the Pipeline hash.
pub fn extract_pipeline_from_run(
    store: &dyn Store,
    run_hash: &Hash,
) -> Result<Hash, MorphError> {
    let obj = store.get(run_hash)?;
    let run = match obj {
        MorphObject::Run(r) => r,
        _ => {
            return Err(MorphError::Serialization(format!(
                "object {} is not a Run",
                run_hash
            )))
        }
    };

    let trace_hash =
        Hash::from_hex(&run.trace).map_err(|_| MorphError::InvalidHash(run.trace.clone()))?;
    let trace = match store.get(&trace_hash)? {
        MorphObject::Trace(t) => t,
        _ => {
            return Err(MorphError::Serialization(format!(
                "object {} (referenced by run.trace) is not a Trace",
                run.trace
            )))
        }
    };

    if trace.events.len() != 2 {
        return Err(MorphError::Serialization(format!(
            "unsupported trace shape: expected 2 events (prompt, response), got {}",
            trace.events.len()
        )));
    }

    let prompt_event = &trace.events[0];
    let response_event = &trace.events[1];

    if prompt_event.kind != "prompt" || response_event.kind != "response" {
        return Err(MorphError::Serialization(format!(
            "unsupported trace shape: expected (prompt, response), got ({}, {})",
            prompt_event.kind, response_event.kind
        )));
    }

    let prompt_text = prompt_event
        .payload
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let prompt_blob = MorphObject::Blob(Blob {
        kind: "prompt".to_string(),
        content: serde_json::json!({ "text": prompt_text }),
    });
    let prompt_blob_hash = store.put(&prompt_blob)?;

    let mut node_env = BTreeMap::new();
    node_env.insert(
        "model".into(),
        serde_json::Value::String(run.environment.model.clone()),
    );
    node_env.insert(
        "version".into(),
        serde_json::Value::String(run.environment.version.clone()),
    );
    if !run.environment.parameters.is_empty() {
        node_env.insert(
            "parameters".into(),
            serde_json::to_value(&run.environment.parameters).unwrap_or_default(),
        );
    }
    if !run.environment.toolchain.is_empty() {
        node_env.insert(
            "toolchain".into(),
            serde_json::to_value(&run.environment.toolchain).unwrap_or_default(),
        );
    }

    let generate_node = PipelineNode {
        id: "generate".to_string(),
        kind: "prompt_call".to_string(),
        ref_: Some(prompt_blob_hash.to_string()),
        params: BTreeMap::new(),
        env: Some(node_env),
    };

    let review_node = PipelineNode {
        id: "review".to_string(),
        kind: "review".to_string(),
        ref_: None,
        params: BTreeMap::new(),
        env: None,
    };

    let edge = PipelineEdge {
        from: "generate".to_string(),
        to: "review".to_string(),
        kind: "data".to_string(),
    };

    let primary_actor = ActorRef {
        id: run.agent.id.clone(),
        actor_type: "agent".to_string(),
        env_config: None,
    };

    let mut attribution = BTreeMap::new();

    attribution.insert(
        "generate".to_string(),
        AttributionEntry {
            agent_id: run.agent.id.clone(),
            agent_version: Some(run.agent.version.clone()),
            instance_id: run.agent.instance_id.clone(),
            actors: Some(vec![primary_actor.clone()]),
        },
    );

    let review_actors: Vec<ActorRef> = if let Some(ref contribs) = run.contributors {
        let reviewers: Vec<&_> = contribs
            .iter()
            .filter(|c| c.role.as_deref() == Some("review"))
            .collect();
        if reviewers.is_empty() {
            vec![primary_actor.clone()]
        } else {
            reviewers
                .iter()
                .map(|c| ActorRef {
                    id: c.id.clone(),
                    actor_type: "agent".to_string(),
                    env_config: None,
                })
                .collect()
        }
    } else {
        vec![primary_actor.clone()]
    };

    let review_agent_id = review_actors
        .first()
        .map(|a| a.id.clone())
        .unwrap_or_default();

    attribution.insert(
        "review".to_string(),
        AttributionEntry {
            agent_id: review_agent_id,
            agent_version: None,
            instance_id: None,
            actors: Some(review_actors),
        },
    );

    let provenance = Provenance {
        derived_from_run: Some(run_hash.to_string()),
        derived_from_trace: Some(trace_hash.to_string()),
        derived_from_event: Some(response_event.id.clone()),
        method: "extracted".to_string(),
    };

    let pipeline = MorphObject::Pipeline(Pipeline {
        graph: PipelineGraph {
            nodes: vec![generate_node, review_node],
            edges: vec![edge],
        },
        prompts: vec![prompt_blob_hash.to_string()],
        eval_suite: None,
        attribution: Some(attribution),
        provenance: Some(provenance),
    });

    let pipeline_hash = store.put(&pipeline)?;
    Ok(pipeline_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::*;

    fn setup_repo() -> (tempfile::TempDir, Box<dyn Store>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _ = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();
        (dir, store)
    }

    fn store_session_run(store: &dyn Store) -> (Hash, Hash) {
        let run_hash =
            crate::record::record_session(store, "Explain recursion", "Recursion is...", Some("gpt-4o"), Some("agent-1"))
                .unwrap();
        let run = match store.get(&run_hash).unwrap() {
            MorphObject::Run(r) => r,
            _ => panic!("expected run"),
        };
        let trace_hash = Hash::from_hex(&run.trace).unwrap();
        (run_hash, trace_hash)
    }

    fn store_session_run_with_reviewer(store: &dyn Store) -> (Hash, Hash) {
        let now = chrono::Utc::now().to_rfc3339();
        let mut prompt_payload = BTreeMap::new();
        prompt_payload.insert("text".into(), serde_json::Value::String("Build feature X".into()));
        let mut response_payload = BTreeMap::new();
        response_payload.insert("text".into(), serde_json::Value::String("Feature X built".into()));

        let trace = MorphObject::Trace(Trace {
            events: vec![
                TraceEvent {
                    id: "evt_prompt".into(),
                    seq: 0,
                    ts: now.clone(),
                    kind: "prompt".into(),
                    payload: prompt_payload,
                },
                TraceEvent {
                    id: "evt_response".into(),
                    seq: 1,
                    ts: now.clone(),
                    kind: "response".into(),
                    payload: response_payload,
                },
            ],
        });
        let trace_hash = store.put(&trace).unwrap();

        let identity = crate::identity::identity_pipeline();
        let pipeline_hash = store.put(&identity).unwrap();

        let mut params = BTreeMap::new();
        params.insert("temperature".into(), serde_json::json!(0.7));

        let run = MorphObject::Run(Run {
            pipeline: pipeline_hash.to_string(),
            commit: None,
            environment: RunEnvironment {
                model: "claude-4".into(),
                version: "2025-06-01".into(),
                parameters: params,
                toolchain: BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: BTreeMap::new(),
            trace: trace_hash.to_string(),
            agent: AgentInfo {
                id: "primary-agent".into(),
                version: "1.0".into(),
                policy: None,
                instance_id: None,
            },
            contributors: Some(vec![ContributorInfo {
                id: "review-agent".into(),
                version: "2.0".into(),
                policy: None,
                instance_id: None,
                role: Some("review".into()),
            }]),
            morph_version: None,
        });
        let run_hash = store.put(&run).unwrap();
        (run_hash, trace_hash)
    }

    #[test]
    fn extract_session_produces_deterministic_graph_shape() {
        let (_dir, store) = setup_repo();
        let (run_hash, _) = store_session_run(store.as_ref());

        let pipeline_hash = extract_pipeline_from_run(store.as_ref(), &run_hash).unwrap();
        let obj = store.get(&pipeline_hash).unwrap();
        let pipeline = match obj {
            MorphObject::Pipeline(p) => p,
            _ => panic!("expected pipeline"),
        };

        assert_eq!(pipeline.graph.nodes.len(), 2);
        assert_eq!(pipeline.graph.nodes[0].id, "generate");
        assert_eq!(pipeline.graph.nodes[0].kind, "prompt_call");
        assert_eq!(pipeline.graph.nodes[1].id, "review");
        assert_eq!(pipeline.graph.nodes[1].kind, "review");

        assert_eq!(pipeline.graph.edges.len(), 1);
        assert_eq!(pipeline.graph.edges[0].from, "generate");
        assert_eq!(pipeline.graph.edges[0].to, "review");
        assert_eq!(pipeline.graph.edges[0].kind, "data");
    }

    #[test]
    fn extract_session_persists_provenance() {
        let (_dir, store) = setup_repo();
        let (run_hash, trace_hash) = store_session_run(store.as_ref());

        let pipeline_hash = extract_pipeline_from_run(store.as_ref(), &run_hash).unwrap();
        let pipeline = match store.get(&pipeline_hash).unwrap() {
            MorphObject::Pipeline(p) => p,
            _ => panic!("expected pipeline"),
        };

        let prov = pipeline.provenance.as_ref().expect("provenance should be present");
        assert_eq!(prov.derived_from_run.as_deref(), Some(run_hash.to_string().as_str()));
        assert_eq!(prov.derived_from_trace.as_deref(), Some(trace_hash.to_string().as_str()));
        assert_eq!(prov.derived_from_event.as_deref(), Some("evt_response"));
        assert_eq!(prov.method, "extracted");
    }

    #[test]
    fn extract_session_generate_env_mirrors_run_environment() {
        let (_dir, store) = setup_repo();
        let (run_hash, _) = store_session_run(store.as_ref());

        let pipeline_hash = extract_pipeline_from_run(store.as_ref(), &run_hash).unwrap();
        let pipeline = match store.get(&pipeline_hash).unwrap() {
            MorphObject::Pipeline(p) => p,
            _ => panic!("expected pipeline"),
        };

        let gen_env = pipeline.graph.nodes[0].env.as_ref().expect("generate should have env");
        assert_eq!(gen_env.get("model").and_then(|v| v.as_str()), Some("gpt-4o"));
    }

    #[test]
    fn extract_session_includes_prompt_blob_ref() {
        let (_dir, store) = setup_repo();
        let (run_hash, _) = store_session_run(store.as_ref());

        let pipeline_hash = extract_pipeline_from_run(store.as_ref(), &run_hash).unwrap();
        let pipeline = match store.get(&pipeline_hash).unwrap() {
            MorphObject::Pipeline(p) => p,
            _ => panic!("expected pipeline"),
        };

        assert!(!pipeline.prompts.is_empty(), "prompts array should not be empty");
        let prompt_ref = &pipeline.prompts[0];
        assert_eq!(prompt_ref.len(), 64, "prompt ref should be a hash");

        let gen_ref = pipeline.graph.nodes[0].ref_.as_ref().expect("generate should have ref");
        assert_eq!(gen_ref, prompt_ref, "generate node ref should match prompts[0]");

        let blob_hash = Hash::from_hex(prompt_ref).unwrap();
        let blob_obj = store.get(&blob_hash).unwrap();
        assert!(matches!(blob_obj, MorphObject::Blob(_)));
    }

    #[test]
    fn extract_session_attribution_includes_primary_agent() {
        let (_dir, store) = setup_repo();
        let (run_hash, _) = store_session_run(store.as_ref());

        let pipeline_hash = extract_pipeline_from_run(store.as_ref(), &run_hash).unwrap();
        let pipeline = match store.get(&pipeline_hash).unwrap() {
            MorphObject::Pipeline(p) => p,
            _ => panic!("expected pipeline"),
        };

        let attr = pipeline.attribution.as_ref().expect("attribution should be present");
        let gen_attr = attr.get("generate").expect("generate attribution");
        assert_eq!(gen_attr.agent_id, "agent-1");
        let actors = gen_attr.actors.as_ref().expect("actors should be present");
        assert!(actors.iter().any(|a| a.id == "agent-1"));
    }

    #[test]
    fn extract_with_reviewer_maps_review_attribution() {
        let (_dir, store) = setup_repo();
        let (run_hash, _) = store_session_run_with_reviewer(store.as_ref());

        let pipeline_hash = extract_pipeline_from_run(store.as_ref(), &run_hash).unwrap();
        let pipeline = match store.get(&pipeline_hash).unwrap() {
            MorphObject::Pipeline(p) => p,
            _ => panic!("expected pipeline"),
        };

        let attr = pipeline.attribution.as_ref().expect("attribution");
        let review_attr = attr.get("review").expect("review attribution");
        let actors = review_attr.actors.as_ref().expect("review actors");
        assert!(actors.iter().any(|a| a.id == "review-agent"));
        assert_eq!(review_attr.agent_id, "review-agent");

        let gen_attr = attr.get("generate").expect("generate attribution");
        assert_eq!(gen_attr.agent_id, "primary-agent");
    }

    #[test]
    fn extract_without_reviewer_falls_back_to_primary_agent() {
        let (_dir, store) = setup_repo();
        let (run_hash, _) = store_session_run(store.as_ref());

        let pipeline_hash = extract_pipeline_from_run(store.as_ref(), &run_hash).unwrap();
        let pipeline = match store.get(&pipeline_hash).unwrap() {
            MorphObject::Pipeline(p) => p,
            _ => panic!("expected pipeline"),
        };

        let attr = pipeline.attribution.as_ref().expect("attribution");
        let review_attr = attr.get("review").expect("review attribution");
        let actors = review_attr.actors.as_ref().expect("review actors");
        assert!(actors.iter().any(|a| a.id == "agent-1"));
    }

    #[test]
    fn extract_fails_on_missing_object() {
        let (_dir, store) = setup_repo();
        let fake = Hash::from_hex(&"a".repeat(64)).unwrap();
        let result = extract_pipeline_from_run(store.as_ref(), &fake);
        assert!(result.is_err());
    }

    #[test]
    fn extract_fails_on_non_run_object() {
        let (_dir, store) = setup_repo();
        let blob = MorphObject::Blob(Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let blob_hash = store.put(&blob).unwrap();
        let result = extract_pipeline_from_run(store.as_ref(), &blob_hash);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not a Run"), "error: {}", msg);
    }

    #[test]
    fn extract_fails_on_missing_trace() {
        let (_dir, store) = setup_repo();

        let identity = crate::identity::identity_pipeline();
        let pipeline_hash = store.put(&identity).unwrap();

        let run = MorphObject::Run(Run {
            pipeline: pipeline_hash.to_string(),
            commit: None,
            environment: RunEnvironment {
                model: "test".into(),
                version: "1".into(),
                parameters: BTreeMap::new(),
                toolchain: BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: BTreeMap::new(),
            trace: "b".repeat(64),
            agent: AgentInfo {
                id: "a".into(),
                version: "1".into(),
                policy: None,
                instance_id: None,
            },
            contributors: None,
            morph_version: None,
        });
        let run_hash = store.put(&run).unwrap();
        let result = extract_pipeline_from_run(store.as_ref(), &run_hash);
        assert!(result.is_err());
    }

    #[test]
    fn extract_fails_on_unsupported_trace_shape() {
        let (_dir, store) = setup_repo();

        let trace = MorphObject::Trace(Trace {
            events: vec![
                TraceEvent {
                    id: "e1".into(),
                    seq: 0,
                    ts: "2025-01-01T00:00:00Z".into(),
                    kind: "tool_call".into(),
                    payload: BTreeMap::new(),
                },
                TraceEvent {
                    id: "e2".into(),
                    seq: 1,
                    ts: "2025-01-01T00:00:00Z".into(),
                    kind: "tool_call".into(),
                    payload: BTreeMap::new(),
                },
            ],
        });
        let trace_hash = store.put(&trace).unwrap();

        let identity = crate::identity::identity_pipeline();
        let pipeline_hash = store.put(&identity).unwrap();

        let run = MorphObject::Run(Run {
            pipeline: pipeline_hash.to_string(),
            commit: None,
            environment: RunEnvironment {
                model: "test".into(),
                version: "1".into(),
                parameters: BTreeMap::new(),
                toolchain: BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: BTreeMap::new(),
            trace: trace_hash.to_string(),
            agent: AgentInfo {
                id: "a".into(),
                version: "1".into(),
                policy: None,
                instance_id: None,
            },
            contributors: None,
            morph_version: None,
        });
        let run_hash = store.put(&run).unwrap();
        let result = extract_pipeline_from_run(store.as_ref(), &run_hash);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unsupported trace shape"), "error: {}", msg);
    }

    #[test]
    fn extract_fails_on_wrong_event_count() {
        let (_dir, store) = setup_repo();

        let trace = MorphObject::Trace(Trace {
            events: vec![TraceEvent {
                id: "e1".into(),
                seq: 0,
                ts: "2025-01-01T00:00:00Z".into(),
                kind: "prompt".into(),
                payload: BTreeMap::new(),
            }],
        });
        let trace_hash = store.put(&trace).unwrap();

        let identity = crate::identity::identity_pipeline();
        let pipeline_hash = store.put(&identity).unwrap();

        let run = MorphObject::Run(Run {
            pipeline: pipeline_hash.to_string(),
            commit: None,
            environment: RunEnvironment {
                model: "test".into(),
                version: "1".into(),
                parameters: BTreeMap::new(),
                toolchain: BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: BTreeMap::new(),
            trace: trace_hash.to_string(),
            agent: AgentInfo {
                id: "a".into(),
                version: "1".into(),
                policy: None,
                instance_id: None,
            },
            contributors: None,
            morph_version: None,
        });
        let run_hash = store.put(&run).unwrap();
        let result = extract_pipeline_from_run(store.as_ref(), &run_hash);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unsupported trace shape"), "error: {}", msg);
    }
}
