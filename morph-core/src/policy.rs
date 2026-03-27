//! Repository-level policy for behavioral certification and CI gating (Phase 6).
//!
//! The policy lives in `.morph/config.json` under the `"policy"` key.
//! It specifies which metrics are required, optional thresholds, merge policy,
//! and a default eval suite for certification.

use crate::objects::MorphObject;
use crate::store::{MorphError, Store};
use crate::Hash;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Repository-level behavioral policy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepoPolicy {
    /// Metric names that must be present for certification to pass.
    #[serde(default)]
    pub required_metrics: Vec<String>,
    /// Minimum thresholds per metric. Direction-aware: "maximize" means value >= threshold,
    /// "minimize" means value <= threshold. Default direction is "maximize".
    #[serde(default)]
    pub thresholds: BTreeMap<String, f64>,
    /// Direction overrides for threshold checks (default: "maximize").
    #[serde(default)]
    pub directions: BTreeMap<String, String>,
    /// Hash of the default eval suite for certification.
    #[serde(default)]
    pub default_eval_suite: Option<String>,
    /// Merge policy mode: "dominance" (default) or "none".
    #[serde(default = "default_merge_policy")]
    pub merge_policy: String,
    /// Default CI runner metadata.
    #[serde(default)]
    pub ci_defaults: BTreeMap<String, String>,
}

fn default_merge_policy() -> String {
    "dominance".to_string()
}

impl Default for RepoPolicy {
    fn default() -> Self {
        RepoPolicy {
            required_metrics: Vec::new(),
            thresholds: BTreeMap::new(),
            directions: BTreeMap::new(),
            default_eval_suite: None,
            merge_policy: default_merge_policy(),
            ci_defaults: BTreeMap::new(),
        }
    }
}

/// Outcome of a certification attempt.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CertificationResult {
    pub passed: bool,
    pub commit: String,
    pub metrics_provided: BTreeMap<String, f64>,
    pub failures: Vec<String>,
    #[serde(default)]
    pub runner: Option<String>,
    #[serde(default)]
    pub eval_suite: Option<String>,
}

/// Outcome of a gate check.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateResult {
    pub passed: bool,
    pub commit: String,
    pub reasons: Vec<String>,
}

// ── config I/O ───────────────────────────────────────────────────────

const CONFIG_FILE: &str = "config.json";
const POLICY_KEY: &str = "policy";

/// Read the repository policy from `.morph/config.json`. Returns default policy if absent.
pub fn read_policy(morph_dir: &Path) -> Result<RepoPolicy, MorphError> {
    let config_path = morph_dir.join(CONFIG_FILE);
    if !config_path.exists() {
        return Ok(RepoPolicy::default());
    }
    let data = std::fs::read_to_string(&config_path)?;
    let config: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| MorphError::Serialization(e.to_string()))?;
    match config.get(POLICY_KEY) {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| MorphError::Serialization(format!("invalid policy: {}", e))),
        None => Ok(RepoPolicy::default()),
    }
}

/// Write the repository policy into `.morph/config.json`, preserving other keys.
pub fn write_policy(morph_dir: &Path, policy: &RepoPolicy) -> Result<(), MorphError> {
    let config_path = morph_dir.join(CONFIG_FILE);
    let mut config: serde_json::Value = if config_path.exists() {
        let data = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&data).map_err(|e| MorphError::Serialization(e.to_string()))?
    } else {
        serde_json::json!({})
    };

    let policy_value =
        serde_json::to_value(policy).map_err(|e| MorphError::Serialization(e.to_string()))?;
    config
        .as_object_mut()
        .ok_or_else(|| MorphError::Serialization("config.json is not an object".into()))?
        .insert(POLICY_KEY.to_string(), policy_value);

    let pretty =
        serde_json::to_string_pretty(&config).map_err(|e| MorphError::Serialization(e.to_string()))?;
    std::fs::write(&config_path, pretty)?;
    Ok(())
}

// ── certification ────────────────────────────────────────────────────

/// Certify a commit against the repository policy using externally produced metrics.
///
/// Validates:
/// 1. All required metrics are present.
/// 2. All thresholds are satisfied (direction-aware).
///
/// Records the result as an Annotation (kind "certification") on the commit.
pub fn certify_commit(
    store: &dyn Store,
    morph_dir: &Path,
    commit_hash: &Hash,
    metrics: &BTreeMap<String, f64>,
    runner: Option<&str>,
    eval_suite: Option<&str>,
) -> Result<CertificationResult, MorphError> {
    let obj = store.get(commit_hash)?;
    match &obj {
        MorphObject::Commit(_) => {}
        _ => {
            return Err(MorphError::Serialization(format!(
                "object {} is not a commit",
                commit_hash
            )));
        }
    }

    let policy = read_policy(morph_dir)?;
    let mut failures = Vec::new();

    for name in &policy.required_metrics {
        if !metrics.contains_key(name) {
            failures.push(format!("missing required metric: {}", name));
        }
    }

    for (name, &threshold) in &policy.thresholds {
        if let Some(&val) = metrics.get(name) {
            let dir = policy
                .directions
                .get(name)
                .map(|s| s.as_str())
                .unwrap_or("maximize");
            let passes = if dir == "minimize" {
                val <= threshold
            } else {
                val >= threshold
            };
            if !passes {
                let op = if dir == "minimize" { "<=" } else { ">=" };
                failures.push(format!(
                    "metric '{}': {} does not satisfy threshold {} {} (direction: {})",
                    name, val, op, threshold, dir
                ));
            }
        }
    }

    let passed = failures.is_empty();

    let result = CertificationResult {
        passed,
        commit: commit_hash.to_string(),
        metrics_provided: metrics.clone(),
        failures: failures.clone(),
        runner: runner.map(String::from),
        eval_suite: eval_suite.map(String::from),
    };

    let mut ann_data = BTreeMap::new();
    ann_data.insert(
        "result".to_string(),
        serde_json::to_value(&result).unwrap_or_default(),
    );
    ann_data.insert(
        "metrics".to_string(),
        serde_json::to_value(metrics).unwrap_or_default(),
    );
    ann_data.insert(
        "passed".to_string(),
        serde_json::Value::Bool(passed),
    );
    if let Some(r) = runner {
        ann_data.insert("runner".to_string(), serde_json::Value::String(r.to_string()));
    }
    if let Some(es) = eval_suite {
        ann_data.insert("eval_suite".to_string(), serde_json::Value::String(es.to_string()));
    }

    let ann = crate::annotate::create_annotation(
        commit_hash,
        None,
        "certification".to_string(),
        ann_data,
        runner.map(String::from),
    );
    store.put(&ann)?;

    Ok(result)
}

// ── gate ─────────────────────────────────────────────────────────────

/// Check whether a commit satisfies the repository's behavioral policy.
///
/// Checks:
/// 1. The commit has the required metrics in its eval_contract.observed_metrics.
/// 2. The commit's metrics satisfy configured thresholds.
/// 3. The commit has been certified (has a "certification" annotation with passed=true).
pub fn gate_check(
    store: &dyn Store,
    morph_dir: &Path,
    commit_hash: &Hash,
) -> Result<GateResult, MorphError> {
    let obj = store.get(commit_hash)?;
    let commit = match obj {
        MorphObject::Commit(c) => c,
        _ => {
            return Err(MorphError::Serialization(format!(
                "object {} is not a commit",
                commit_hash
            )));
        }
    };

    let policy = read_policy(morph_dir)?;
    let mut reasons = Vec::new();

    for name in &policy.required_metrics {
        if !commit.eval_contract.observed_metrics.contains_key(name) {
            let certified_metrics = find_certification_metrics(store, commit_hash)?;
            if !certified_metrics.contains_key(name) {
                reasons.push(format!("missing required metric: {}", name));
            }
        }
    }

    let all_metrics = {
        let mut m = commit.eval_contract.observed_metrics.clone();
        let certified = find_certification_metrics(store, commit_hash)?;
        for (k, v) in certified {
            m.entry(k).or_insert(v);
        }
        m
    };

    for (name, &threshold) in &policy.thresholds {
        if let Some(&val) = all_metrics.get(name) {
            let dir = policy
                .directions
                .get(name)
                .map(|s| s.as_str())
                .unwrap_or("maximize");
            let passes = if dir == "minimize" {
                val <= threshold
            } else {
                val >= threshold
            };
            if !passes {
                let op = if dir == "minimize" { "<=" } else { ">=" };
                reasons.push(format!(
                    "metric '{}': {} does not satisfy threshold {} {} (direction: {})",
                    name, val, op, threshold, dir
                ));
            }
        }
    }

    if !has_passing_certification(store, commit_hash)? {
        reasons.push("commit is not certified (no passing certification annotation found)".to_string());
    }

    let passed = reasons.is_empty();
    Ok(GateResult {
        passed,
        commit: commit_hash.to_string(),
        reasons,
    })
}

/// Find the most recent certification annotation on a commit and extract its metrics.
fn find_certification_metrics(
    store: &dyn Store,
    commit_hash: &Hash,
) -> Result<BTreeMap<String, f64>, MorphError> {
    let annotations = crate::annotate::list_annotations(store, commit_hash, None)?;
    for (_hash, ann) in annotations.iter().rev() {
        if ann.kind == "certification" {
            if let Some(metrics_val) = ann.data.get("metrics") {
                if let Ok(metrics) = serde_json::from_value::<BTreeMap<String, f64>>(metrics_val.clone()) {
                    return Ok(metrics);
                }
            }
        }
    }
    Ok(BTreeMap::new())
}

/// Check whether a commit has at least one passing certification annotation.
fn has_passing_certification(
    store: &dyn Store,
    commit_hash: &Hash,
) -> Result<bool, MorphError> {
    let annotations = crate::annotate::list_annotations(store, commit_hash, None)?;
    for (_hash, ann) in &annotations {
        if ann.kind == "certification" {
            if let Some(passed) = ann.data.get("passed") {
                if passed.as_bool() == Some(true) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{Blob, EvalContract, EvalSuite};

    fn setup_repo() -> (tempfile::TempDir, Box<dyn Store>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _ = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();
        (dir, store)
    }

    fn make_commit(store: &dyn Store, dir: &tempfile::TempDir, metrics: BTreeMap<String, f64>) -> Hash {
        let root = dir.path();
        std::fs::write(root.join("f.txt"), "data").unwrap();
        crate::add_paths(store, root, &[std::path::PathBuf::from(".")]).unwrap();
        crate::create_tree_commit(
            store, root, None, None, metrics, "test commit".into(), None, Some("0.3"),
        ).unwrap()
    }

    #[test]
    fn policy_round_trip() {
        let (dir, _store) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        let policy = RepoPolicy {
            required_metrics: vec!["acc".into(), "f1".into()],
            thresholds: {
                let mut m = BTreeMap::new();
                m.insert("acc".into(), 0.8);
                m
            },
            directions: BTreeMap::new(),
            default_eval_suite: Some("abc123".into()),
            merge_policy: "dominance".into(),
            ci_defaults: {
                let mut m = BTreeMap::new();
                m.insert("runner".into(), "ci-v1".into());
                m
            },
        };

        write_policy(&morph_dir, &policy).unwrap();
        let read_back = read_policy(&morph_dir).unwrap();
        assert_eq!(read_back.required_metrics, policy.required_metrics);
        assert_eq!(read_back.thresholds, policy.thresholds);
        assert_eq!(read_back.default_eval_suite, policy.default_eval_suite);
        assert_eq!(read_back.merge_policy, policy.merge_policy);
        assert_eq!(read_back.ci_defaults, policy.ci_defaults);
    }

    #[test]
    fn policy_preserves_other_config_keys() {
        let (dir, _store) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        let config_path = morph_dir.join("config.json");
        let existing: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&config_path).unwrap()
        ).unwrap();
        assert!(existing.get("repo_version").is_some());

        let policy = RepoPolicy {
            required_metrics: vec!["acc".into()],
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let after: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&config_path).unwrap()
        ).unwrap();
        assert!(after.get("repo_version").is_some(), "repo_version should be preserved");
        assert!(after.get("policy").is_some(), "policy should be present");
    }

    #[test]
    fn default_policy_when_absent() {
        let (dir, _store) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        let policy = read_policy(&morph_dir).unwrap();
        assert!(policy.required_metrics.is_empty());
        assert!(policy.thresholds.is_empty());
        assert_eq!(policy.merge_policy, "dominance");
    }

    #[test]
    fn certify_passes_with_valid_metrics() {
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        let policy = RepoPolicy {
            required_metrics: vec!["acc".into()],
            thresholds: {
                let mut m = BTreeMap::new();
                m.insert("acc".into(), 0.8);
                m
            },
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let mut metrics = BTreeMap::new();
        metrics.insert("acc".into(), 0.9);
        let commit_hash = make_commit(store.as_ref(), &dir, BTreeMap::new());

        let result = certify_commit(
            store.as_ref(), &morph_dir, &commit_hash, &metrics, Some("ci-v1"), None,
        ).unwrap();
        assert!(result.passed, "certification should pass: {:?}", result.failures);
    }

    #[test]
    fn certify_fails_when_required_metrics_missing() {
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        let policy = RepoPolicy {
            required_metrics: vec!["acc".into(), "f1".into()],
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let mut metrics = BTreeMap::new();
        metrics.insert("acc".into(), 0.9);
        let commit_hash = make_commit(store.as_ref(), &dir, BTreeMap::new());

        let result = certify_commit(
            store.as_ref(), &morph_dir, &commit_hash, &metrics, None, None,
        ).unwrap();
        assert!(!result.passed);
        assert!(result.failures.iter().any(|f| f.contains("f1")));
    }

    #[test]
    fn certify_fails_when_thresholds_not_met() {
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        let policy = RepoPolicy {
            required_metrics: vec!["acc".into()],
            thresholds: {
                let mut m = BTreeMap::new();
                m.insert("acc".into(), 0.9);
                m
            },
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let mut metrics = BTreeMap::new();
        metrics.insert("acc".into(), 0.8);
        let commit_hash = make_commit(store.as_ref(), &dir, BTreeMap::new());

        let result = certify_commit(
            store.as_ref(), &morph_dir, &commit_hash, &metrics, None, None,
        ).unwrap();
        assert!(!result.passed);
        assert!(result.failures.iter().any(|f| f.contains("acc") && f.contains("threshold")));
    }

    #[test]
    fn certify_respects_minimize_direction() {
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        let policy = RepoPolicy {
            required_metrics: vec!["latency".into()],
            thresholds: {
                let mut m = BTreeMap::new();
                m.insert("latency".into(), 2.0);
                m
            },
            directions: {
                let mut m = BTreeMap::new();
                m.insert("latency".into(), "minimize".into());
                m
            },
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let commit_hash = make_commit(store.as_ref(), &dir, BTreeMap::new());

        let mut good = BTreeMap::new();
        good.insert("latency".into(), 1.5);
        let result = certify_commit(
            store.as_ref(), &morph_dir, &commit_hash, &good, None, None,
        ).unwrap();
        assert!(result.passed);

        let commit_hash2 = make_commit(store.as_ref(), &dir, BTreeMap::new());
        let mut bad = BTreeMap::new();
        bad.insert("latency".into(), 3.0);
        let result2 = certify_commit(
            store.as_ref(), &morph_dir, &commit_hash2, &bad, None, None,
        ).unwrap();
        assert!(!result2.passed);
    }

    #[test]
    fn gate_passes_for_certified_commit() {
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        let policy = RepoPolicy {
            required_metrics: vec!["acc".into()],
            thresholds: {
                let mut m = BTreeMap::new();
                m.insert("acc".into(), 0.8);
                m
            },
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let mut metrics = BTreeMap::new();
        metrics.insert("acc".into(), 0.9);
        let commit_hash = make_commit(store.as_ref(), &dir, metrics.clone());

        certify_commit(
            store.as_ref(), &morph_dir, &commit_hash, &metrics, Some("ci"), None,
        ).unwrap();

        let gate = gate_check(store.as_ref(), &morph_dir, &commit_hash).unwrap();
        assert!(gate.passed, "gate should pass: {:?}", gate.reasons);
    }

    #[test]
    fn gate_fails_for_uncertified_commit() {
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        let policy = RepoPolicy {
            required_metrics: vec!["acc".into()],
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let mut metrics = BTreeMap::new();
        metrics.insert("acc".into(), 0.9);
        let commit_hash = make_commit(store.as_ref(), &dir, metrics);

        let gate = gate_check(store.as_ref(), &morph_dir, &commit_hash).unwrap();
        assert!(!gate.passed);
        assert!(gate.reasons.iter().any(|r| r.contains("not certified")));
    }

    #[test]
    fn gate_fails_when_metrics_missing() {
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        let policy = RepoPolicy {
            required_metrics: vec!["acc".into(), "f1".into()],
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let mut metrics = BTreeMap::new();
        metrics.insert("acc".into(), 0.9);
        let commit_hash = make_commit(store.as_ref(), &dir, metrics.clone());

        certify_commit(
            store.as_ref(), &morph_dir, &commit_hash, &metrics, Some("ci"), None,
        ).unwrap();

        let gate = gate_check(store.as_ref(), &morph_dir, &commit_hash).unwrap();
        assert!(!gate.passed);
        assert!(gate.reasons.iter().any(|r| r.contains("f1")));
    }

    #[test]
    fn gate_output_identifies_failure_reason() {
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        let policy = RepoPolicy {
            required_metrics: vec!["acc".into()],
            thresholds: {
                let mut m = BTreeMap::new();
                m.insert("acc".into(), 0.95);
                m
            },
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let mut metrics = BTreeMap::new();
        metrics.insert("acc".into(), 0.8);
        let commit_hash = make_commit(store.as_ref(), &dir, metrics.clone());

        certify_commit(
            store.as_ref(), &morph_dir, &commit_hash, &metrics, Some("ci"), None,
        ).unwrap();

        let gate = gate_check(store.as_ref(), &morph_dir, &commit_hash).unwrap();
        assert!(!gate.passed);
        let all_reasons = gate.reasons.join("; ");
        assert!(all_reasons.contains("threshold"), "should mention threshold: {}", all_reasons);
    }

    #[test]
    fn certify_records_annotation() {
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        let policy = RepoPolicy::default();
        write_policy(&morph_dir, &policy).unwrap();

        let metrics = BTreeMap::new();
        let commit_hash = make_commit(store.as_ref(), &dir, BTreeMap::new());

        certify_commit(
            store.as_ref(), &morph_dir, &commit_hash, &metrics, Some("ci-runner"), None,
        ).unwrap();

        let anns = crate::annotate::list_annotations(store.as_ref(), &commit_hash, None).unwrap();
        assert!(!anns.is_empty(), "should have at least one annotation");
        let (_, ann) = &anns[0];
        assert_eq!(ann.kind, "certification");
        assert_eq!(ann.data.get("passed").and_then(|v| v.as_bool()), Some(true));
    }
}
