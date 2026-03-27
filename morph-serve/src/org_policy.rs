//! Organization-level policy: default behavioral requirements above per-repo config.
//!
//! The org policy is optional and loaded from a JSON file at service startup.
//! It provides default required_metrics, thresholds, and named presets that
//! repos can reference. The effective policy for a repo is the union of
//! org defaults and repo-level overrides.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Organization-level behavioral policy.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OrgPolicy {
    #[serde(default)]
    pub required_metrics: Vec<String>,
    #[serde(default)]
    pub thresholds: BTreeMap<String, f64>,
    #[serde(default)]
    pub directions: BTreeMap<String, String>,
    #[serde(default)]
    pub presets: BTreeMap<String, PolicyPreset>,
}

/// A named policy preset that repos can adopt.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyPreset {
    pub required_metrics: Vec<String>,
    #[serde(default)]
    pub thresholds: BTreeMap<String, f64>,
}

/// Read an org policy from a JSON file. Returns None if the path doesn't exist.
pub fn load_org_policy(path: &Path) -> Result<Option<OrgPolicy>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(path).map_err(|e| format!("read org policy: {}", e))?;
    let policy: OrgPolicy =
        serde_json::from_str(&data).map_err(|e| format!("parse org policy: {}", e))?;
    Ok(Some(policy))
}

/// Write an org policy to a JSON file.
pub fn save_org_policy(path: &Path, policy: &OrgPolicy) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {}", e))?;
    }
    let json =
        serde_json::to_string_pretty(policy).map_err(|e| format!("serialize org policy: {}", e))?;
    std::fs::write(path, json).map_err(|e| format!("write org policy: {}", e))?;
    Ok(())
}

/// Compute effective required metrics: org ∪ repo.
pub fn effective_required_metrics(
    org: Option<&OrgPolicy>,
    repo_required: &[String],
) -> Vec<String> {
    let mut set: BTreeMap<String, ()> = BTreeMap::new();
    if let Some(org) = org {
        for m in &org.required_metrics {
            set.insert(m.clone(), ());
        }
    }
    for m in repo_required {
        set.insert(m.clone(), ());
    }
    set.into_keys().collect()
}

/// Compute effective thresholds: org defaults, repo overrides win.
pub fn effective_thresholds(
    org: Option<&OrgPolicy>,
    repo_thresholds: &BTreeMap<String, f64>,
) -> BTreeMap<String, f64> {
    let mut merged = BTreeMap::new();
    if let Some(org) = org {
        for (k, v) in &org.thresholds {
            merged.insert(k.clone(), *v);
        }
    }
    for (k, v) in repo_thresholds {
        merged.insert(k.clone(), *v);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_metrics_union() {
        let org = OrgPolicy {
            required_metrics: vec!["acc".into(), "latency".into()],
            ..Default::default()
        };
        let repo = vec!["acc".into(), "f1".into()];
        let effective = effective_required_metrics(Some(&org), &repo);
        assert_eq!(effective, vec!["acc", "f1", "latency"]);
    }

    #[test]
    fn effective_thresholds_repo_overrides() {
        let org = OrgPolicy {
            thresholds: {
                let mut m = BTreeMap::new();
                m.insert("acc".into(), 0.8);
                m.insert("f1".into(), 0.7);
                m
            },
            ..Default::default()
        };
        let mut repo = BTreeMap::new();
        repo.insert("acc".into(), 0.9);
        let effective = effective_thresholds(Some(&org), &repo);
        assert_eq!(effective.get("acc"), Some(&0.9));
        assert_eq!(effective.get("f1"), Some(&0.7));
    }

    #[test]
    fn roundtrip_org_policy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("org-policy.json");
        let policy = OrgPolicy {
            required_metrics: vec!["acc".into()],
            thresholds: {
                let mut m = BTreeMap::new();
                m.insert("acc".into(), 0.8);
                m
            },
            presets: {
                let mut p = BTreeMap::new();
                p.insert(
                    "strict".into(),
                    PolicyPreset {
                        required_metrics: vec!["acc".into(), "f1".into()],
                        thresholds: {
                            let mut m = BTreeMap::new();
                            m.insert("acc".into(), 0.95);
                            m
                        },
                    },
                );
                p
            },
            ..Default::default()
        };
        save_org_policy(&path, &policy).unwrap();
        let loaded = load_org_policy(&path).unwrap().unwrap();
        assert_eq!(loaded.required_metrics, policy.required_metrics);
        assert_eq!(loaded.thresholds, policy.thresholds);
        assert!(loaded.presets.contains_key("strict"));
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let result = load_org_policy(&path).unwrap();
        assert!(result.is_none());
    }
}
