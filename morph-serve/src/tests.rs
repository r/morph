//! Unit + API integration tests for the Morph hosted service.

use crate::{build_router, RepoEntry, ServiceConfig};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use morph_core::objects::*;
use morph_core::store::Store;
use morph_core::MorphObject;
use std::collections::BTreeMap;
use tower::ServiceExt;

fn setup_repo() -> (tempfile::TempDir, Box<dyn Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = morph_core::init_repo(dir.path()).unwrap();
    // Phase 2a: tests predate the opinionated default policy and
    // certify against bespoke metrics like `acc`. Reset to an empty
    // policy so each test sets the required metrics it actually uses.
    let permissive = morph_core::RepoPolicy::default();
    morph_core::policy::write_policy(&dir.path().join(".morph"), &permissive).unwrap();
    (dir, Box::new(store))
}

fn make_config(dir: &tempfile::TempDir) -> ServiceConfig {
    ServiceConfig {
        repos: vec![RepoEntry {
            name: "default".to_string(),
            morph_dir: dir.path().join(".morph"),
        }],
        addr: "127.0.0.1:0".parse().unwrap(),
        org_policy_path: None,
    }
}

fn store_blob(store: &dyn Store) -> morph_core::Hash {
    let blob = MorphObject::Blob(Blob {
        kind: "prompt".into(),
        content: serde_json::json!({"text": "hello"}),
    });
    store.put(&blob).unwrap()
}

fn make_commit_with_metrics(
    store: &dyn Store,
    dir: &tempfile::TempDir,
    metrics: BTreeMap<String, f64>,
    message: &str,
) -> morph_core::Hash {
    std::fs::write(dir.path().join("f.txt"), "data").unwrap();
    morph_core::add_paths(store, dir.path(), &[std::path::PathBuf::from(".")]).unwrap();
    morph_core::create_tree_commit(
        store,
        dir.path(),
        None,
        None,
        metrics,
        message.into(),
        None,
        Some("0.3"),
    )
    .unwrap()
}

async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    (status, json)
}

async fn post_json(
    app: axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    (status, json)
}

// ── Repo list & summary ─────────────────────────────────────────────

#[tokio::test]
async fn test_repo_list() {
    let (dir, _store) = setup_repo();
    let app = build_router(&make_config(&dir));
    let (status, json) = get_json(app, "/api/repos").await;
    assert_eq!(status, StatusCode::OK);
    let repos = json["repos"].as_array().unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0]["name"], "default");
}

#[tokio::test]
async fn test_repo_summary() {
    let (dir, store) = setup_repo();
    let mut m = BTreeMap::new();
    m.insert("acc".into(), 0.9);
    make_commit_with_metrics(store.as_ref(), &dir, m, "first commit");

    let app = build_router(&make_config(&dir));
    let (status, json) = get_json(app, "/api/repos/default/summary").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["name"], "default");
    assert_eq!(json["commit_count"], 1);
    assert_eq!(json["branch_count"], 1);
}

#[tokio::test]
async fn test_repo_summary_not_found() {
    let (dir, _store) = setup_repo();
    let app = build_router(&make_config(&dir));
    let (status, json) = get_json(app, "/api/repos/nonexistent/summary").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["code"], "repo_not_found");
}

// ── Branches ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_branch_list() {
    let (dir, store) = setup_repo();
    make_commit_with_metrics(store.as_ref(), &dir, BTreeMap::new(), "init");

    let app = build_router(&make_config(&dir));
    let (status, json) = get_json(app, "/api/repos/default/branches").await;
    assert_eq!(status, StatusCode::OK);
    let branches = json["branches"].as_array().unwrap();
    assert!(branches.iter().any(|b| b["name"] == "main"));
    assert_eq!(json["current"], "main");
}

// ── Commits ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_commit_list() {
    let (dir, store) = setup_repo();
    let mut m = BTreeMap::new();
    m.insert("acc".into(), 0.9);
    make_commit_with_metrics(store.as_ref(), &dir, m, "test commit");

    let app = build_router(&make_config(&dir));
    let (status, json) = get_json(app, "/api/repos/default/commits").await;
    assert_eq!(status, StatusCode::OK);
    let commits = json["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0]["message"], "test commit");
    assert_eq!(commits[0]["metric_count"], 1);
}

#[tokio::test]
async fn test_commit_detail() {
    let (dir, store) = setup_repo();
    let mut m = BTreeMap::new();
    m.insert("acc".into(), 0.9);
    let hash = make_commit_with_metrics(store.as_ref(), &dir, m, "detailed commit");

    let app = build_router(&make_config(&dir));
    let uri = format!("/api/repos/default/commits/{}", hash);
    let (status, json) = get_json(app, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["message"], "detailed commit");
    assert_eq!(json["eval_contract"]["observed_metrics"]["acc"], 0.9);
    assert!(json["behavioral_status"].is_object());
    assert_eq!(json["behavioral_status"]["is_merge"], false);
}

#[tokio::test]
async fn test_commit_detail_bad_hash() {
    let (dir, _store) = setup_repo();
    let app = build_router(&make_config(&dir));
    let (status, json) = get_json(app, "/api/repos/default/commits/notahash").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["code"], "bad_hash");
}

#[tokio::test]
async fn test_commit_detail_not_found() {
    let (dir, _store) = setup_repo();
    let fake = "a".repeat(64);
    let app = build_router(&make_config(&dir));
    let uri = format!("/api/repos/default/commits/{}", fake);
    let (status, json) = get_json(app, &uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["code"], "not_found");
}

// ── Runs ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_run_list_and_detail() {
    let (dir, store) = setup_repo();
    let run_hash =
        morph_core::record_session(store.as_ref(), "hi", "hello", Some("m"), Some("a")).unwrap();

    let app = build_router(&make_config(&dir));
    let (status, json) = get_json(app.clone(), "/api/repos/default/runs").await;
    assert_eq!(status, StatusCode::OK);
    let runs = json["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);

    let uri = format!("/api/repos/default/runs/{}", run_hash);
    let (status, json) = get_json(app, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["agent"]["id"], "a");
    assert_eq!(json["environment"]["model"], "m");
}

// ── Traces ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_trace_detail() {
    let (dir, store) = setup_repo();
    let run_hash =
        morph_core::record_session(store.as_ref(), "prompt", "response", None, None).unwrap();
    let run = match store.get(&run_hash).unwrap() {
        MorphObject::Run(r) => r,
        _ => panic!("expected run"),
    };

    let app = build_router(&make_config(&dir));
    let uri = format!("/api/repos/default/traces/{}", run.trace);
    let (status, json) = get_json(app, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["event_count"], 2);
    let events = json["events"].as_array().unwrap();
    assert_eq!(events[0]["kind"], "user");
    assert_eq!(events[1]["kind"], "assistant");
}

// ── Pipelines ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_pipeline_detail() {
    let (dir, store) = setup_repo();
    let run_hash =
        morph_core::record_session(store.as_ref(), "test", "answer", Some("gpt"), Some("ag"))
            .unwrap();
    let pipeline_hash =
        morph_core::extract_pipeline_from_run(store.as_ref(), &run_hash).unwrap();

    let app = build_router(&make_config(&dir));
    let uri = format!("/api/repos/default/pipelines/{}", pipeline_hash);
    let (status, json) = get_json(app, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["node_count"], 2);
    assert_eq!(json["edge_count"], 1);
    assert!(json["provenance"].is_object());
    assert_eq!(json["provenance"]["method"], "extracted");
}

// ── Objects (raw) ───────────────────────────────────────────────────

#[tokio::test]
async fn test_raw_object() {
    let (dir, store) = setup_repo();
    let hash = store_blob(store.as_ref());

    let app = build_router(&make_config(&dir));
    let uri = format!("/api/repos/default/objects/{}", hash);
    let (status, json) = get_json(app, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["type"], "blob");
}

// ── Annotations ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_annotations() {
    let (dir, store) = setup_repo();
    let hash = store_blob(store.as_ref());
    let mut data = BTreeMap::new();
    data.insert("rating".into(), serde_json::json!("good"));
    let ann = morph_core::create_annotation(&hash, None, "feedback".into(), data, None);
    store.put(&ann).unwrap();

    let app = build_router(&make_config(&dir));
    let uri = format!("/api/repos/default/annotations/{}", hash);
    let (status, json) = get_json(app, &uri).await;
    assert_eq!(status, StatusCode::OK);
    let anns = json["annotations"].as_array().unwrap();
    assert_eq!(anns.len(), 1);
    assert_eq!(anns[0]["kind"], "feedback");
}

// ── Policy ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_policy_default() {
    let (dir, _store) = setup_repo();
    let app = build_router(&make_config(&dir));
    let (status, json) = get_json(app, "/api/repos/default/policy").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["repo_policy"].is_object());
    assert_eq!(json["repo_policy"]["merge_policy"], "dominance");
}

#[tokio::test]
async fn test_policy_with_org() {
    let (dir, _store) = setup_repo();
    let org_dir = tempfile::tempdir().unwrap();
    let org_path = org_dir.path().join("org-policy.json");
    let org = crate::org_policy::OrgPolicy {
        required_metrics: vec!["org_metric".into()],
        thresholds: {
            let mut m = BTreeMap::new();
            m.insert("org_metric".into(), 0.5);
            m
        },
        ..Default::default()
    };
    crate::org_policy::save_org_policy(&org_path, &org).unwrap();

    let config = ServiceConfig {
        repos: vec![RepoEntry {
            name: "default".into(),
            morph_dir: dir.path().join(".morph"),
        }],
        addr: "127.0.0.1:0".parse().unwrap(),
        org_policy_path: Some(org_path),
    };
    let app = build_router(&config);
    let (status, json) = get_json(app, "/api/repos/default/policy").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["org_policy"].is_object());
    let effective = json["effective_required_metrics"].as_array().unwrap();
    assert!(effective.iter().any(|v| v == "org_metric"));
}

// ── Gate ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_gate_status() {
    let (dir, store) = setup_repo();
    let mut m = BTreeMap::new();
    m.insert("acc".into(), 0.9);
    let hash = make_commit_with_metrics(store.as_ref(), &dir, m, "gate test");

    let app = build_router(&make_config(&dir));
    let uri = format!("/api/repos/default/gate/{}", hash);
    let (status, json) = get_json(app, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["passed"].is_boolean());
    assert_eq!(json["commit"], hash.to_string());
}

// ── Behavioral status (certification) ───────────────────────────────

#[tokio::test]
async fn test_behavioral_status_certified() {
    let (dir, store) = setup_repo();
    let morph_dir = dir.path().join(".morph");
    let mut m = BTreeMap::new();
    m.insert("acc".into(), 0.9);
    let hash = make_commit_with_metrics(store.as_ref(), &dir, m.clone(), "cert test");
    morph_core::policy::certify_commit(
        store.as_ref(),
        &morph_dir,
        &hash,
        &m,
        Some("ci"),
        None,
    )
    .unwrap();

    let app = build_router(&make_config(&dir));
    let uri = format!("/api/repos/default/commits/{}", hash);
    let (status, json) = get_json(app, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["behavioral_status"]["certified"], true);
    let cert = &json["behavioral_status"]["certification"];
    assert_eq!(cert["passed"], true);
    assert_eq!(cert["runner"], "ci");
}

// ── Backward-compatible endpoints ───────────────────────────────────

#[tokio::test]
async fn test_compat_log() {
    let (dir, store) = setup_repo();
    make_commit_with_metrics(store.as_ref(), &dir, BTreeMap::new(), "compat");

    let app = build_router(&make_config(&dir));
    let (status, json) = get_json(app, "/api/log").await;
    assert_eq!(status, StatusCode::OK);
    let commits = json["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 1);
}

#[tokio::test]
async fn test_compat_runs() {
    let (dir, store) = setup_repo();
    morph_core::record_session(store.as_ref(), "p", "r", None, None).unwrap();

    let app = build_router(&make_config(&dir));
    let (status, json) = get_json(app, "/api/runs").await;
    assert_eq!(status, StatusCode::OK);
    let runs = json["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
}

#[tokio::test]
async fn test_compat_object() {
    let (dir, store) = setup_repo();
    let hash = store_blob(store.as_ref());

    let app = build_router(&make_config(&dir));
    let uri = format!("/api/object/{}", hash);
    let (status, json) = get_json(app, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["type"], "blob");
}

#[tokio::test]
async fn test_compat_graph() {
    let (dir, store) = setup_repo();
    let head = make_commit_with_metrics(store.as_ref(), &dir, BTreeMap::new(), "graph");

    let app = build_router(&make_config(&dir));
    let (status, json) = get_json(app, "/api/graph").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["nodes"].is_array());
    assert!(json["edges"].is_array());
    assert_eq!(
        json["head"].as_str(),
        Some(head.to_string().as_str()),
        "/api/graph must report HEAD so the client can focus the viewport on \
         the most recent commit instead of zooming out to fit everything"
    );
}

#[tokio::test]
async fn test_compat_graph_with_runs() {
    let (dir, store) = setup_repo();
    morph_core::record_session(store.as_ref(), "prompt", "response", Some("gpt"), Some("agent"))
        .unwrap();

    let app = build_router(&make_config(&dir));
    let (status, json) = get_json(app, "/api/graph").await;
    assert_eq!(status, StatusCode::OK);
    let nodes = json["nodes"].as_array().unwrap();
    let edges = json["edges"].as_array().unwrap();
    assert!(nodes.len() >= 3, "should have run, trace, and pipeline nodes");
    assert!(edges.len() >= 2, "should have run->trace and run->pipeline edges");

    let types: Vec<&str> = nodes.iter().map(|n| n["type"].as_str().unwrap()).collect();
    assert!(types.contains(&"run"), "should contain a run node");
    assert!(types.contains(&"trace"), "should contain a trace node");
    assert!(types.contains(&"pipeline"), "should contain a pipeline node");

    let node_ids: std::collections::HashSet<&str> =
        nodes.iter().map(|n| n["id"].as_str().unwrap()).collect();
    for e in edges {
        assert!(
            node_ids.contains(e["from"].as_str().unwrap()),
            "edge 'from' should reference an existing node"
        );
        assert!(
            node_ids.contains(e["to"].as_str().unwrap()),
            "edge 'to' should reference an existing node"
        );
    }
}

#[tokio::test]
async fn test_compat_graph_empty() {
    let (dir, _store) = setup_repo();
    let app = build_router(&make_config(&dir));
    let (status, json) = get_json(app, "/api/graph").await;
    assert_eq!(status, StatusCode::OK);
    let nodes = json["nodes"].as_array().unwrap();
    assert!(nodes.is_empty(), "empty repo should return no nodes");
    assert!(
        json.get("head").is_none() || json["head"].is_null(),
        "empty repo must not report a HEAD"
    );
}

/// The `/graph` page must use a viewport-fixed body height (not just min-height)
/// and disable vis-network's `improvedLayout` so large graphs render correctly.
/// Without these, the vis canvas expands the page past the viewport and the
/// rendered nodes end up off-screen, producing a blank graph.
#[tokio::test]
async fn test_graph_page_has_layout_fixes() {
    let (dir, _store) = setup_repo();
    let app = build_router(&make_config(&dir));
    let resp = app
        .oneshot(Request::builder().uri("/graph").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let html = std::str::from_utf8(&bytes).unwrap();

    assert!(
        html.contains("height: 100vh") && html.contains("overflow: hidden"),
        "graph body must pin height to viewport and clip overflow, or the vis \
         canvas grows past the viewport and nodes render off-screen"
    );
    assert!(
        html.contains("improvedLayout: false"),
        "graph must disable vis-network's improvedLayout; it silently fails on \
         networks with disconnected components and leaves the canvas blank"
    );
    assert!(
        html.contains("network.focus(headId"),
        "graph must focus on the HEAD commit after stabilization; fitting the \
         whole graph makes every node too small to read on large histories"
    );
    assert!(
        html.contains("id=\"fitAll\"") && html.contains("id=\"focusHead\""),
        "graph must expose Fit all / Focus HEAD buttons so users can switch \
         between the zoomed-in and zoomed-out views"
    );
}

// ── Org policy endpoints ────────────────────────────────────────────

#[tokio::test]
async fn test_org_policy_get_set() {
    let (dir, _store) = setup_repo();
    let org_dir = tempfile::tempdir().unwrap();
    let org_path = org_dir.path().join("org.json");

    let config = ServiceConfig {
        repos: vec![RepoEntry {
            name: "default".into(),
            morph_dir: dir.path().join(".morph"),
        }],
        addr: "127.0.0.1:0".parse().unwrap(),
        org_policy_path: Some(org_path.clone()),
    };

    let app = build_router(&config);
    let (status, json) = get_json(app, "/api/org/policy").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.is_null());

    let new_policy = serde_json::json!({
        "required_metrics": ["acc"],
        "thresholds": {"acc": 0.8}
    });
    let app = build_router(&config);
    let (status, json) = post_json(app, "/api/org/policy", new_policy).await;
    assert_eq!(status, StatusCode::OK);
    let metrics = json["required_metrics"].as_array().unwrap();
    assert!(metrics.iter().any(|v| v == "acc"));

    assert!(org_path.exists(), "org policy file should be persisted");
}

// ── Multi-repo ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_multi_repo() {
    let (dir_a, store_a) = setup_repo();
    let (dir_b, store_b) = setup_repo();
    make_commit_with_metrics(store_a.as_ref(), &dir_a, BTreeMap::new(), "repo a");
    make_commit_with_metrics(store_b.as_ref(), &dir_b, BTreeMap::new(), "repo b");

    let config = ServiceConfig {
        repos: vec![
            RepoEntry {
                name: "alpha".into(),
                morph_dir: dir_a.path().join(".morph"),
            },
            RepoEntry {
                name: "beta".into(),
                morph_dir: dir_b.path().join(".morph"),
            },
        ],
        addr: "127.0.0.1:0".parse().unwrap(),
        org_policy_path: None,
    };

    let app = build_router(&config);
    let (status, json) = get_json(app.clone(), "/api/repos").await;
    assert_eq!(status, StatusCode::OK);
    let repos = json["repos"].as_array().unwrap();
    assert_eq!(repos.len(), 2);

    let (status, json) = get_json(app.clone(), "/api/repos/alpha/commits").await;
    assert_eq!(status, StatusCode::OK);
    let commits = json["commits"].as_array().unwrap();
    assert_eq!(commits[0]["message"], "repo a");

    let (status, json) = get_json(app, "/api/repos/beta/commits").await;
    assert_eq!(status, StatusCode::OK);
    let commits = json["commits"].as_array().unwrap();
    assert_eq!(commits[0]["message"], "repo b");
}

// ── Static pages ────────────────────────────────────────────────────

#[tokio::test]
async fn test_index_html() {
    let (dir, _store) = setup_repo();
    let app = build_router(&make_config(&dir));
    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("Morph"));
}

// ── Service layer unit tests ────────────────────────────────────────

#[test]
fn service_repo_summary_empty() {
    let (dir, _store) = setup_repo();
    let ctx = crate::service::RepoContext {
        name: "test".into(),
        morph_dir: dir.path().join(".morph"),
    };
    let summary = ctx.summary().unwrap();
    assert_eq!(summary.commit_count, 0);
    assert_eq!(summary.run_count, 0);
}

#[test]
fn service_commit_detail_with_evidence() {
    let (dir, store) = setup_repo();
    let mut m = BTreeMap::new();
    m.insert("acc".into(), 0.95);
    let hash = make_commit_with_metrics(store.as_ref(), &dir, m, "evidence test");

    let ctx = crate::service::RepoContext {
        name: "test".into(),
        morph_dir: dir.path().join(".morph"),
    };
    let detail = ctx.commit_detail(&hash.to_string()).unwrap();
    assert_eq!(detail.eval_contract.observed_metrics["acc"], 0.95);
    assert!(!detail.behavioral_status.is_merge);
}

#[test]
fn service_run_trace_pipeline_views() {
    let (dir, store) = setup_repo();
    let run_hash =
        morph_core::record_session(store.as_ref(), "q", "a", Some("model"), Some("agent"))
            .unwrap();
    let run_obj = match store.get(&run_hash).unwrap() {
        MorphObject::Run(r) => r,
        _ => panic!("expected run"),
    };
    let pipeline_hash =
        morph_core::extract_pipeline_from_run(store.as_ref(), &run_hash).unwrap();

    let ctx = crate::service::RepoContext {
        name: "test".into(),
        morph_dir: dir.path().join(".morph"),
    };

    let run_view = ctx.run_detail(&run_hash.to_string()).unwrap();
    assert_eq!(run_view.agent.id, "agent");
    assert_eq!(run_view.environment.model, "model");

    let trace_view = ctx.trace_detail(&run_obj.trace).unwrap();
    assert_eq!(trace_view.event_count, 2);

    let pipeline_view = ctx.pipeline_detail(&pipeline_hash.to_string()).unwrap();
    assert_eq!(pipeline_view.node_count, 2);
    assert!(pipeline_view.provenance.is_some());
}

#[test]
fn service_policy_view() {
    let (dir, _store) = setup_repo();
    let ctx = crate::service::RepoContext {
        name: "test".into(),
        morph_dir: dir.path().join(".morph"),
    };
    let policy = ctx.policy(None).unwrap();
    assert_eq!(policy.repo_policy.merge_policy, "dominance");
}

#[test]
fn service_missing_object_error() {
    let (dir, _store) = setup_repo();
    let ctx = crate::service::RepoContext {
        name: "test".into(),
        morph_dir: dir.path().join(".morph"),
    };
    let fake = "a".repeat(64);
    let result = ctx.commit_detail(&fake);
    assert!(result.is_err());
}

#[test]
fn service_invalid_hash_error() {
    let (dir, _store) = setup_repo();
    let ctx = crate::service::RepoContext {
        name: "test".into(),
        morph_dir: dir.path().join(".morph"),
    };
    let result = ctx.commit_detail("not-a-hash");
    assert!(result.is_err());
}
