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
    /// PR 6 stage F: server-side push gating. Each named branch
    /// listed here must pass `gate_check` against the configured
    /// `RepoPolicy` before the bare/working repo will accept a
    /// `RefWrite` over SSH. Leave empty to gate nothing (default
    /// behavior, matching pre-PR6 servers).
    #[serde(default)]
    pub push_gated_branches: Vec<String>,
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
            push_gated_branches: Vec::new(),
        }
    }
}

/// Names from `policy.required_metrics` that are absent from `observed`.
/// Returns an empty `Vec` when the gate is satisfied. The CLI and MCP
/// commit handlers both share this check so the failure message stays
/// in sync. Order matches `required_metrics` so the user sees them in
/// the order they were configured.
pub fn missing_required_metrics(
    policy: &RepoPolicy,
    observed: &BTreeMap<String, f64>,
) -> Vec<String> {
    policy
        .required_metrics
        .iter()
        .filter(|m| !observed.contains_key(m.as_str()))
        .cloned()
        .collect()
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

// ── effective metrics (unified read path) ────────────────────────────

/// PR 1 (reference-mode foundation): the single read path every metric
/// consumer routes through.
///
/// `effective_metrics` returns the union of two layers, with the
/// **latest certification annotation winning** when a key appears in
/// both:
///
///   1. The commit's inline `eval_contract.observed_metrics` (the
///      values the author committed with).
///   2. The most recent `kind: "certification"` annotation attached
///      to the commit (late-arriving evidence — flaky-test re-runs,
///      external CI runs, manual `morph certify`).
///
/// This makes the read path mode-orthogonal (standalone and
/// reference repos behave identically) and lets a commit recorded
/// without metrics — the common case for git-hook-mirrored commits
/// in reference mode — accumulate evidence over time without
/// rewriting history.
///
/// "Latest wins" matches the lifecycle: certifications always
/// supersede inline values, because they're the more recent claim
/// about the same commit. To resurrect an older certification,
/// re-run `morph certify` — that's the only way evidence gets added.
pub fn effective_metrics(
    store: &dyn Store,
    commit_hash: &Hash,
) -> Result<BTreeMap<String, f64>, MorphError> {
    let mut effective = match store.get(commit_hash)? {
        MorphObject::Commit(c) => c.eval_contract.observed_metrics.clone(),
        _ => {
            return Err(MorphError::Serialization(format!(
                "object {} is not a commit",
                commit_hash
            )));
        }
    };
    if let Some(certified) = latest_certification_metrics(store, commit_hash)? {
        for (k, v) in certified {
            effective.insert(k, v);
        }
    }
    Ok(effective)
}

/// Like `effective_metrics`, but starting from an already-loaded
/// `Commit`. Saves a redundant store lookup on the hot merge path
/// where the caller already has the commit in hand.
pub fn effective_metrics_for_commit(
    store: &dyn Store,
    commit_hash: &Hash,
    commit: &crate::objects::Commit,
) -> Result<BTreeMap<String, f64>, MorphError> {
    let mut effective = commit.eval_contract.observed_metrics.clone();
    if let Some(certified) = latest_certification_metrics(store, commit_hash)? {
        for (k, v) in certified {
            effective.insert(k, v);
        }
    }
    Ok(effective)
}

/// Most recent certification annotation's metrics, if any.
/// `None` means "no certification on this commit"; `Some(empty)`
/// means "certified, but the certification carried no metrics" —
/// the caller should treat those distinctly only if the difference
/// matters to them.
///
/// "Most recent" is decided by `Annotation.timestamp` (RFC 3339)
/// because `list_annotations` returns annotations in hash-space
/// order, not chronological order. Sorting by timestamp keeps the
/// helper deterministic across stores even when several
/// certifications stack on the same commit (a flaky-test re-run
/// followed by a fresh CI run, for instance).
fn latest_certification_metrics(
    store: &dyn Store,
    commit_hash: &Hash,
) -> Result<Option<BTreeMap<String, f64>>, MorphError> {
    let mut annotations = crate::annotate::list_annotations(store, commit_hash, None)?;
    annotations.retain(|(_, a)| a.kind == "certification");
    if annotations.is_empty() {
        return Ok(None);
    }
    // RFC 3339 timestamps sort lexicographically the same way they
    // sort chronologically, so a string compare is enough. We use
    // `(timestamp, hash)` so two certifications written within the
    // same second get a stable tiebreaker (the bigger hash wins).
    annotations.sort_by(|(ah, a), (bh, b)| {
        a.timestamp
            .cmp(&b.timestamp)
            .then_with(|| ah.to_string().cmp(&bh.to_string()))
    });
    let (_, latest) = annotations.last().expect("non-empty after retain");
    if let Some(metrics_val) = latest.data.get("metrics") {
        if let Ok(metrics) =
            serde_json::from_value::<BTreeMap<String, f64>>(metrics_val.clone())
        {
            return Ok(Some(metrics));
        }
    }
    Ok(Some(BTreeMap::new()))
}

// ── gate ─────────────────────────────────────────────────────────────

/// Check whether a commit satisfies the repository's behavioral policy.
///
/// Checks (using `effective_metrics` so late certifications count):
/// 1. All `policy.required_metrics` are present.
/// 2. All `policy.thresholds` are satisfied (direction-aware).
/// 3. The commit has at least one passing certification annotation.
pub fn gate_check(
    store: &dyn Store,
    morph_dir: &Path,
    commit_hash: &Hash,
) -> Result<GateResult, MorphError> {
    let metrics = effective_metrics(store, commit_hash)?;

    let policy = read_policy(morph_dir)?;
    let mut reasons = Vec::new();

    for name in &policy.required_metrics {
        if !metrics.contains_key(name) {
            reasons.push(format!("missing required metric: {}", name));
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

// ── Push gating (PR 6 stage F) ────────────────────────────────────────

/// Extract the branch name from a ref name. Only `heads/<name>`
/// participates in the push gate; tag and remote-tracking refs are
/// untouched.
fn branch_from_ref(ref_name: &str) -> Option<&str> {
    ref_name.strip_prefix("heads/")
}

/// PR 9: match a branch name against a single `push_gated_branches`
/// pattern. The grammar mirrors what Git users expect from
/// `branch.<name>` patterns:
///
/// - `*` matches zero or more non-`/` characters.
/// - `?` matches exactly one non-`/` character.
/// - everything else is literal.
///
/// `*` deliberately does *not* cross `/` boundaries, so `release/*`
/// matches `release/v1.0` but not `release/v1/hotfix`. Patterns
/// without metacharacters keep their pre-PR9 exact-match meaning,
/// so existing policies continue to work unchanged.
pub fn branch_matches_pattern(branch: &str, pattern: &str) -> bool {
    glob_match(pattern.as_bytes(), branch.as_bytes())
}

fn glob_match(pat: &[u8], text: &[u8]) -> bool {
    // Iterative two-pointer matcher with a single backtrack point
    // for `*`. Plenty fast for branch-name length, no allocations.
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_p, mut star_t): (Option<usize>, usize) = (None, 0);
    while ti < text.len() {
        if pi < pat.len() {
            match pat[pi] {
                b'?' if text[ti] != b'/' => {
                    pi += 1;
                    ti += 1;
                    continue;
                }
                b'*' => {
                    star_p = Some(pi);
                    star_t = ti;
                    pi += 1;
                    continue;
                }
                c if c == text[ti] => {
                    pi += 1;
                    ti += 1;
                    continue;
                }
                _ => {}
            }
        }
        // Mismatch (or pattern exhausted): try to extend the most
        // recent `*` over one more text byte, but never cross `/`.
        if let Some(sp) = star_p {
            if text[star_t] == b'/' {
                return false;
            }
            pi = sp + 1;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }
    // Trailing `*`s in the pattern can absorb empty input.
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

/// PR 9: does any pattern in `patterns` match `branch`?
pub fn branch_matches_any(branch: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| branch_matches_pattern(branch, p))
}

/// PR 6 stage F: server-side push gate enforcement.
///
/// On a `RefWrite` from the SSH helper, this checks whether the
/// target branch is listed in `RepoPolicy.push_gated_branches`. If
/// so, it runs `gate_check` against the new tip and refuses the
/// write on failure with a clear, user-actionable message.
///
/// Non-head refs (tags, remote-tracking) and branches not listed
/// in the policy pass through unchanged.
pub fn enforce_push_gate(
    store: &dyn Store,
    morph_dir: &Path,
    ref_name: &str,
    tip: &Hash,
) -> Result<(), MorphError> {
    let Some(branch) = branch_from_ref(ref_name) else {
        return Ok(());
    };
    let policy = read_policy(morph_dir)?;
    if !branch_matches_any(branch, &policy.push_gated_branches) {
        return Ok(());
    }
    let result = gate_check(store, morph_dir, tip)?;
    if !result.passed {
        return Err(MorphError::Serialization(format!(
            "push gate failed for branch '{}': {}",
            branch,
            result.reasons.join(", ")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn missing_required_metrics_returns_absent_names_in_order() {
        let mut policy = RepoPolicy::default();
        policy.required_metrics = vec!["tests_total".into(), "tests_passed".into(), "pass_rate".into()];

        let mut observed = BTreeMap::new();
        observed.insert("tests_total".into(), 10.0);
        let missing = missing_required_metrics(&policy, &observed);
        assert_eq!(missing, vec!["tests_passed".to_string(), "pass_rate".to_string()]);

        observed.insert("tests_passed".into(), 10.0);
        observed.insert("pass_rate".into(), 1.0);
        assert!(missing_required_metrics(&policy, &observed).is_empty());
    }

    #[test]
    fn missing_required_metrics_empty_policy_is_always_satisfied() {
        let policy = RepoPolicy::default();
        let observed: BTreeMap<String, f64> = BTreeMap::new();
        assert!(missing_required_metrics(&policy, &observed).is_empty());
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
            push_gated_branches: vec![],
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
    fn push_gated_branches_round_trip() {
        // PR 6 stage F cycle 26 RED→GREEN: protected/gated branches
        // are part of `RepoPolicy` and survive a write/read cycle.
        // The server's RefWrite handler consults this list to
        // decide whether to enforce gate_check.
        let (dir, _store) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        let policy = RepoPolicy {
            push_gated_branches: vec!["main".into(), "release/*".into()],
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();
        let read_back = read_policy(&morph_dir).unwrap();
        assert_eq!(read_back.push_gated_branches, vec!["main", "release/*"]);
    }

    #[test]
    fn enforce_push_gate_passes_for_unconfigured_branches() {
        // PR 6 stage F cycle 27 RED→GREEN: with an empty
        // `push_gated_branches`, every ref-write goes through
        // unchanged (matches pre-PR6 behavior, no surprises for
        // existing servers).
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        let metrics = BTreeMap::new();
        let h = make_commit(store.as_ref(), &dir, metrics);
        enforce_push_gate(store.as_ref(), &morph_dir, "heads/main", &h)
            .expect("default policy should not gate anything");
    }

    #[test]
    fn enforce_push_gate_passes_for_non_head_refs() {
        // tags/, remotes/, etc. are unaffected. Only branch tips
        // under heads/ go through the gate.
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");

        let policy = RepoPolicy {
            required_metrics: vec!["acc".into()],
            push_gated_branches: vec!["main".into()],
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let h = make_commit(store.as_ref(), &dir, BTreeMap::new());
        enforce_push_gate(store.as_ref(), &morph_dir, "tags/v1", &h)
            .expect("tag refs are not gated");
    }

    #[test]
    fn enforce_push_gate_passes_when_branch_not_listed() {
        // Branch isn't in `push_gated_branches` → no gate, even if
        // the commit would fail gate_check.
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        let policy = RepoPolicy {
            required_metrics: vec!["acc".into()],
            push_gated_branches: vec!["main".into()],
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let h = make_commit(store.as_ref(), &dir, BTreeMap::new());
        enforce_push_gate(store.as_ref(), &morph_dir, "heads/feature", &h)
            .expect("non-listed branch is not gated");
    }

    #[test]
    fn enforce_push_gate_rejects_failing_commit_on_gated_branch() {
        // The headline case. `main` is gated, the commit lacks
        // metric `acc`, gate_check fails, the server refuses the
        // ref-write with a clear "push gate failed" message.
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        let policy = RepoPolicy {
            required_metrics: vec!["acc".into()],
            push_gated_branches: vec!["main".into()],
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let h = make_commit(store.as_ref(), &dir, BTreeMap::new());
        let err = enforce_push_gate(store.as_ref(), &morph_dir, "heads/main", &h)
            .expect_err("gate must reject");
        let msg = format!("{}", err);
        assert!(
            msg.contains("push gate"),
            "expected push gate error, got: {}",
            msg
        );
        assert!(msg.contains("main"), "should mention branch, got: {}", msg);
    }

    #[test]
    fn enforce_push_gate_passes_when_metrics_meet_thresholds() {
        // Same setup but the commit ships the required metrics
        // *and* has a passing certification annotation. Both are
        // required: gate_check verifies thresholds and that the
        // commit was certified. This is the "happy path" that
        // a healthy CI pipeline should always hit.
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        let policy = RepoPolicy {
            required_metrics: vec!["acc".into()],
            thresholds: {
                let mut m = BTreeMap::new();
                m.insert("acc".into(), 0.5);
                m
            },
            push_gated_branches: vec!["main".into()],
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let mut metrics = BTreeMap::new();
        metrics.insert("acc".into(), 0.9);
        let h = make_commit(store.as_ref(), &dir, metrics.clone());
        // Certify the commit so gate_check sees a passing
        // certification annotation.
        let cert = certify_commit(
            store.as_ref(),
            &morph_dir,
            &h,
            &metrics,
            Some("ci-v1"),
            None,
        )
        .unwrap();
        assert!(cert.passed);
        enforce_push_gate(store.as_ref(), &morph_dir, "heads/main", &h)
            .expect("certified commit with passing metric should be accepted");
    }

    // ── PR 9: glob patterns in push_gated_branches ───────────────

    #[test]
    fn branch_matches_pattern_handles_literal_strings() {
        assert!(branch_matches_pattern("main", "main"));
        assert!(!branch_matches_pattern("mainline", "main"));
        assert!(!branch_matches_pattern("dev", "main"));
        // The empty pattern only matches the empty branch — used by
        // nobody, but a useful sanity test.
        assert!(branch_matches_pattern("", ""));
        assert!(!branch_matches_pattern("anything", ""));
    }

    #[test]
    fn branch_matches_pattern_star_does_not_cross_slash() {
        // Headline use case: `release/*` covers `release/v1.0` but
        // refuses to gobble nested namespaces.
        assert!(branch_matches_pattern("release/v1.0", "release/*"));
        assert!(branch_matches_pattern("release/v2.5.1", "release/*"));
        assert!(!branch_matches_pattern("release", "release/*"));
        assert!(!branch_matches_pattern(
            "release/v1/hotfix",
            "release/*"
        ));
        // Top-level star matches any branch with no slashes.
        assert!(branch_matches_pattern("main", "*"));
        assert!(branch_matches_pattern("dev", "*"));
        assert!(!branch_matches_pattern("release/v1", "*"));
    }

    #[test]
    fn branch_matches_pattern_question_mark_matches_one_non_slash() {
        assert!(branch_matches_pattern("v1", "v?"));
        assert!(branch_matches_pattern("v9", "v?"));
        assert!(!branch_matches_pattern("v10", "v?"));
        assert!(!branch_matches_pattern("v/", "v?"));
    }

    #[test]
    fn branch_matches_pattern_combinations() {
        assert!(branch_matches_pattern(
            "release/v1.0-rc1",
            "release/v?.*"
        ));
        assert!(!branch_matches_pattern(
            "release/v.0-rc1",
            "release/v?.*"
        ));
        // Multiple stars in one segment.
        assert!(branch_matches_pattern("hotfix-prod-2026", "hotfix-*-*"));
        assert!(!branch_matches_pattern(
            "hotfix-prod/2026",
            "hotfix-*-*"
        ));
    }

    #[test]
    fn branch_matches_any_short_circuits() {
        let patterns = vec!["main".into(), "release/*".into()];
        assert!(branch_matches_any("main", &patterns));
        assert!(branch_matches_any("release/v1", &patterns));
        assert!(!branch_matches_any("feature/x", &patterns));
        assert!(!branch_matches_any("release/v1/hotfix", &patterns));
        // Empty pattern list never matches.
        assert!(!branch_matches_any("anything", &[]));
    }

    #[test]
    fn enforce_push_gate_matches_glob_patterns_in_push_gated_branches() {
        // PR 9 cycle 6: the headline behavior — a `release/*` entry
        // must gate `release/v1.0` even though the literal string
        // `release/v1.0` was never enumerated in the policy.
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        let policy = RepoPolicy {
            required_metrics: vec!["acc".into()],
            push_gated_branches: vec!["release/*".into()],
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let mut metrics = BTreeMap::new();
        metrics.insert("other".into(), 0.9);
        let h = make_commit(store.as_ref(), &dir, metrics);

        let err = enforce_push_gate(
            store.as_ref(),
            &morph_dir,
            "heads/release/v1.0",
            &h,
        )
        .expect_err("release/v1.0 must be gated by `release/*`");
        let msg = format!("{}", err);
        assert!(msg.contains("push gate"), "wrong message: {}", msg);
        assert!(msg.contains("release/v1.0"), "wrong branch name: {}", msg);

        // A branch outside the glob must not be gated.
        enforce_push_gate(
            store.as_ref(),
            &morph_dir,
            "heads/feature/login",
            &h,
        )
        .expect("feature/login is outside `release/*` and must pass through");
    }

    #[test]
    fn enforce_push_gate_glob_does_not_cross_slash_in_release_pattern() {
        // PR 9 cycle 7: `release/*` is a **single-component** glob,
        // so a multi-segment branch like `release/v1/hotfix` is
        // *not* gated by it. Admins who want that need either an
        // explicit pattern (`release/*/*`) or a more permissive one
        // — same shape as Git's refspec semantics.
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        let policy = RepoPolicy {
            required_metrics: vec!["acc".into()],
            push_gated_branches: vec!["release/*".into()],
            ..Default::default()
        };
        write_policy(&morph_dir, &policy).unwrap();

        let mut metrics = BTreeMap::new();
        metrics.insert("other".into(), 0.9);
        let h = make_commit(store.as_ref(), &dir, metrics);

        // Without the glob's slash boundary this would mistakenly
        // gate the nested branch and fail the gate. Asserting that
        // it passes proves the matcher honors the boundary.
        enforce_push_gate(
            store.as_ref(),
            &morph_dir,
            "heads/release/v1/hotfix",
            &h,
        )
        .expect("release/v1/hotfix should not be gated by `release/*`");
    }

    #[test]
    fn push_gated_branches_default_empty_for_legacy_configs() {
        // Older policies don't include the field; deserializing
        // them must default to an empty list rather than failing.
        let json = r#"{"required_metrics":["acc"],"merge_policy":"dominance"}"#;
        let p: RepoPolicy = serde_json::from_str(json).unwrap();
        assert_eq!(p.required_metrics, vec!["acc"]);
        assert!(p.push_gated_branches.is_empty());
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
        // Phase 2a: use a bare temp dir (no `init_repo`) so we
        // exercise the legacy code path where `.morph/config.json`
        // has no `policy` key. Old/upgraded repos must still read
        // back the empty defaults rather than failing.
        let dir = tempfile::tempdir().unwrap();
        let morph_dir = dir.path().join(".morph");
        std::fs::create_dir_all(&morph_dir).unwrap();
        // No config.json at all — read_policy should return the
        // empty default rather than blowing up.
        let policy = read_policy(&morph_dir).unwrap();
        assert!(policy.required_metrics.is_empty());
        assert!(policy.thresholds.is_empty());
        assert_eq!(policy.merge_policy, "dominance");

        // Now write a config.json without a "policy" key (the shape
        // older Morph repos shipped) and confirm we still get
        // empty defaults rather than a parse error.
        std::fs::write(
            morph_dir.join("config.json"),
            serde_json::json!({"repo_version":"0.0"}).to_string(),
        )
        .unwrap();
        let policy2 = read_policy(&morph_dir).unwrap();
        assert!(policy2.required_metrics.is_empty());
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

    // ── effective_metrics (PR 1: unified certification model) ────────

    #[test]
    fn effective_metrics_returns_inline_when_no_certification() {
        let (dir, store) = setup_repo();
        let mut inline = BTreeMap::new();
        inline.insert("acc".into(), 0.9);
        inline.insert("f1".into(), 0.85);
        let h = make_commit(store.as_ref(), &dir, inline.clone());

        let got = effective_metrics(store.as_ref(), &h).unwrap();
        assert_eq!(got, inline);
    }

    #[test]
    fn effective_metrics_returns_empty_for_uncertified_empty_commit() {
        let (dir, store) = setup_repo();
        let h = make_commit(store.as_ref(), &dir, BTreeMap::new());
        let got = effective_metrics(store.as_ref(), &h).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn effective_metrics_layers_certification_over_empty_inline() {
        // Headline standalone-mode case: a commit recorded without
        // metrics (empty inline), then later certified, surfaces the
        // certification metrics. This is the "evidence over time"
        // contract — every metric reader (status, eval gaps, merge
        // gate, show JSON) uses this helper so the late evidence is
        // visible everywhere consistently.
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        let h = make_commit(store.as_ref(), &dir, BTreeMap::new());

        let mut metrics = BTreeMap::new();
        metrics.insert("acc".into(), 0.95);
        metrics.insert("tests_passed".into(), 42.0);
        certify_commit(store.as_ref(), &morph_dir, &h, &metrics, None, None).unwrap();

        let got = effective_metrics(store.as_ref(), &h).unwrap();
        assert_eq!(got, metrics);
    }

    #[test]
    fn effective_metrics_certification_overrides_inline_per_key() {
        // If a key appears in both inline and certification, the
        // certification value wins because it's the more recent
        // claim about the same commit (e.g. a flaky-test re-run).
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        let mut inline = BTreeMap::new();
        inline.insert("acc".into(), 0.80);
        inline.insert("latency".into(), 200.0);
        let h = make_commit(store.as_ref(), &dir, inline);

        let mut certified = BTreeMap::new();
        certified.insert("acc".into(), 0.95);
        certify_commit(store.as_ref(), &morph_dir, &h, &certified, None, None).unwrap();

        let got = effective_metrics(store.as_ref(), &h).unwrap();
        assert_eq!(got.get("acc"), Some(&0.95), "certification overrides inline");
        assert_eq!(
            got.get("latency"),
            Some(&200.0),
            "inline keys absent from cert survive"
        );
    }

    #[test]
    fn effective_metrics_uses_latest_certification_when_multiple_exist() {
        // Multiple certifications can stack on a commit (re-run after
        // a flake fix, second CI runner). The most recently appended
        // certification wins — same semantics as `gate_check` had
        // before the helper was extracted.
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        let h = make_commit(store.as_ref(), &dir, BTreeMap::new());

        let mut first = BTreeMap::new();
        first.insert("acc".into(), 0.80);
        certify_commit(store.as_ref(), &morph_dir, &h, &first, Some("ci-1"), None).unwrap();

        let mut second = BTreeMap::new();
        second.insert("acc".into(), 0.95);
        certify_commit(store.as_ref(), &morph_dir, &h, &second, Some("ci-2"), None).unwrap();

        let got = effective_metrics(store.as_ref(), &h).unwrap();
        assert_eq!(got.get("acc"), Some(&0.95), "latest certification wins");
    }

    #[test]
    fn effective_metrics_for_commit_matches_effective_metrics() {
        let (dir, store) = setup_repo();
        let morph_dir = dir.path().join(".morph");
        let mut inline = BTreeMap::new();
        inline.insert("f1".into(), 0.7);
        let h = make_commit(store.as_ref(), &dir, inline);

        let mut metrics = BTreeMap::new();
        metrics.insert("acc".into(), 0.95);
        certify_commit(store.as_ref(), &morph_dir, &h, &metrics, None, None).unwrap();

        let commit = match store.get(&h).unwrap() {
            MorphObject::Commit(c) => c,
            _ => panic!("expected Commit"),
        };
        let from_hash = effective_metrics(store.as_ref(), &h).unwrap();
        let from_loaded = effective_metrics_for_commit(store.as_ref(), &h, &commit).unwrap();
        assert_eq!(from_hash, from_loaded);
    }

    #[test]
    fn effective_metrics_errors_on_non_commit_target() {
        let (dir, store) = setup_repo();
        let _ = dir;
        let blob_hash = store
            .put(&MorphObject::Blob(crate::objects::Blob {
                kind: "text".into(),
                content: serde_json::json!("hello"),
            }))
            .unwrap();
        let err = effective_metrics(store.as_ref(), &blob_hash).expect_err(
            "effective_metrics on a non-commit must surface a clear error",
        );
        assert!(format!("{}", err).contains("not a commit"));
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
