//! Ingest execution evidence: run record and eval record (v0-spec §6.6, §6.7).

use crate::objects::MorphObject;
use crate::store::{MorphError, Store};
use crate::Hash;
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
