//! Ingest execution evidence: run record and eval record (v0-spec §6.6, §6.7).

use crate::hash::content_hash;
use crate::identity::identity_program;
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

    let trace_hash = content_hash(&trace)?;
    store.put(&trace)?;

    let identity = identity_program();
    let program_hash = content_hash(&identity)?;
    store.put(&identity)?;

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
        program: program_hash.to_string(),
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
        },
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
}
