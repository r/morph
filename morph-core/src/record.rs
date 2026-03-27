//! Ingest execution evidence: run record and eval record (v0-spec §6.6, §6.7).

use crate::identity::identity_pipeline;
use crate::objects::{AgentInfo, Blob, MorphObject, Run, RunEnvironment, Trace, TraceEvent};
use crate::store::{MorphError, Store};
use crate::Hash;
use std::collections::BTreeMap;
use std::path::Path;

/// Ingest a Run from JSON. Optionally ingest trace and artifacts first so refs resolve.
/// Returns the Run's hash.
pub fn record_run(
    store: &dyn Store,
    run_path: &Path,
    trace_path: Option<&Path>,
    artifact_paths: &[&Path],
) -> Result<Hash, MorphError> {
    let run_json = std::fs::read_to_string(run_path)?;
    let run_obj: MorphObject = serde_json::from_str(&run_json).map_err(|e| MorphError::Serialization(e.to_string()))?;
    let run = match &run_obj {
        MorphObject::Run(r) => r,
        _ => return Err(MorphError::Serialization("file is not a Run object".into())),
    };

    if let Some(tp) = trace_path {
        let trace_json = std::fs::read_to_string(tp)?;
        let trace_obj: MorphObject = serde_json::from_str(&trace_json).map_err(|e| MorphError::Serialization(e.to_string()))?;
        let trace_hash = store.put(&trace_obj)?;
        if trace_hash.to_string() != run.trace {
            return Err(MorphError::Serialization(format!("trace hash mismatch: computed {} vs run.trace {}", trace_hash, run.trace)));
        }
    }

    for ap in artifact_paths {
        let art_json = std::fs::read_to_string(ap)?;
        let art_obj: MorphObject = serde_json::from_str(&art_json).map_err(|e| MorphError::Serialization(e.to_string()))?;
        store.put(&art_obj)?;
    }

    let hash = store.put(&run_obj)?;
    Ok(hash)
}

/// Ingest evaluation results from JSON. Expected shape: {"metrics": {"name": number, ...}}.
/// Returns the metrics map for use in commit or merge.
pub fn record_eval_metrics(path: &Path) -> Result<std::collections::BTreeMap<String, f64>, MorphError> {
    let s = std::fs::read_to_string(path)?;
    let value: serde_json::Value = serde_json::from_str(&s).map_err(|e| MorphError::Serialization(e.to_string()))?;
    let obj = value.as_object().ok_or_else(|| MorphError::Serialization("expected JSON object".into()))?;
    let metrics = obj.get("metrics").ok_or_else(|| MorphError::Serialization("missing 'metrics' key".into()))?;
    let map = metrics.as_object().ok_or_else(|| MorphError::Serialization("metrics must be an object".into()))?;
    let mut out = std::collections::BTreeMap::new();
    for (k, v) in map {
        let num = v.as_f64().ok_or_else(|| MorphError::Serialization(format!("metric {} must be a number", k)))?;
        out.insert(k.clone(), num);
    }
    Ok(out)
}

/// Record a single prompt/response session as a Run with a Trace (no files).
/// Uses the identity program. Call this from the IDE so the agent can pass its own response text.
pub fn record_session(
    store: &dyn Store,
    prompt: &str,
    response: &str,
    model_name: Option<&str>,
    agent_id: Option<&str>,
) -> Result<Hash, MorphError> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut prompt_payload = BTreeMap::new();
    prompt_payload.insert("text".to_string(), serde_json::Value::String(prompt.to_string()));
    let mut response_payload = BTreeMap::new();
    response_payload.insert("text".to_string(), serde_json::Value::String(response.to_string()));

    let trace = MorphObject::Trace(Trace {
        events: vec![
            TraceEvent {
                id: "evt_prompt".to_string(),
                seq: 0,
                ts: now.clone(),
                kind: "prompt".to_string(),
                payload: prompt_payload,
            },
            TraceEvent {
                id: "evt_response".to_string(),
                seq: 1,
                ts: now.clone(),
                kind: "response".to_string(),
                payload: response_payload,
            },
        ],
    });

    let trace_hash = store.put(&trace)?;

    let identity = identity_pipeline();
    let pipeline_hash = store.put(&identity)?;

    let prompt_blob = MorphObject::Blob(Blob {
        kind: "prompt".to_string(),
        content: serde_json::json!({
            "text": prompt,
            "response": response,
            "timestamp": now,
        }),
    });
    store.put(&prompt_blob)?;

    let run = MorphObject::Run(Run {
        pipeline: pipeline_hash.to_string(),
        commit: None,
        environment: RunEnvironment {
            model: model_name.unwrap_or("cursor").to_string(),
            version: "1.0".to_string(),
            parameters: BTreeMap::new(),
            toolchain: BTreeMap::new(),
        },
        input_state_hash: "0".repeat(64),
        output_artifacts: vec![],
        metrics: BTreeMap::new(),
        trace: trace_hash.to_string(),
        agent: AgentInfo {
            id: agent_id.unwrap_or("cursor").to_string(),
            version: "1.0".to_string(),
            policy: None,
            instance_id: None,
        },
        contributors: None,
        morph_version: None,
    });

    let run_hash = store.put(&run)?;
    Ok(run_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::FsStore;

    #[test]
    fn record_session_stores_run_and_trace() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().to_path_buf());
        let hash = record_session(
            &store,
            "What is 2+2?",
            "2+2 equals 4.",
            Some("test-model"),
            Some("test-agent"),
        )
        .unwrap();
        assert_eq!(hash.to_string().len(), 64);
        let run_obj = store.get(&hash).unwrap();
        assert!(matches!(run_obj, MorphObject::Run(_)));
    }

    #[test]
    fn record_session_populates_type_index_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::init_repo(dir.path()).unwrap();
        let run_hash = record_session(
            &store,
            "Hello",
            "Hi there!",
            None,
            None,
        )
        .unwrap();

        let morph = dir.path().join(".morph");
        let prompts: Vec<_> = std::fs::read_dir(morph.join("prompts"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!prompts.is_empty(), "prompts/ should contain at least one file");

        let runs: Vec<_> = std::fs::read_dir(morph.join("runs"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(runs.len(), 1, "runs/ should contain exactly one file");
        assert!(
            runs[0].file_name().to_string_lossy().contains(&run_hash.to_string()),
            "runs/ entry should match the run hash"
        );

        let traces: Vec<_> = std::fs::read_dir(morph.join("traces"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(traces.len(), 1, "traces/ should contain exactly one file");
    }

    #[test]
    fn record_run_ingests_run_from_json_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().to_path_buf());

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

        let run = MorphObject::Run(Run {
            pipeline: "0".repeat(64),
            commit: None,
            environment: RunEnvironment {
                model: "test".into(),
                version: "0".into(),
                parameters: BTreeMap::new(),
                toolchain: BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: BTreeMap::new(),
            trace: trace_hash.to_string(),
            agent: AgentInfo {
                id: "test-agent".into(),
                version: "1".into(),
                policy: None,
                instance_id: None,
            },
            contributors: None,
            morph_version: None,
        });
        let run_json = serde_json::to_string_pretty(&run).unwrap();
        let run_file = dir.path().join("run.json");
        std::fs::write(&run_file, &run_json).unwrap();

        let hash = record_run(&store, &run_file, None, &[]).unwrap();
        let obj = store.get(&hash).unwrap();
        assert!(matches!(obj, MorphObject::Run(_)));
    }

    #[test]
    fn record_run_with_trace_file_validates_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().to_path_buf());

        let trace = MorphObject::Trace(Trace {
            events: vec![TraceEvent {
                id: "e1".into(),
                seq: 0,
                ts: "2025-01-01T00:00:00Z".into(),
                kind: "prompt".into(),
                payload: BTreeMap::new(),
            }],
        });
        let trace_json = serde_json::to_string_pretty(&trace).unwrap();
        let trace_file = dir.path().join("trace.json");
        std::fs::write(&trace_file, &trace_json).unwrap();
        let trace_hash = store.put(&trace).unwrap();

        let run = MorphObject::Run(Run {
            pipeline: "0".repeat(64),
            commit: None,
            environment: RunEnvironment {
                model: "test".into(),
                version: "0".into(),
                parameters: BTreeMap::new(),
                toolchain: BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: BTreeMap::new(),
            trace: trace_hash.to_string(),
            agent: AgentInfo {
                id: "cli".into(),
                version: "0".into(),
                policy: None,
                instance_id: None,
            },
            contributors: None,
            morph_version: None,
        });
        let run_json = serde_json::to_string_pretty(&run).unwrap();
        let run_file = dir.path().join("run.json");
        std::fs::write(&run_file, &run_json).unwrap();

        let hash = record_run(&store, &run_file, Some(trace_file.as_path()), &[]).unwrap();
        assert!(store.get(&hash).is_ok());
    }

    #[test]
    fn record_run_trace_hash_mismatch_fails() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().to_path_buf());

        let trace = MorphObject::Trace(Trace {
            events: vec![TraceEvent {
                id: "different".into(),
                seq: 0,
                ts: "2025-01-01T00:00:00Z".into(),
                kind: "prompt".into(),
                payload: BTreeMap::new(),
            }],
        });
        let trace_json = serde_json::to_string_pretty(&trace).unwrap();
        let trace_file = dir.path().join("trace.json");
        std::fs::write(&trace_file, &trace_json).unwrap();

        let run = MorphObject::Run(Run {
            pipeline: "0".repeat(64),
            commit: None,
            environment: RunEnvironment {
                model: "test".into(),
                version: "0".into(),
                parameters: BTreeMap::new(),
                toolchain: BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: BTreeMap::new(),
            trace: "a".repeat(64),
            agent: AgentInfo {
                id: "cli".into(),
                version: "0".into(),
                policy: None,
                instance_id: None,
            },
            contributors: None,
            morph_version: None,
        });
        let run_json = serde_json::to_string_pretty(&run).unwrap();
        let run_file = dir.path().join("run.json");
        std::fs::write(&run_file, &run_json).unwrap();

        let result = record_run(&store, &run_file, Some(trace_file.as_path()), &[]);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("trace hash mismatch"), "expected mismatch error, got: {}", err);
    }

    #[test]
    fn record_run_rejects_non_run_object() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().to_path_buf());

        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"text": "hello"}),
        });
        let blob_json = serde_json::to_string_pretty(&blob).unwrap();
        let run_file = dir.path().join("run.json");
        std::fs::write(&run_file, &blob_json).unwrap();

        let result = record_run(&store, &run_file, None, &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not a Run"));
    }

    #[test]
    fn record_eval_metrics_parses_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.json");
        std::fs::write(&path, r#"{"metrics": {"accuracy": 0.95, "latency": 1.2}}"#).unwrap();

        let metrics = record_eval_metrics(&path).unwrap();
        assert_eq!(metrics.len(), 2);
        assert!((metrics["accuracy"] - 0.95).abs() < 1e-10);
        assert!((metrics["latency"] - 1.2).abs() < 1e-10);
    }

    #[test]
    fn record_eval_metrics_rejects_missing_metrics_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.json");
        std::fs::write(&path, r#"{"accuracy": 0.95}"#).unwrap();

        let result = record_eval_metrics(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing 'metrics' key"));
    }

    #[test]
    fn record_eval_metrics_rejects_non_numeric_metric() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.json");
        std::fs::write(&path, r#"{"metrics": {"name": "not-a-number"}}"#).unwrap();

        let result = record_eval_metrics(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be a number"));
    }

    #[test]
    fn record_eval_metrics_rejects_non_object_root() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.json");
        std::fs::write(&path, r#"[1, 2, 3]"#).unwrap();

        let result = record_eval_metrics(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expected JSON object"));
    }

    #[test]
    fn record_eval_metrics_rejects_non_object_metrics() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.json");
        std::fs::write(&path, r#"{"metrics": "not-an-object"}"#).unwrap();

        let result = record_eval_metrics(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("metrics must be an object"));
    }

    #[test]
    fn record_session_uses_defaults_for_model_and_agent() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().to_path_buf());
        let hash = record_session(&store, "p", "r", None, None).unwrap();
        let obj = store.get(&hash).unwrap();
        if let MorphObject::Run(run) = obj {
            assert_eq!(run.environment.model, "cursor");
            assert_eq!(run.agent.id, "cursor");
        } else {
            panic!("expected Run");
        }
    }

    #[test]
    fn record_run_with_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().to_path_buf());

        let artifact = MorphObject::Artifact(crate::objects::Artifact {
            kind: "model".into(),
            content: "base64data".into(),
            metadata: BTreeMap::new(),
        });
        let art_json = serde_json::to_string_pretty(&artifact).unwrap();
        let art_file = dir.path().join("artifact.json");
        std::fs::write(&art_file, &art_json).unwrap();

        let trace = MorphObject::Trace(Trace {
            events: vec![],
        });
        let trace_hash = store.put(&trace).unwrap();

        let run = MorphObject::Run(Run {
            pipeline: "0".repeat(64),
            commit: None,
            environment: RunEnvironment {
                model: "test".into(),
                version: "0".into(),
                parameters: BTreeMap::new(),
                toolchain: BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: BTreeMap::new(),
            trace: trace_hash.to_string(),
            agent: AgentInfo {
                id: "test".into(),
                version: "1".into(),
                policy: None,
                instance_id: None,
            },
            contributors: None,
            morph_version: None,
        });
        let run_json = serde_json::to_string_pretty(&run).unwrap();
        let run_file = dir.path().join("run.json");
        std::fs::write(&run_file, &run_json).unwrap();

        let hash = record_run(&store, &run_file, None, &[art_file.as_path()]).unwrap();
        assert!(store.get(&hash).is_ok());
    }
}
