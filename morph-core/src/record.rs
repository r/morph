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

/// A single message in a conversation (user prompt, assistant response, tool call, etc.).
///
/// The optional `metadata` map is merged into the trace event payload alongside
/// `text`, allowing callers to attach structured data (tool names, file paths,
/// etc.) without Morph needing to understand the semantics.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// Record a single prompt/response session as a Run with a Trace (no files).
/// Uses the identity pipeline. Call this from the IDE so the agent can pass its own response text.
pub fn record_session(
    store: &dyn Store,
    prompt: &str,
    response: &str,
    model_name: Option<&str>,
    agent_id: Option<&str>,
) -> Result<Hash, MorphError> {
    let messages = vec![
        ConversationMessage { role: "user".into(), content: prompt.to_string(), metadata: BTreeMap::new(), timestamp: None },
        ConversationMessage { role: "assistant".into(), content: response.to_string(), metadata: BTreeMap::new(), timestamp: None },
    ];
    record_conversation(store, &messages, model_name, agent_id)
}

/// Record a full multi-turn conversation as a Run with a Trace.
/// Each message becomes a TraceEvent preserving the complete back-and-forth
/// (user prompts, assistant responses, tool calls, tool results, etc.).
pub fn record_conversation(
    store: &dyn Store,
    messages: &[ConversationMessage],
    model_name: Option<&str>,
    agent_id: Option<&str>,
) -> Result<Hash, MorphError> {
    let now = chrono::Utc::now().to_rfc3339();

    let events: Vec<TraceEvent> = messages
        .iter()
        .enumerate()
        .map(|(i, msg)| {
            let mut payload = msg.metadata.clone();
            payload.insert("text".into(), serde_json::Value::String(msg.content.clone()));
            let ts = msg.timestamp.as_deref().unwrap_or(&now).to_string();
            TraceEvent {
                id: format!("evt_{}", i),
                seq: i as u64,
                ts,
                kind: msg.role.clone(),
                payload,
            }
        })
        .collect();

    let trace = MorphObject::Trace(Trace { events });
    let trace_hash = store.put(&trace)?;

    let identity = identity_pipeline();
    let pipeline_hash = store.put(&identity)?;

    let first_prompt = messages.iter().find(|m| m.role == "user").map(|m| m.content.as_str()).unwrap_or("");
    let last_response = messages.iter().rev().find(|m| m.role == "assistant").map(|m| m.content.as_str()).unwrap_or("");

    let prompt_blob = MorphObject::Blob(Blob {
        kind: "prompt".to_string(),
        content: serde_json::json!({
            "text": first_prompt,
            "response": last_response,
            "timestamp": now,
            "message_count": messages.len(),
        }),
    });
    store.put(&prompt_blob)?;

    // Link run to HEAD commit if one exists
    let head_commit = crate::commit::resolve_head(store).ok().flatten().map(|h| h.to_string());

    let run = MorphObject::Run(Run {
        pipeline: pipeline_hash.to_string(),
        commit: head_commit,
        environment: RunEnvironment {
            model: model_name.unwrap_or("unknown").to_string(),
            version: "1.0".to_string(),
            parameters: BTreeMap::new(),
            toolchain: BTreeMap::new(),
        },
        input_state_hash: "0".repeat(64),
        output_artifacts: vec![],
        metrics: BTreeMap::new(),
        trace: trace_hash.to_string(),
        agent: AgentInfo {
            id: agent_id.unwrap_or("unknown").to_string(),
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
    fn record_conversation_stores_all_messages() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().to_path_buf());
        let messages = vec![
            ConversationMessage { role: "user".into(), content: "Build a web server".into(), metadata: BTreeMap::new(), timestamp: None },
            ConversationMessage { role: "assistant".into(), content: "I'll create the files".into(), metadata: BTreeMap::new(), timestamp: None },
            ConversationMessage { role: "tool_call".into(), content: "write_file(server.py)".into(), metadata: BTreeMap::new(), timestamp: None },
            ConversationMessage { role: "tool_result".into(), content: "file written".into(), metadata: BTreeMap::new(), timestamp: None },
            ConversationMessage { role: "assistant".into(), content: "Done! Server is ready.".into(), metadata: BTreeMap::new(), timestamp: None },
        ];
        let hash = record_conversation(&store, &messages, Some("qwen-3.5"), Some("opencode")).unwrap();
        let run_obj = store.get(&hash).unwrap();
        let run = match run_obj {
            MorphObject::Run(r) => r,
            _ => panic!("expected Run"),
        };
        assert_eq!(run.environment.model, "qwen-3.5");
        assert_eq!(run.agent.id, "opencode");

        let trace_hash = Hash::from_hex(&run.trace).unwrap();
        let trace_obj = store.get(&trace_hash).unwrap();
        let trace = match trace_obj {
            MorphObject::Trace(t) => t,
            _ => panic!("expected Trace"),
        };
        assert_eq!(trace.events.len(), 5);
        assert_eq!(trace.events[0].kind, "user");
        assert_eq!(trace.events[1].kind, "assistant");
        assert_eq!(trace.events[2].kind, "tool_call");
        assert_eq!(trace.events[3].kind, "tool_result");
        assert_eq!(trace.events[4].kind, "assistant");
        assert_eq!(
            trace.events[0].payload["text"].as_str().unwrap(),
            "Build a web server"
        );
    }

    #[test]
    fn record_session_uses_defaults_for_model_and_agent() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().to_path_buf());
        let hash = record_session(&store, "p", "r", None, None).unwrap();
        let obj = store.get(&hash).unwrap();
        if let MorphObject::Run(run) = obj {
            assert_eq!(run.environment.model, "unknown");
            assert_eq!(run.agent.id, "unknown");
        } else {
            panic!("expected Run");
        }
    }

    #[test]
    fn record_conversation_metadata_merges_into_payload() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().to_path_buf());
        let mut meta = BTreeMap::new();
        meta.insert("name".into(), serde_json::json!("read_file"));
        meta.insert("path".into(), serde_json::json!("src/main.rs"));
        let messages = vec![
            ConversationMessage { role: "user".into(), content: "Read the file".into(), metadata: BTreeMap::new(), timestamp: None },
            ConversationMessage { role: "file_read".into(), content: "fn main() {}".into(), metadata: meta, timestamp: None },
        ];
        let hash = record_conversation(&store, &messages, Some("test"), Some("test")).unwrap();
        let run = match store.get(&hash).unwrap() { MorphObject::Run(r) => r, _ => panic!("expected Run") };
        let trace_hash = Hash::from_hex(&run.trace).unwrap();
        let trace = match store.get(&trace_hash).unwrap() { MorphObject::Trace(t) => t, _ => panic!("expected Trace") };

        assert_eq!(trace.events[1].payload["text"].as_str().unwrap(), "fn main() {}");
        assert_eq!(trace.events[1].payload["name"].as_str().unwrap(), "read_file");
        assert_eq!(trace.events[1].payload["path"].as_str().unwrap(), "src/main.rs");
    }

    #[test]
    fn record_conversation_per_message_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().to_path_buf());
        let messages = vec![
            ConversationMessage { role: "user".into(), content: "Hello".into(), metadata: BTreeMap::new(), timestamp: Some("2026-01-01T10:00:00Z".into()) },
            ConversationMessage { role: "assistant".into(), content: "Hi".into(), metadata: BTreeMap::new(), timestamp: Some("2026-01-01T10:00:05Z".into()) },
        ];
        let hash = record_conversation(&store, &messages, Some("test"), Some("test")).unwrap();
        let run = match store.get(&hash).unwrap() { MorphObject::Run(r) => r, _ => panic!("expected Run") };
        let trace_hash = Hash::from_hex(&run.trace).unwrap();
        let trace = match store.get(&trace_hash).unwrap() { MorphObject::Trace(t) => t, _ => panic!("expected Trace") };

        assert_eq!(trace.events[0].ts, "2026-01-01T10:00:00Z");
        assert_eq!(trace.events[1].ts, "2026-01-01T10:00:05Z");
    }

    #[test]
    fn record_conversation_opencode_style_tool_parts() {
        // Simulates the JSON the OpenCode plugin emits on session.idle:
        // a user prompt, an assistant text part, a tool_call + tool_result for
        // a Read, a file_edit for Write, and a usage event. Asserts that every
        // structured kind survives to the trace with the right metadata so tap
        // can reconstruct the session.
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().to_path_buf());
        let json = r#"[
          {"role":"user","content":"Fix the bug in auth.rs"},
          {"role":"assistant","content":"Let me read the file first."},
          {"role":"file_read","content":"src/auth.rs","metadata":{"name":"read","call_id":"c1","path":"src/auth.rs","status":"pending","input":"{\"filePath\":\"src/auth.rs\"}"},"timestamp":"2026-04-18T10:00:00Z"},
          {"role":"tool_result","content":"fn login() { /* buggy */ }","metadata":{"name":"read","call_id":"c1","path":"src/auth.rs"},"timestamp":"2026-04-18T10:00:01Z"},
          {"role":"assistant","content":"Applying fix."},
          {"role":"file_edit","content":"fn login() { /* fixed */ }","metadata":{"name":"write","call_id":"c2","path":"src/auth.rs","status":"pending","new_content":"fn login() { /* fixed */ }","input":"{\"filePath\":\"src/auth.rs\"}"},"timestamp":"2026-04-18T10:00:02Z"},
          {"role":"tool_result","content":"file written","metadata":{"name":"write","call_id":"c2","path":"src/auth.rs"},"timestamp":"2026-04-18T10:00:03Z"},
          {"role":"tool_call","content":"{\"command\":\"cargo test\"}","metadata":{"name":"bash","call_id":"c3","status":"pending","input":"{\"command\":\"cargo test\"}"},"timestamp":"2026-04-18T10:00:04Z"},
          {"role":"tool_result","content":"test result: ok. 1 passed","metadata":{"name":"bash","call_id":"c3"},"timestamp":"2026-04-18T10:00:05Z"},
          {"role":"usage","content":"","metadata":{"input_tokens":120,"output_tokens":80,"reasoning_tokens":0}}
        ]"#;
        let messages: Vec<ConversationMessage> = serde_json::from_str(json).unwrap();
        let hash = record_conversation(
            &store, &messages, Some("anthropic/claude-opus-4"), Some("opencode"),
        ).unwrap();

        let run = match store.get(&hash).unwrap() { MorphObject::Run(r) => r, _ => panic!("expected Run") };
        assert_eq!(run.environment.model, "anthropic/claude-opus-4");
        assert_eq!(run.agent.id, "opencode");

        let trace_hash = Hash::from_hex(&run.trace).unwrap();
        let trace = match store.get(&trace_hash).unwrap() { MorphObject::Trace(t) => t, _ => panic!("expected Trace") };

        // Count kinds — every structured role must be present.
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for ev in &trace.events { *counts.entry(ev.kind.clone()).or_insert(0) += 1; }
        assert_eq!(counts.get("user").copied().unwrap_or(0), 1);
        assert_eq!(counts.get("assistant").copied().unwrap_or(0), 2);
        assert_eq!(counts.get("file_read").copied().unwrap_or(0), 1);
        assert_eq!(counts.get("file_edit").copied().unwrap_or(0), 1);
        assert_eq!(counts.get("tool_call").copied().unwrap_or(0), 1);
        assert_eq!(counts.get("tool_result").copied().unwrap_or(0), 3);
        assert_eq!(counts.get("usage").copied().unwrap_or(0), 1);

        // Path metadata should propagate so tap can surface file context.
        let first_file_read = trace.events.iter().find(|e| e.kind == "file_read").unwrap();
        assert_eq!(first_file_read.payload["path"].as_str().unwrap(), "src/auth.rs");
        assert_eq!(first_file_read.payload["name"].as_str().unwrap(), "read");
        assert_eq!(first_file_read.payload["call_id"].as_str().unwrap(), "c1");

        // Per-message timestamps preserved.
        assert_eq!(trace.events[2].ts, "2026-04-18T10:00:00Z");
        assert_eq!(trace.events[3].ts, "2026-04-18T10:00:01Z");

        // Usage event carries tokens in the payload.
        let usage = trace.events.iter().find(|e| e.kind == "usage").unwrap();
        assert_eq!(usage.payload["input_tokens"].as_i64().unwrap(), 120);
        assert_eq!(usage.payload["output_tokens"].as_i64().unwrap(), 80);

        // Tap must also see this as a non-shallow trace with tools+files.
        let diag = crate::tap::diagnose_run(&store, &hash).unwrap();
        assert!(diag.has_tool_calls, "diag should detect tool_call");
        assert!(diag.has_file_reads, "diag should detect file_read");
        assert!(diag.has_file_edits, "diag should detect file_edit");
        assert!(diag.has_tool_output, "diag should detect tool_result");
    }

    #[test]
    fn record_conversation_backward_compat_no_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path().to_path_buf());
        let json = r#"[{"role":"user","content":"Hello"},{"role":"assistant","content":"Hi"}]"#;
        let messages: Vec<ConversationMessage> = serde_json::from_str(json).unwrap();
        assert!(messages[0].metadata.is_empty());
        assert!(messages[0].timestamp.is_none());
        let hash = record_conversation(&store, &messages, None, None).unwrap();
        assert!(store.get(&hash).is_ok());
    }

    #[test]
    fn record_conversation_links_to_head_commit() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::init_repo(dir.path()).unwrap();
        let commit_hash = crate::create_tree_commit(
            &store, dir.path(), None, None,
            BTreeMap::new(), "initial".into(), None, None,
        ).unwrap();

        let hash = record_session(&store, "p", "r", None, None).unwrap();
        let run = match store.get(&hash).unwrap() { MorphObject::Run(r) => r, _ => panic!("expected Run") };
        assert_eq!(run.commit.as_deref(), Some(commit_hash.to_string().as_str()));
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
