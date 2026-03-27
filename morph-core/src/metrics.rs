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
/// Respects direction: "maximize" requires val >= threshold, "minimize" requires val <= threshold.
pub fn check_thresholds(
    observed: &BTreeMap<String, f64>,
    suite: &EvalSuite,
) -> Result<bool, MorphError> {
    for m in &suite.metrics {
        let val = observed.get(&m.name).ok_or_else(|| {
            MorphError::Serialization(format!("missing metric: {}", m.name))
        })?;
        let passes = if m.direction == "minimize" {
            *val <= m.threshold
        } else {
            *val >= m.threshold
        };
        if !passes {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Check that merged metrics dominate parent for every key in parent.
/// Assumes all metrics are "maximize" (merged >= parent). Use [check_dominance_with_suite]
/// when direction information is available.
pub fn check_dominance(
    merged: &BTreeMap<String, f64>,
    parent: &BTreeMap<String, f64>,
) -> bool {
    parent.iter().all(|(k, v)| merged.get(k).map_or(false, |m| *m >= *v))
}

/// Direction-aware dominance: merged must be "at least as good" as parent for every metric
/// in parent. For "maximize" metrics, merged >= parent. For "minimize" metrics, merged <= parent.
/// Falls back to maximize for metrics not found in the suite.
pub fn check_dominance_with_suite(
    merged: &BTreeMap<String, f64>,
    parent: &BTreeMap<String, f64>,
    suite: &EvalSuite,
) -> bool {
    let directions: BTreeMap<&str, &str> = suite
        .metrics
        .iter()
        .map(|m| (m.name.as_str(), m.direction.as_str()))
        .collect();
    parent.iter().all(|(k, v)| {
        merged.get(k).map_or(false, |m| {
            let dir = directions.get(k.as_str()).copied().unwrap_or("maximize");
            if dir == "minimize" {
                *m <= *v
            } else {
                *m >= *v
            }
        })
    })
}

/// Compute the union of two eval suites (THEORY.md §13.1: T = T1 ⊔ T2).
/// Metrics are merged by name. If both suites define the same metric name, they must agree
/// on aggregation, threshold, and direction; otherwise returns an error.
/// Cases are concatenated (deduplicated by id).
pub fn union_suites(a: &EvalSuite, b: &EvalSuite) -> Result<EvalSuite, MorphError> {
    let mut metrics_map: BTreeMap<String, &crate::objects::EvalMetric> = BTreeMap::new();
    for m in &a.metrics {
        metrics_map.insert(m.name.clone(), m);
    }
    for m in &b.metrics {
        if let Some(existing) = metrics_map.get(&m.name) {
            if existing.aggregation != m.aggregation
                || existing.direction != m.direction
                || (existing.threshold - m.threshold).abs() > f64::EPSILON
            {
                return Err(MorphError::Serialization(format!(
                    "metric '{}' defined differently in both suites",
                    m.name
                )));
            }
        } else {
            metrics_map.insert(m.name.clone(), m);
        }
    }

    let mut case_ids = std::collections::BTreeSet::new();
    let mut cases = Vec::new();
    for c in a.cases.iter().chain(b.cases.iter()) {
        if case_ids.insert(c.id.clone()) {
            cases.push(c.clone());
        }
    }

    let metrics = metrics_map.values().map(|m| (*m).clone()).collect();
    Ok(EvalSuite { cases, metrics })
}

/// Retire metrics from a suite (paper §5.3). Returns a new suite with the retired
/// metrics removed. Each retired metric name must exist in the suite.
pub fn retire_metrics(
    suite: &EvalSuite,
    retired: &[String],
) -> Result<EvalSuite, MorphError> {
    let retired_set: std::collections::BTreeSet<&str> = retired.iter().map(|s| s.as_str()).collect();
    for name in &retired_set {
        if !suite.metrics.iter().any(|m| m.name == *name) {
            return Err(MorphError::Serialization(format!(
                "cannot retire metric '{}': not found in suite",
                name
            )));
        }
    }
    let metrics = suite
        .metrics
        .iter()
        .filter(|m| !retired_set.contains(m.name.as_str()))
        .cloned()
        .collect();
    let cases = suite
        .cases
        .iter()
        .filter(|c| !retired_set.contains(c.metric.as_str()))
        .cloned()
        .collect();
    Ok(EvalSuite { cases, metrics })
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
                EvalMetric::new("acc", "mean", 0.8),
                EvalMetric::new("f1", "mean", 0.0),
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
            metrics: vec![EvalMetric::new("acc", "mean", 0.9)],
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

    #[test]
    fn check_thresholds_minimize_direction() {
        let suite = EvalSuite {
            cases: vec![],
            metrics: vec![EvalMetric {
                name: "latency".into(),
                aggregation: "p95".into(),
                threshold: 2.0,
                direction: "minimize".into(),
            }],
        };
        let mut good = BTreeMap::new();
        good.insert("latency".to_string(), 1.5);
        assert!(check_thresholds(&good, &suite).unwrap());

        let mut bad = BTreeMap::new();
        bad.insert("latency".to_string(), 3.0);
        assert!(!check_thresholds(&bad, &suite).unwrap());
    }

    #[test]
    fn aggregate_empty_scores_err() {
        let s: [f64; 0] = [];
        assert!(aggregate(&s, "mean").is_err());
    }

    #[test]
    fn aggregate_unknown_method_err() {
        let s = [1.0, 2.0];
        assert!(aggregate(&s, "unknown").is_err());
    }

    #[test]
    fn aggregate_lower_ci_bound() {
        let s = [1.0, 2.0, 3.0, 4.0, 5.0];
        let out = aggregate(&s, "lower_ci_bound").unwrap();
        assert!(out < 3.0 && out > 1.0);
    }

    #[test]
    fn aggregate_suite_roundtrip() {
        let suite = EvalSuite {
            cases: vec![],
            metrics: vec![
                EvalMetric::new("a", "mean", 0.0),
                EvalMetric::new("b", "min", 0.0),
            ],
        };
        let mut per_case = BTreeMap::new();
        per_case.insert("a".into(), vec![1.0, 2.0, 3.0]);
        per_case.insert("b".into(), vec![5.0, 1.0, 3.0]);
        let out = aggregate_suite(&per_case, &suite).unwrap();
        assert!((out.get("a").copied().unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(out.get("b").copied().unwrap(), 1.0);
    }

    // ── check_dominance_with_suite ───────────────────────────────────

    #[test]
    fn dominance_with_suite_maximize() {
        let suite = EvalSuite {
            cases: vec![],
            metrics: vec![EvalMetric::new("acc", "mean", 0.0)],
        };
        let mut merged = BTreeMap::new();
        merged.insert("acc".into(), 0.95);
        let mut parent = BTreeMap::new();
        parent.insert("acc".into(), 0.9);
        assert!(check_dominance_with_suite(&merged, &parent, &suite));
    }

    #[test]
    fn dominance_with_suite_minimize() {
        let suite = EvalSuite {
            cases: vec![],
            metrics: vec![EvalMetric {
                name: "latency".into(),
                aggregation: "p95".into(),
                threshold: 5.0,
                direction: "minimize".into(),
            }],
        };
        let mut merged_good = BTreeMap::new();
        merged_good.insert("latency".into(), 1.0);
        let mut parent = BTreeMap::new();
        parent.insert("latency".into(), 2.0);
        assert!(check_dominance_with_suite(&merged_good, &parent, &suite));

        let mut merged_bad = BTreeMap::new();
        merged_bad.insert("latency".into(), 3.0);
        assert!(!check_dominance_with_suite(&merged_bad, &parent, &suite));
    }

    #[test]
    fn dominance_with_suite_mixed_directions() {
        let suite = EvalSuite {
            cases: vec![],
            metrics: vec![
                EvalMetric::new("acc", "mean", 0.0),
                EvalMetric {
                    name: "cost".into(),
                    aggregation: "mean".into(),
                    threshold: 10.0,
                    direction: "minimize".into(),
                },
            ],
        };
        let mut parent = BTreeMap::new();
        parent.insert("acc".into(), 0.9);
        parent.insert("cost".into(), 5.0);

        let mut both_better = BTreeMap::new();
        both_better.insert("acc".into(), 0.95);
        both_better.insert("cost".into(), 3.0);
        assert!(check_dominance_with_suite(&both_better, &parent, &suite));

        let mut acc_worse = BTreeMap::new();
        acc_worse.insert("acc".into(), 0.85);
        acc_worse.insert("cost".into(), 3.0);
        assert!(!check_dominance_with_suite(&acc_worse, &parent, &suite));

        let mut cost_worse = BTreeMap::new();
        cost_worse.insert("acc".into(), 0.95);
        cost_worse.insert("cost".into(), 7.0);
        assert!(!check_dominance_with_suite(&cost_worse, &parent, &suite));
    }

    // ── union_suites ─────────────────────────────────────────────────

    #[test]
    fn union_suites_disjoint() {
        let a = EvalSuite {
            cases: vec![],
            metrics: vec![EvalMetric::new("acc", "mean", 0.8)],
        };
        let b = EvalSuite {
            cases: vec![],
            metrics: vec![EvalMetric::new("f1", "mean", 0.7)],
        };
        let u = union_suites(&a, &b).unwrap();
        assert_eq!(u.metrics.len(), 2);
        assert!(u.metrics.iter().any(|m| m.name == "acc"));
        assert!(u.metrics.iter().any(|m| m.name == "f1"));
    }

    #[test]
    fn union_suites_overlapping_identical() {
        let m = EvalMetric::new("acc", "mean", 0.8);
        let a = EvalSuite { cases: vec![], metrics: vec![m.clone()] };
        let b = EvalSuite { cases: vec![], metrics: vec![m] };
        let u = union_suites(&a, &b).unwrap();
        assert_eq!(u.metrics.len(), 1);
    }

    #[test]
    fn union_suites_conflicting_threshold_errors() {
        let a = EvalSuite {
            cases: vec![],
            metrics: vec![EvalMetric::new("acc", "mean", 0.8)],
        };
        let b = EvalSuite {
            cases: vec![],
            metrics: vec![EvalMetric::new("acc", "mean", 0.9)],
        };
        assert!(union_suites(&a, &b).is_err());
    }

    #[test]
    fn union_suites_deduplicates_cases() {
        let case = crate::objects::EvalCase {
            id: "c1".into(),
            input: serde_json::json!({}),
            expected: serde_json::json!({}),
            metric: "acc".into(),
            fixture_source: "candidate".into(),
        };
        let a = EvalSuite { cases: vec![case.clone()], metrics: vec![] };
        let b = EvalSuite { cases: vec![case], metrics: vec![] };
        let u = union_suites(&a, &b).unwrap();
        assert_eq!(u.cases.len(), 1);
    }

    // ── retire_metrics (paper §5.3) ──────────────────────────────────

    #[test]
    fn retire_removes_metric() {
        let suite = EvalSuite {
            cases: vec![],
            metrics: vec![
                EvalMetric::new("acc", "mean", 0.8),
                EvalMetric::new("f1", "mean", 0.7),
            ],
        };
        let retired = retire_metrics(&suite, &["f1".to_string()]).unwrap();
        assert_eq!(retired.metrics.len(), 1);
        assert_eq!(retired.metrics[0].name, "acc");
    }

    #[test]
    fn retire_removes_associated_cases() {
        let suite = EvalSuite {
            cases: vec![
                crate::objects::EvalCase {
                    id: "c1".into(),
                    input: serde_json::json!({}),
                    expected: serde_json::json!({}),
                    metric: "acc".into(),
                    fixture_source: "candidate".into(),
                },
                crate::objects::EvalCase {
                    id: "c2".into(),
                    input: serde_json::json!({}),
                    expected: serde_json::json!({}),
                    metric: "f1".into(),
                    fixture_source: "candidate".into(),
                },
            ],
            metrics: vec![
                EvalMetric::new("acc", "mean", 0.8),
                EvalMetric::new("f1", "mean", 0.7),
            ],
        };
        let retired = retire_metrics(&suite, &["f1".to_string()]).unwrap();
        assert_eq!(retired.cases.len(), 1);
        assert_eq!(retired.cases[0].metric, "acc");
    }

    #[test]
    fn retire_nonexistent_metric_errors() {
        let suite = EvalSuite {
            cases: vec![],
            metrics: vec![EvalMetric::new("acc", "mean", 0.8)],
        };
        assert!(retire_metrics(&suite, &["bogus".to_string()]).is_err());
    }

    #[test]
    fn retire_all_metrics_ok() {
        let suite = EvalSuite {
            cases: vec![],
            metrics: vec![EvalMetric::new("acc", "mean", 0.8)],
        };
        let retired = retire_metrics(&suite, &["acc".to_string()]).unwrap();
        assert!(retired.metrics.is_empty());
    }

    // ── check_dominance edge cases ───────────────────────────────────

    #[test]
    fn dominance_merged_has_extra_metrics() {
        let mut merged = BTreeMap::new();
        merged.insert("acc".into(), 0.95);
        merged.insert("f1".into(), 0.90);
        let mut parent = BTreeMap::new();
        parent.insert("acc".into(), 0.9);
        assert!(check_dominance(&merged, &parent), "superset should dominate");
    }

    #[test]
    fn dominance_merged_missing_parent_metric_fails() {
        let mut merged = BTreeMap::new();
        merged.insert("f1".into(), 0.95);
        let mut parent = BTreeMap::new();
        parent.insert("acc".into(), 0.9);
        assert!(!check_dominance(&merged, &parent), "missing parent metric should fail");
    }

    #[test]
    fn dominance_with_suite_metric_missing_from_both() {
        let suite = EvalSuite {
            cases: vec![],
            metrics: vec![
                EvalMetric::new("acc", "mean", 0.0),
                EvalMetric::new("recall", "mean", 0.0),
            ],
        };
        let mut merged = BTreeMap::new();
        merged.insert("acc".into(), 0.95);
        let mut parent = BTreeMap::new();
        parent.insert("acc".into(), 0.9);
        assert!(
            check_dominance_with_suite(&merged, &parent, &suite),
            "metric missing from both should pass (only parent keys are checked)"
        );
    }

    // ── aggregate single-element slices ──────────────────────────────

    #[test]
    fn aggregate_single_mean() {
        assert!((aggregate(&[42.0], "mean").unwrap() - 42.0).abs() < 1e-9);
    }

    #[test]
    fn aggregate_single_min() {
        assert_eq!(aggregate(&[42.0], "min").unwrap(), 42.0);
    }

    #[test]
    fn aggregate_single_p95() {
        assert!((aggregate(&[42.0], "p95").unwrap() - 42.0).abs() < 1e-9);
    }

    #[test]
    fn aggregate_single_lower_ci_bound() {
        assert!((aggregate(&[42.0], "lower_ci_bound").unwrap() - 42.0).abs() < 1e-9);
    }
}
