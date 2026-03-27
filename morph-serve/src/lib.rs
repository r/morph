//! Morph hosted service: shared inspection and policy layer.
//!
//! Serves one or more Morph repositories over HTTP with stable JSON APIs
//! for browsing commits, runs, traces, pipelines, behavioral status,
//! certifications, and org-level policy.

pub mod handlers;
pub mod org_policy;
pub mod service;
pub mod views;

use axum::routing::get;
use axum::Router;
use handlers::AppState;
use morph_core::objects::MorphObject;
use morph_core::store::{MorphError, ObjectType, Store};
use morph_core::{log_from, Hash};
use service::RepoContext;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

/// Configuration for the Morph hosted service.
#[derive(Clone)]
pub struct ServiceConfig {
    pub repos: Vec<RepoEntry>,
    pub addr: SocketAddr,
    pub org_policy_path: Option<PathBuf>,
}

/// A named repository entry.
#[derive(Clone)]
pub struct RepoEntry {
    pub name: String,
    pub morph_dir: PathBuf,
}

/// Build the Axum router (usable in tests without starting a server).
pub fn build_router(config: &ServiceConfig) -> Router {
    let mut repos = BTreeMap::new();
    for entry in &config.repos {
        repos.insert(
            entry.name.clone(),
            RepoContext {
                name: entry.name.clone(),
                morph_dir: entry.morph_dir.clone(),
            },
        );
    }

    let org = config
        .org_policy_path
        .as_ref()
        .and_then(|p| org_policy::load_org_policy(p).ok().flatten());

    let state = AppState {
        repos: Arc::new(repos),
        org_policy: Arc::new(RwLock::new(org)),
        org_policy_path: config.org_policy_path.clone(),
    };

    Router::new()
        // Static pages
        .route("/", get(handlers::page_index))
        .route("/index.html", get(handlers::page_index))
        .route("/graph", get(handlers::page_graph))
        .route("/graph.html", get(handlers::page_graph))
        // Repo-scoped API
        .route("/api/repos", get(handlers::api_repo_list))
        .route("/api/repos/{repo}/summary", get(handlers::api_repo_summary))
        .route("/api/repos/{repo}/branches", get(handlers::api_branches))
        .route("/api/repos/{repo}/commits", get(handlers::api_commits))
        .route(
            "/api/repos/{repo}/commits/{hash}",
            get(handlers::api_commit_detail),
        )
        .route("/api/repos/{repo}/runs", get(handlers::api_runs))
        .route(
            "/api/repos/{repo}/runs/{hash}",
            get(handlers::api_run_detail),
        )
        .route(
            "/api/repos/{repo}/traces/{hash}",
            get(handlers::api_trace_detail),
        )
        .route(
            "/api/repos/{repo}/pipelines/{hash}",
            get(handlers::api_pipeline_detail),
        )
        .route(
            "/api/repos/{repo}/objects/{hash}",
            get(handlers::api_object),
        )
        .route(
            "/api/repos/{repo}/annotations/{hash}",
            get(handlers::api_annotations),
        )
        .route("/api/repos/{repo}/policy", get(handlers::api_policy))
        .route(
            "/api/repos/{repo}/gate/{hash}",
            get(handlers::api_gate),
        )
        // Org-level policy
        .route(
            "/api/org/policy",
            get(handlers::api_org_policy_get).post(handlers::api_org_policy_set),
        )
        // Backward-compatible endpoints (default repo)
        .route("/api/log", get(handlers::api_compat_log))
        .route("/api/runs", get(handlers::api_compat_runs))
        .route("/api/object/{hash}", get(handlers::api_compat_object))
        .route("/api/graph", get(handlers::api_compat_graph))
        .with_state(state)
        .layer(CorsLayer::permissive())
}

/// Legacy entry point: single repo. Backward-compatible with `morph visualize`.
pub fn run_blocking(
    morph_dir: PathBuf,
    addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = ServiceConfig {
        repos: vec![RepoEntry {
            name: "default".to_string(),
            morph_dir,
        }],
        addr,
        org_policy_path: None,
    };
    run_service(config)
}

/// Full service entry point with multi-repo and org policy support.
pub fn run_service(
    config: ServiceConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("morph_serve=info".parse()?),
        )
        .try_init()
        .ok();

    let addr = config.addr;
    let repo_names: Vec<_> = config.repos.iter().map(|r| r.name.clone()).collect();
    tracing::info!(
        "morph serve at http://{} (repos: {})",
        addr,
        repo_names.join(", ")
    );

    let app = build_router(&config);

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await
    })?;
    Ok(())
}

// ── Graph response builder (used by backward-compat /api/graph) ─────

#[derive(serde::Serialize)]
struct GraphResponse {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

#[derive(serde::Serialize)]
struct GraphNode {
    id: String,
    #[serde(rename = "type")]
    node_type: String,
    label: String,
}

#[derive(serde::Serialize)]
struct GraphEdge {
    from: String,
    to: String,
}

pub(crate) fn build_graph_response(
    store: &dyn Store,
) -> Result<serde_json::Value, MorphError> {
    let mut nodes: std::collections::HashMap<String, GraphNode> =
        std::collections::HashMap::new();
    let mut edges: Vec<GraphEdge> = Vec::new();

    fn ensure_node(
        nodes: &mut std::collections::HashMap<String, GraphNode>,
        store: &dyn Store,
        id: &str,
        preferred_type: &str,
        label: String,
    ) {
        if nodes.contains_key(id) {
            return;
        }
        let (node_type, node_label) = if preferred_type != "?" {
            (preferred_type.to_string(), label)
        } else if let Ok(h) = Hash::from_hex(id) {
            match store.get(&h) {
                Ok(obj) => {
                    let (t, l) = match &obj {
                        MorphObject::Commit(c) => {
                            ("commit", c.message.lines().next().unwrap_or("").to_string())
                        }
                        MorphObject::Run(r) => {
                            ("run", format!("{} {}", r.agent.id, r.agent.version))
                        }
                        MorphObject::Trace(_) => ("trace", "trace".to_string()),
                        MorphObject::Pipeline(_) => ("pipeline", "pipeline".to_string()),
                        MorphObject::Tree(_) => ("tree", "tree".to_string()),
                        MorphObject::Blob(b) if b.kind == "prompt" => {
                            let text = b
                                .content
                                .get("text")
                                .or_else(|| b.content.get("body"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .lines()
                                .next()
                                .unwrap_or("")
                                .trim();
                            let lbl = if text.is_empty() {
                                id[..12.min(id.len())].to_string()
                            } else {
                                let truncated = if text.len() > 24 { &text[..24] } else { text };
                                format!("{}…", truncated.trim_end())
                            };
                            ("prompt", lbl)
                        }
                        _ => ("object", id[..12.min(id.len())].to_string()),
                    };
                    (
                        t.to_string(),
                        if l.is_empty() {
                            id[..12.min(id.len())].to_string()
                        } else {
                            l
                        },
                    )
                }
                Err(_) => ("object".to_string(), id[..12.min(id.len())].to_string()),
            }
        } else {
            ("object".to_string(), id[..12.min(id.len())].to_string())
        };
        nodes.insert(
            id.to_string(),
            GraphNode {
                id: id.to_string(),
                node_type,
                label: if node_label.is_empty() {
                    id[..12.min(id.len())].to_string()
                } else {
                    node_label
                },
            },
        );
    }

    if let Ok(commit_hashes) = log_from(store, "HEAD") {
        for h in commit_hashes {
            let id = h.to_string();
            let obj = match store.get(&h) {
                Ok(o) => o,
                Err(_) => continue,
            };
            let commit = match &obj {
                MorphObject::Commit(c) => c,
                _ => continue,
            };
            let msg = commit.message.lines().next().unwrap_or("").trim();
            let label = if msg.is_empty() {
                id[..12.min(id.len())].to_string()
            } else {
                msg.to_string()
            };
            ensure_node(&mut nodes, store, &id, "commit", label);
            if let Some(ref tree) = commit.tree {
                edges.push(GraphEdge {
                    from: id.clone(),
                    to: tree.clone(),
                });
                ensure_node(&mut nodes, store, tree, "tree", "tree".to_string());
            }
            edges.push(GraphEdge {
                from: id.clone(),
                to: commit.pipeline.clone(),
            });
            ensure_node(
                &mut nodes,
                store,
                &commit.pipeline,
                "pipeline",
                "pipeline".to_string(),
            );
            for p in &commit.parents {
                edges.push(GraphEdge {
                    from: p.clone(),
                    to: id.clone(),
                });
                ensure_node(&mut nodes, store, p, "?", "".to_string());
            }
        }
    }

    let run_hashes = store.list(ObjectType::Run)?;
    for h in run_hashes {
        let id = h.to_string();
        let obj = match store.get(&h) {
            Ok(o) => o,
            Err(_) => continue,
        };
        let run = match &obj {
            MorphObject::Run(r) => r,
            _ => continue,
        };
        let label = format!("{} {}", run.agent.id, run.agent.version);
        ensure_node(&mut nodes, store, &id, "run", label);
        edges.push(GraphEdge {
            from: id.clone(),
            to: run.trace.clone(),
        });
        ensure_node(
            &mut nodes,
            store,
            &run.trace,
            "trace",
            "trace".to_string(),
        );
        edges.push(GraphEdge {
            from: id.clone(),
            to: run.pipeline.clone(),
        });
        ensure_node(
            &mut nodes,
            store,
            &run.pipeline,
            "pipeline",
            "pipeline".to_string(),
        );
    }

    let pipeline_ids: Vec<String> = nodes
        .values()
        .filter(|n| n.node_type == "pipeline")
        .map(|n| n.id.clone())
        .collect();
    for pipeline_id in pipeline_ids {
        let pipeline_hash = match Hash::from_hex(&pipeline_id) {
            Ok(h) => h,
            Err(_) => continue,
        };
        let obj = match store.get(&pipeline_hash) {
            Ok(o) => o,
            Err(_) => continue,
        };
        if let MorphObject::Pipeline(pipeline) = &obj {
            for prompt_hash in &pipeline.prompts {
                if prompt_hash.is_empty() {
                    continue;
                }
                ensure_node(&mut nodes, store, prompt_hash, "?", String::new());
                if !edges.iter().any(|e| e.from == pipeline_id && e.to == *prompt_hash) {
                    edges.push(GraphEdge {
                        from: pipeline_id.clone(),
                        to: prompt_hash.clone(),
                    });
                }
            }
        }
    }

    let nodes_vec: Vec<GraphNode> = nodes.into_values().collect();
    let resp = GraphResponse {
        nodes: nodes_vec,
        edges,
    };
    serde_json::to_value(&resp).map_err(|e| MorphError::Serialization(e.to_string()))
}

#[cfg(test)]
mod tests;
