//! Metrics validation: aggregation and threshold checking (v0-spec §4.4).
//! Morph does not run evaluations; it validates reported scores.

use crate::objects::EvalSuite;
use crate::store::MorphError;
use std::collections::BTreeMap;

/// Aggregate per-case scores into a single value.
/// Built-in methods: mean, min, p95, lower_ci_bound.
pub fn aggregate(scores: &[f64], method: &str) -> Result<f64, MorphError> {
    if scores.is_empty() {
        return Err(MorphError::Serialization("empty scores".into()));
    }
    let out = match method {
        "mean" => {
            let sum: f64 = scores.iter().sum();
            sum / scores.len() as f64
        }
        "min" => scores
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min),
        "p95" => percentile(scores, 0.95),
        "lower_ci_bound" => lower_ci_95(scores),
        _ => return Err(MorphError::Serialization(format!("unknown aggregation: {}", method))),
    };
    Ok(out)
}

fn percentile(scores: &[f64], p: f64) -> f64 {
    let mut sorted: Vec<f64> = scores.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = (p * (sorted.len() as f64 - 1.0)).ceil() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn lower_ci_95(scores: &[f64]) -> f64 {
    let n = scores.len() as f64;
    if n < 2.0 {
        return scores[0];
    }
    let mean: f64 = scores.iter().sum::<f64>() / n;
    let variance = scores.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let se = (variance / n).sqrt();
    let t_approx = 1.96;
    mean - t_approx * se
}

/// Check that observed metrics meet or exceed each metric's threshold in the suite.
pub fn check_thresholds(
    observed: &BTreeMap<String, f64>,
    suite: &EvalSuite,
) -> Result<bool, MorphError> {
    for m in &suite.metrics {
        let val = observed.get(&m.name).ok_or_else(|| {
            MorphError::Serialization(format!("missing metric: {}", m.name))
        })?;
        if *val < m.threshold {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Check that merged metrics dominate parent (merged >= parent for every key in parent).
/// Used for merge (v0-spec §6.8).
pub fn check_dominance(
    merged: &BTreeMap<String, f64>,
    parent: &BTreeMap<String, f64>,
) -> bool {
    parent.iter().all(|(k, v)| merged.get(k).map_or(false, |m| *m >= *v))
}

/// Compute aggregated metrics from per-case scores using suite's aggregation methods.
pub fn aggregate_suite(
    per_case: &BTreeMap<String, Vec<f64>>,
    suite: &EvalSuite,
) -> Result<BTreeMap<String, f64>, MorphError> {
    let mut out = BTreeMap::new();
    for m in &suite.metrics {
        let scores = per_case.get(&m.name).ok_or_else(|| {
            MorphError::Serialization(format!("missing per-case scores for metric: {}", m.name))
        })?;
        let agg = aggregate(scores, &m.aggregation)?;
        out.insert(m.name.clone(), agg);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::EvalMetric;

    #[test]
    fn aggregate_mean() {
        let s = [1.0, 2.0, 3.0];
        assert!((aggregate(&s, "mean").unwrap() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn aggregate_min() {
        let s = [1.0, 2.0, 3.0];
        assert_eq!(aggregate(&s, "min").unwrap(), 1.0);
    }

    #[test]
    fn aggregate_p95() {
        let s: Vec<f64> = (0..100).map(|i| i as f64).collect();
        let p95 = aggregate(&s, "p95").unwrap();
        assert!(p95 >= 94.0 && p95 <= 96.0);
    }

    #[test]
    fn check_thresholds_pass() {
        let suite = EvalSuite {
            cases: vec![],
            metrics: vec![
                EvalMetric { name: "acc".into(), aggregation: "mean".into(), threshold: 0.8 },
                EvalMetric { name: "f1".into(), aggregation: "mean".into(), threshold: 0.0 },
            ],
        };
        let mut obs = BTreeMap::new();
        obs.insert("acc".to_string(), 0.9);
        obs.insert("f1".to_string(), 0.85);
        assert!(check_thresholds(&obs, &suite).unwrap());
    }

    #[test]
    fn check_thresholds_fail() {
        let suite = EvalSuite {
            cases: vec![],
            metrics: vec![EvalMetric { name: "acc".into(), aggregation: "mean".into(), threshold: 0.9 }],
        };
        let mut obs = BTreeMap::new();
        obs.insert("acc".to_string(), 0.8);
        assert!(!check_thresholds(&obs, &suite).unwrap());
    }

    #[test]
    fn dominance_reflexive() {
        let mut m = BTreeMap::new();
        m.insert("a".into(), 1.0);
        assert!(check_dominance(&m, &m));
    }

    #[test]
    fn dominance_merged_greater() {
        let mut merged = BTreeMap::new();
        merged.insert("acc".into(), 0.95);
        let mut parent = BTreeMap::new();
        parent.insert("acc".into(), 0.9);
        assert!(check_dominance(&merged, &parent));
    }

    #[test]
    fn dominance_merged_less_fails() {
        let mut merged = BTreeMap::new();
        merged.insert("acc".into(), 0.8);
        let mut parent = BTreeMap::new();
        parent.insert("acc".into(), 0.9);
        assert!(!check_dominance(&merged, &parent));
    }
}
