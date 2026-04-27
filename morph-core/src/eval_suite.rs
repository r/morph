//! Phase 4a: turn YAML specs and Cucumber `.feature` files into
//! [`EvalCase`] objects so the agent can register acceptance cases
//! without writing JSON by hand.
//!
//! The ingestors deliberately stay shallow: each top-level YAML spec
//! becomes one case, each Cucumber `Scenario` becomes one case, and
//! we preserve the original text in `expected.raw` so a human
//! reviewer can always reconstruct the source from the suite.
//!
//! Key entry points:
//!   - [`add_cases_from_yaml`]    — load N YAML spec files.
//!   - [`add_cases_from_cucumber`] — load N `.feature` files.
//!   - [`add_cases_from_paths`]   — auto-detect by extension; recurses
//!     into directories so callers can feed `morph-cli/tests/specs/`.
//!   - [`build_or_extend_suite`]  — append-and-dedupe cases against
//!     an existing suite (or build a fresh one).

use crate::hash::Hash;
use crate::objects::{EvalCase, EvalSuite, MorphObject};
use crate::store::{MorphError, Store};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// Phase 5b: structured "what's still missing?" gaps. Used by the
/// `morph eval gaps` CLI command, the `morph_eval_gaps` MCP tool,
/// and the optional Cursor stop-hook script. Each gap carries a
/// canonical `kind` plus a human-readable `hint` so the agent can
/// follow up directly without rerouting through documentation.
pub fn compute_eval_gaps(
    morph_dir: &Path,
    store: &dyn Store,
    changed_files: u64,
) -> Result<Vec<Value>, MorphError> {
    let mut gaps = Vec::new();
    let head = crate::commit::resolve_head(store)?;
    if let Some(h) = &head {
        if let MorphObject::Commit(c) = store.get(h)? {
            if c.eval_contract.observed_metrics.is_empty() {
                gaps.push(json!({
                    "kind": "empty_head_metrics",
                    "hint": "Run `morph eval run -- <test command>` then `morph commit --from-run <hash>`.",
                }));
            }
        }
    }
    let policy = crate::policy::read_policy(morph_dir)?;
    let suite_empty = match policy.default_eval_suite.as_deref() {
        None => true,
        Some(suite_hex) => match Hash::from_hex(suite_hex).ok().and_then(|h| store.get(&h).ok()) {
            Some(MorphObject::EvalSuite(s)) => s.cases.is_empty(),
            _ => true,
        },
    };
    if suite_empty {
        gaps.push(json!({
            "kind": "empty_default_suite",
            "hint": "Add YAML/cucumber acceptance cases via `morph eval add-case <file>`.",
        }));
    }
    let runs = store.list(crate::store::ObjectType::Run)?;
    let recent: Vec<_> = runs.iter().rev().take(5).collect();
    let any_with_metrics = recent.iter().any(|h| {
        matches!(
            store.get(h),
            Ok(MorphObject::Run(r)) if !r.metrics.is_empty()
        )
    });
    if !any_with_metrics && changed_files > 0 {
        gaps.push(json!({
            "kind": "no_recent_run",
            "hint": "Working tree has changes but no metric-bearing run was recorded. Run `morph eval run`.",
        }));
    }
    Ok(gaps)
}

/// YAML specs in `morph-cli/tests/specs/` are typically a sequence
/// of top-level documents, but we also accept a single mapping. Each
/// resulting case keeps the raw spec under `expected.raw` so the
/// commit reviewer can reconstruct the original assertion.
pub fn add_cases_from_yaml(specs: &[PathBuf]) -> Result<Vec<EvalCase>, MorphError> {
    let mut out = Vec::new();
    for path in specs {
        let text = std::fs::read_to_string(path).map_err(|e| {
            MorphError::Io(std::io::Error::new(
                e.kind(),
                format!("read {}: {}", path.display(), e),
            ))
        })?;
        let docs: Vec<Value> = parse_yaml_docs(&text, path)?;
        for (i, doc) in docs.iter().enumerate() {
            if let Some(case) = case_from_yaml_doc(path, i, doc) {
                out.push(case);
            }
        }
    }
    Ok(out)
}

/// Cucumber ingestion uses a small hand-rolled parser so we don't
/// pull in `gherkin` for what amounts to a "split the file by
/// `Scenario:` headers" job. Comments and `Background:` blocks are
/// preserved on each scenario so the case stays self-contained.
pub fn add_cases_from_cucumber(features: &[PathBuf]) -> Result<Vec<EvalCase>, MorphError> {
    let mut out = Vec::new();
    for path in features {
        let text = std::fs::read_to_string(path).map_err(|e| {
            MorphError::Io(std::io::Error::new(
                e.kind(),
                format!("read {}: {}", path.display(), e),
            ))
        })?;
        let parsed = parse_feature(&text);
        for sc in &parsed.scenarios {
            out.push(EvalCase {
                id: case_id(path, &sc.name),
                input: json!({
                    "kind": "cucumber",
                    "feature_path": path.display().to_string(),
                    "feature_name": parsed.feature_name,
                    "scenario": sc.name,
                    "tags": sc.tags,
                }),
                expected: json!({
                    "raw": sc.body,
                    "background": parsed.background,
                }),
                metric: "pass".to_string(),
                fixture_source: "candidate".to_string(),
            });
        }
    }
    Ok(out)
}

/// Walks the supplied paths and dispatches to [`add_cases_from_yaml`]
/// or [`add_cases_from_cucumber`] based on file extension. Directories
/// are walked one level deep — that's enough for the conventional
/// `tests/specs/` and `features/` layouts.
pub fn add_cases_from_paths(paths: &[PathBuf]) -> Result<Vec<EvalCase>, MorphError> {
    let mut yaml = Vec::new();
    let mut feature = Vec::new();
    for p in paths {
        collect_inputs(p, &mut yaml, &mut feature)?;
    }
    let mut cases = add_cases_from_yaml(&yaml)?;
    cases.extend(add_cases_from_cucumber(&feature)?);
    Ok(cases)
}

/// Return the sorted list of case ids present in `new_suite` but not
/// in `old_suite`. Used by `morph commit` to auto-detect newly
/// introduced acceptance cases when the user hasn't passed
/// `--new-cases` explicitly.
///
/// `None` for either argument is treated as "no suite" (empty case
/// set):
///
/// * `old_suite = None, new_suite = Some(s)` — every case in `s` is
///   "new" (typical for a root commit).
/// * `old_suite = Some(s), new_suite = None` — returns `vec![]`
///   (cases were retired, but that's not a *new-cases* event).
/// * Both `None` — returns `vec![]`.
///
/// Hashes that don't resolve to an `EvalSuite` object are also
/// treated as empty case sets so that a corrupt or never-stored
/// suite reference doesn't crash the commit path.
pub fn diff_suite_case_ids(
    store: &dyn Store,
    new_suite: Option<&Hash>,
    old_suite: Option<&Hash>,
) -> Result<Vec<String>, MorphError> {
    let new_ids = load_case_ids(store, new_suite)?;
    let old_ids = load_case_ids(store, old_suite)?;
    let old_set: std::collections::BTreeSet<&String> = old_ids.iter().collect();
    let mut diff: Vec<String> = new_ids
        .into_iter()
        .filter(|id| !old_set.contains(id))
        .collect();
    diff.sort();
    diff.dedup();
    Ok(diff)
}

fn load_case_ids(
    store: &dyn Store,
    suite: Option<&Hash>,
) -> Result<Vec<String>, MorphError> {
    let Some(h) = suite else { return Ok(vec![]); };
    match store.get(h) {
        Ok(MorphObject::EvalSuite(s)) => Ok(s.cases.into_iter().map(|c| c.id).collect()),
        // Non-suite or missing → treat as empty so callers don't
        // need to distinguish "no suite" from "suite had no cases".
        _ => Ok(vec![]),
    }
}

/// Build a suite from `new_cases`, optionally extending the suite
/// stored at `prev`. Cases dedupe by `id`; the *new* version wins so
/// a re-ingest picks up edits.
pub fn build_or_extend_suite(
    store: &dyn Store,
    prev: Option<Hash>,
    new_cases: &[EvalCase],
) -> Result<Hash, MorphError> {
    let mut suite = match prev {
        Some(h) => match store.get(&h)? {
            MorphObject::EvalSuite(s) => s,
            _ => EvalSuite { cases: vec![], metrics: vec![] },
        },
        None => EvalSuite { cases: vec![], metrics: vec![] },
    };
    for c in new_cases {
        if let Some(idx) = suite.cases.iter().position(|existing| existing.id == c.id) {
            suite.cases[idx] = c.clone();
        } else {
            suite.cases.push(c.clone());
        }
    }
    store.put(&MorphObject::EvalSuite(suite))
}

// ── implementation details ───────────────────────────────────────────

fn collect_inputs(
    p: &Path,
    yaml: &mut Vec<PathBuf>,
    feature: &mut Vec<PathBuf>,
) -> Result<(), MorphError> {
    if p.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(p)
            .map_err(|e| {
                MorphError::Io(std::io::Error::new(
                    e.kind(),
                    format!("read_dir {}: {}", p.display(), e),
                ))
            })?
            .filter_map(|e: std::io::Result<std::fs::DirEntry>| {
                e.ok().map(|d| d.path())
            })
            .collect();
        entries.sort();
        for entry in entries {
            if entry.is_file() {
                push_by_ext(&entry, yaml, feature);
            }
        }
    } else if p.is_file() {
        push_by_ext(p, yaml, feature);
    }
    Ok(())
}

fn push_by_ext(p: &Path, yaml: &mut Vec<PathBuf>, feature: &mut Vec<PathBuf>) {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("yaml") | Some("yml") => yaml.push(p.to_path_buf()),
        Some("feature") => feature.push(p.to_path_buf()),
        _ => {}
    }
}

fn parse_yaml_docs(text: &str, path: &Path) -> Result<Vec<Value>, MorphError> {
    // Try multi-doc first, then sequence-of-mappings, then single doc.
    let mut acc = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(text) {
        let v = Value::deserialize(doc).map_err(|e| {
            MorphError::Serialization(format!("parse YAML {}: {}", path.display(), e))
        })?;
        match v {
            Value::Array(arr) => acc.extend(arr),
            Value::Null => {}
            other => acc.push(other),
        }
    }
    Ok(acc)
}

fn case_from_yaml_doc(path: &Path, idx: usize, doc: &Value) -> Option<EvalCase> {
    let map: &Map<String, Value> = doc.as_object()?;
    let name = map
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("doc_{}", idx));
    let id = case_id(path, &name);
    let input = json!({
        "kind": "yaml_spec",
        "spec_path": path.display().to_string(),
        "name": name,
    });
    Some(EvalCase {
        id,
        input,
        expected: Value::Object(map.clone()),
        metric: "pass".to_string(),
        fixture_source: "candidate".to_string(),
    })
}

fn case_id(path: &Path, name: &str) -> String {
    // Stable, human-readable id: filename stem + ":" + sanitized name.
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("spec");
    let safe: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    format!("{stem}:{safe}")
}

// ── Cucumber parser ──────────────────────────────────────────────────

#[derive(Debug, Default)]
struct ParsedFeature {
    feature_name: String,
    background: String,
    scenarios: Vec<ParsedScenario>,
}

#[derive(Debug, Default)]
struct ParsedScenario {
    name: String,
    tags: Vec<String>,
    body: String,
}

fn parse_feature(text: &str) -> ParsedFeature {
    // Track current section by header. We only care about
    // Feature/Background/Scenario/Scenario Outline plus tag lines
    // immediately preceding a scenario.
    let mut out = ParsedFeature::default();
    let mut pending_tags: Vec<String> = Vec::new();
    let mut current: Option<ParsedScenario> = None;
    let mut in_background = false;

    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let stripped = line.trim_start();

        if stripped.starts_with('#') || stripped.is_empty() {
            push_line(&mut current, &mut out, in_background, line);
            continue;
        }

        if stripped.starts_with('@') {
            // Tag line — collect until the next scenario header.
            for t in stripped.split_whitespace() {
                if let Some(stripped_tag) = t.strip_prefix('@') {
                    pending_tags.push(stripped_tag.to_string());
                }
            }
            continue;
        }

        if let Some(rest) = stripped.strip_prefix("Feature:") {
            out.feature_name = rest.trim().to_string();
            in_background = false;
            continue;
        }

        if stripped.starts_with("Background:") {
            // Flush any in-progress scenario before switching modes.
            if let Some(sc) = current.take() {
                out.scenarios.push(sc);
            }
            in_background = true;
            out.background.push_str(line);
            out.background.push('\n');
            continue;
        }

        if let Some(rest) = stripped
            .strip_prefix("Scenario Outline:")
            .or_else(|| stripped.strip_prefix("Scenario:"))
            .or_else(|| stripped.strip_prefix("Example:"))
        {
            if let Some(sc) = current.take() {
                out.scenarios.push(sc);
            }
            in_background = false;
            let mut sc = ParsedScenario {
                name: rest.trim().to_string(),
                tags: std::mem::take(&mut pending_tags),
                body: String::new(),
            };
            sc.body.push_str(line);
            sc.body.push('\n');
            current = Some(sc);
            continue;
        }

        push_line(&mut current, &mut out, in_background, line);
    }

    if let Some(sc) = current.take() {
        out.scenarios.push(sc);
    }
    out
}

fn push_line(
    current: &mut Option<ParsedScenario>,
    out: &mut ParsedFeature,
    in_background: bool,
    line: &str,
) {
    if let Some(sc) = current {
        sc.body.push_str(line);
        sc.body.push('\n');
    } else if in_background {
        out.background.push_str(line);
        out.background.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        std::fs::write(path, content).unwrap();
    }

    fn make_store() -> (tempfile::TempDir, Box<dyn Store>) {
        let dir = tempfile::tempdir().unwrap();
        let store: Box<dyn Store> = Box::new(crate::store::FsStore::new(dir.path()));
        (dir, store)
    }

    #[test]
    fn yaml_spec_with_named_doc_becomes_case() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("foo.yaml");
        write(&p, "- name: my_case\n  steps:\n    - morph: [status]\n");

        let cases = add_cases_from_yaml(&[p.clone()]).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].id, "foo:my_case");
        assert_eq!(cases[0].metric, "pass");
        // Raw spec is preserved under `expected` so reviewers can audit.
        let expected = cases[0].expected.as_object().unwrap();
        assert_eq!(expected.get("name").unwrap().as_str().unwrap(), "my_case");
    }

    #[test]
    fn yaml_spec_anonymous_doc_gets_synthetic_id() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("anon.yaml");
        write(&p, "- steps:\n    - morph: [status]\n");
        let cases = add_cases_from_yaml(&[p]).unwrap();
        assert_eq!(cases.len(), 1);
        assert!(cases[0].id.ends_with(":doc_0"));
    }

    #[test]
    fn cucumber_feature_yields_one_case_per_scenario() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("foo.feature");
        write(
            &p,
            "Feature: thing\n  Background:\n    Given a setup\n  @smoke\n  Scenario: first\n    When x\n    Then y\n  Scenario: second\n    When a\n    Then b\n",
        );
        let cases = add_cases_from_cucumber(&[p]).unwrap();
        assert_eq!(cases.len(), 2);
        let first = &cases[0];
        assert_eq!(first.id, "foo:first");
        assert_eq!(first.input["scenario"].as_str().unwrap(), "first");
        assert_eq!(
            first.input["tags"].as_array().unwrap()[0].as_str().unwrap(),
            "smoke",
        );
        assert!(first.expected["background"].as_str().unwrap().contains("Given a setup"));
        let second = &cases[1];
        assert_eq!(second.id, "foo:second");
        assert!(second.input["tags"].as_array().unwrap().is_empty());
    }

    #[test]
    fn add_cases_from_paths_walks_directory() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.yaml"), "- name: alpha\n");
        write(&dir.path().join("b.feature"), "Feature: x\n  Scenario: beta\n    Then ok\n");
        write(&dir.path().join("c.txt"), "ignored\n"); // unsupported ext

        let cases = add_cases_from_paths(&[dir.path().to_path_buf()]).unwrap();
        let ids: Vec<&str> = cases.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"a:alpha"), "ids={ids:?}");
        assert!(ids.contains(&"b:beta"), "ids={ids:?}");
        assert_eq!(cases.len(), 2, "txt must be ignored");
    }

    #[test]
    fn build_or_extend_suite_dedupes_by_id() {
        let (_dir, store) = make_store();
        let case_v1 = EvalCase {
            id: "x:case".into(),
            input: json!({"v": 1}),
            expected: json!({}),
            metric: "pass".into(),
            fixture_source: "candidate".into(),
        };
        let h1 = build_or_extend_suite(store.as_ref(), None, &[case_v1.clone()]).unwrap();
        let case_v2 = EvalCase { input: json!({"v": 2}), ..case_v1.clone() };
        let other = EvalCase { id: "x:other".into(), ..case_v1.clone() };
        let h2 = build_or_extend_suite(store.as_ref(), Some(h1), &[case_v2.clone(), other.clone()]).unwrap();

        let suite = match store.get(&h2).unwrap() {
            MorphObject::EvalSuite(s) => s,
            _ => panic!("expected EvalSuite"),
        };
        assert_eq!(suite.cases.len(), 2);
        let by_id: std::collections::HashMap<_, _> =
            suite.cases.iter().map(|c| (c.id.clone(), c.clone())).collect();
        assert_eq!(by_id["x:case"].input["v"], json!(2), "v1 should be replaced by v2");
        assert!(by_id.contains_key("x:other"));
    }

    #[test]
    fn diff_suite_case_ids_returns_only_new_ids() {
        let (_dir, store) = make_store();
        let case_a = EvalCase {
            id: "a".into(),
            input: json!({}),
            expected: json!({}),
            metric: "pass".into(),
            fixture_source: "candidate".into(),
        };
        let case_b = EvalCase { id: "b".into(), ..case_a.clone() };
        let case_c = EvalCase { id: "c".into(), ..case_a.clone() };
        let old = build_or_extend_suite(store.as_ref(), None, &[case_a.clone(), case_b.clone()]).unwrap();
        let new = build_or_extend_suite(store.as_ref(), Some(old), std::slice::from_ref(&case_c)).unwrap();
        let diff = diff_suite_case_ids(store.as_ref(), Some(&new), Some(&old)).unwrap();
        assert_eq!(diff, vec!["c".to_string()]);
    }

    #[test]
    fn diff_suite_case_ids_treats_no_old_suite_as_all_new() {
        let (_dir, store) = make_store();
        let case_a = EvalCase {
            id: "a".into(),
            input: json!({}),
            expected: json!({}),
            metric: "pass".into(),
            fixture_source: "candidate".into(),
        };
        let case_b = EvalCase { id: "b".into(), ..case_a.clone() };
        let new = build_or_extend_suite(store.as_ref(), None, &[case_a, case_b]).unwrap();
        let diff = diff_suite_case_ids(store.as_ref(), Some(&new), None).unwrap();
        assert_eq!(diff, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn diff_suite_case_ids_returns_empty_when_unchanged() {
        let (_dir, store) = make_store();
        let case_a = EvalCase {
            id: "a".into(),
            input: json!({}),
            expected: json!({}),
            metric: "pass".into(),
            fixture_source: "candidate".into(),
        };
        let h = build_or_extend_suite(store.as_ref(), None, &[case_a]).unwrap();
        let diff = diff_suite_case_ids(store.as_ref(), Some(&h), Some(&h)).unwrap();
        assert!(diff.is_empty());
    }

    #[test]
    fn diff_suite_case_ids_tolerates_non_suite_hash() {
        let (_dir, store) = make_store();
        let blob = MorphObject::Blob(crate::objects::Blob {
            kind: "x".into(),
            content: json!({}),
        });
        let blob_hash = store.put(&blob).unwrap();
        let diff = diff_suite_case_ids(store.as_ref(), Some(&blob_hash), None).unwrap();
        assert!(diff.is_empty(), "non-suite hash must be treated as empty");
    }

    #[test]
    fn build_or_extend_suite_starts_fresh_when_prev_missing() {
        let (_dir, store) = make_store();
        let cases = vec![EvalCase {
            id: "z:1".into(),
            input: json!({}),
            expected: json!({}),
            metric: "pass".into(),
            fixture_source: "candidate".into(),
        }];
        let h = build_or_extend_suite(store.as_ref(), None, &cases).unwrap();
        match store.get(&h).unwrap() {
            MorphObject::EvalSuite(s) => assert_eq!(s.cases.len(), 1),
            _ => panic!("expected EvalSuite"),
        }
    }

    // ── compute_eval_gaps coverage ────────────────────────────────

    fn fresh_repo() -> (tempfile::TempDir, Box<dyn Store>) {
        let dir = tempfile::tempdir().unwrap();
        let _ = crate::repo::init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        let store: Box<dyn Store> = crate::open_store(&morph_dir).unwrap();
        (dir, store)
    }

    fn morph_dir(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join(".morph")
    }

    fn gap_kinds(gaps: &[Value]) -> Vec<String> {
        gaps.iter()
            .filter_map(|v| v.get("kind").and_then(|k| k.as_str()).map(|s| s.to_string()))
            .collect()
    }

    #[test]
    fn compute_eval_gaps_fresh_repo_reports_missing_suite_only_when_clean() {
        let (dir, store) = fresh_repo();
        let gaps = compute_eval_gaps(&morph_dir(&dir), store.as_ref(), 0).unwrap();
        let kinds = gap_kinds(&gaps);
        assert!(kinds.contains(&"empty_default_suite".to_string()));
        assert!(!kinds.contains(&"no_recent_run".to_string()));
        assert!(!kinds.contains(&"empty_head_metrics".to_string()));
    }

    #[test]
    fn compute_eval_gaps_dirty_tree_without_runs_reports_no_recent_run() {
        let (dir, store) = fresh_repo();
        let gaps = compute_eval_gaps(&morph_dir(&dir), store.as_ref(), 7).unwrap();
        let kinds = gap_kinds(&gaps);
        assert!(kinds.contains(&"no_recent_run".to_string()));
        assert!(kinds.contains(&"empty_default_suite".to_string()));
    }

    #[test]
    fn compute_eval_gaps_recent_run_silences_no_recent_run() {
        let (dir, store) = fresh_repo();
        let mut metrics = std::collections::BTreeMap::new();
        metrics.insert("tests_passed".to_string(), 1.0);
        metrics.insert("tests_total".to_string(), 1.0);
        crate::record::record_eval_run(
            store.as_ref(),
            &metrics,
            "cargo",
            Some("cargo test"),
            Some("ok"),
            Some(0),
        )
        .unwrap();

        let gaps = compute_eval_gaps(&morph_dir(&dir), store.as_ref(), 5).unwrap();
        assert!(!gap_kinds(&gaps).contains(&"no_recent_run".to_string()));
    }

    #[test]
    fn compute_eval_gaps_empty_head_metrics_is_flagged() {
        let (dir, store) = fresh_repo();
        // Make a commit with empty metrics so HEAD has no behavioral evidence.
        std::fs::write(dir.path().join("f.txt"), "data").unwrap();
        crate::add_paths(store.as_ref(), dir.path(), &[std::path::PathBuf::from(".")]).unwrap();
        crate::create_tree_commit(
            store.as_ref(),
            dir.path(),
            None,
            None,
            std::collections::BTreeMap::new(),
            "empty".into(),
            None,
            Some("0.5"),
        )
        .unwrap();

        let gaps = compute_eval_gaps(&morph_dir(&dir), store.as_ref(), 0).unwrap();
        assert!(gap_kinds(&gaps).contains(&"empty_head_metrics".to_string()));
    }

    #[test]
    fn compute_eval_gaps_silent_when_everything_satisfied() {
        let (dir, store) = fresh_repo();

        // 1) Register a real eval suite via the public ingestion path.
        let spec = dir.path().join("spec.yaml");
        std::fs::write(&spec, "- name: alpha\n").unwrap();
        let cases = add_cases_from_paths(&[spec]).unwrap();
        let suite_hash = build_or_extend_suite(store.as_ref(), None, &cases).unwrap();
        let mut policy = crate::policy::read_policy(&morph_dir(&dir)).unwrap();
        policy.default_eval_suite = Some(suite_hash.to_string());
        crate::policy::write_policy(&morph_dir(&dir), &policy).unwrap();

        // 2) Make a commit that carries metrics so HEAD is non-empty.
        let mut metrics = std::collections::BTreeMap::new();
        metrics.insert("tests_passed".to_string(), 1.0);
        metrics.insert("tests_total".to_string(), 1.0);
        std::fs::write(dir.path().join("g.txt"), "data").unwrap();
        crate::add_paths(store.as_ref(), dir.path(), &[std::path::PathBuf::from(".")]).unwrap();
        crate::create_tree_commit(
            store.as_ref(),
            dir.path(),
            None,
            Some(&suite_hash),
            metrics.clone(),
            "with metrics".into(),
            None,
            Some("0.5"),
        )
        .unwrap();

        // 3) Working tree clean (changed_files = 0) so no_recent_run cannot trigger.
        let gaps = compute_eval_gaps(&morph_dir(&dir), store.as_ref(), 0).unwrap();
        assert!(gaps.is_empty(), "expected zero gaps, got {:?}", gaps);
    }
}
