//! Library to serve a Morph repo for browser-based browsing.
//! Used by `morph visualize`. Reads .morph/ directly; no export.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use morph_core::{log_from, open_store, Hash, MorphObject, ObjectType, Store};
use std::net::SocketAddr;
use std::path::PathBuf;
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

static INDEX_HTML: &str = include_str!("../static/index.html");
static GRAPH_HTML: &str = include_str!("../static/graph.html");

#[derive(Clone)]
struct AppState {
    morph_dir: PathBuf,
}

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

/// Run the serve loop (blocking). Call from CLI. Binds to `addr`.
pub fn run_blocking(
    morph_dir: PathBuf,
    addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("morph_serve=info".parse()?))
        .init();

    tracing::info!(
        "morph visualize at http://{} (repo: {})",
        addr,
        morph_dir.parent().unwrap_or(&morph_dir).display()
    );

    let state = AppState { morph_dir };

    let app = Router::new()
        .route("/", get(|| async { Html(INDEX_HTML) }))
        .route("/index.html", get(|| async { Html(INDEX_HTML) }))
        .route("/api/log", get(api_log))
        .route("/api/runs", get(api_runs))
        .route("/api/graph", get(api_graph))
        .route("/api/object/{hash}", get(api_object))
        .route("/graph", get(|| async { Html(GRAPH_HTML) }))
        .route("/graph.html", get(|| async { Html(GRAPH_HTML) }))
        .with_state(state)
        .layer(CorsLayer::permissive());

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await
    })?;
    Ok(())
}

async fn api_log(State(state): State<AppState>) -> Result<Json<Vec<CommitEntry>>, ApiError> {
    let store = open_store(&state.morph_dir)?;
    let hashes = log_from(store.as_ref(), "HEAD")?;
    let mut out = Vec::with_capacity(hashes.len());
    for h in hashes {
        let obj = store.get(&h)?;
        let commit = match &obj {
            MorphObject::Commit(c) => c,
            _ => continue,
        };
        out.push(CommitEntry {
            hash: h.to_string(),
            message: commit.message.clone(),
            author: commit.author.clone(),
            timestamp: commit.timestamp.clone(),
            program: commit.program.clone(),
            parents: commit.parents.clone(),
            eval_contract: commit.eval_contract.clone(),
            tree: commit.tree.clone(),
            morph_version: commit.morph_version.clone(),
        });
    }
    Ok(Json(out))
}

async fn api_runs(State(state): State<AppState>) -> Result<Json<Vec<RunEntry>>, ApiError> {
    let store = open_store(&state.morph_dir)?;
    let hashes = store.list(ObjectType::Run)?;
    let mut out = Vec::with_capacity(hashes.len());
    for h in hashes {
        let obj = store.get(&h)?;
        if let MorphObject::Run(run) = &obj {
            out.push(RunEntry {
                hash: h.to_string(),
                trace: run.trace.clone(),
                program: run.program.clone(),
                agent: format!("{} {}", run.agent.id, run.agent.version),
            });
        }
    }
    Ok(Json(out))
}

#[derive(serde::Serialize)]
struct RunEntry {
    hash: String,
    trace: String,
    program: String,
    agent: String,
}

#[derive(serde::Serialize)]
struct CommitEntry {
    hash: String,
    message: String,
    author: String,
    timestamp: String,
    program: String,
    parents: Vec<String>,
    eval_contract: morph_core::objects::EvalContract,
    tree: Option<String>,
    morph_version: Option<String>,
}

async fn api_graph(State(state): State<AppState>) -> Result<Json<GraphResponse>, ApiError> {
    let store = open_store(&state.morph_dir)?;
    let mut nodes: std::collections::HashMap<String, GraphNode> = std::collections::HashMap::new();
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
                        MorphObject::Commit(c) => ("commit", c.message.lines().next().unwrap_or("").to_string()),
                        MorphObject::Run(r) => ("run", format!("{} {}", r.agent.id, r.agent.version)),
                        MorphObject::Trace(_) => ("trace", "trace".to_string()),
                        MorphObject::Program(_) => ("program", "program".to_string()),
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
                            let label = if text.is_empty() {
                                id[..12.min(id.len())].to_string()
                            } else {
                                let truncated = if text.len() > 24 { &text[..24] } else { text };
                                format!("{}…", truncated.trim_end())
                            };
                            ("prompt", label)
                        }
                        _ => ("object", id[..12.min(id.len())].to_string()),
                    };
                    (t.to_string(), if l.is_empty() { id[..12.min(id.len())].to_string() } else { l })
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
                node_type: node_type,
                label: if node_label.is_empty() { id[..12.min(id.len())].to_string() } else { node_label },
            },
        );
    }

    if let Ok(commit_hashes) = log_from(store.as_ref(), "HEAD") {
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
            let label = if msg.is_empty() { id[..12.min(id.len())].to_string() } else { msg.to_string() };
            ensure_node(&mut nodes, store.as_ref(), &id, "commit", label);
            if let Some(ref tree) = commit.tree {
                edges.push(GraphEdge { from: id.clone(), to: tree.clone() });
                ensure_node(&mut nodes, store.as_ref(), tree, "tree", "tree".to_string());
            }
            edges.push(GraphEdge { from: id.clone(), to: commit.program.clone() });
            ensure_node(&mut nodes, store.as_ref(), &commit.program, "program", "program".to_string());
            for p in &commit.parents {
                edges.push(GraphEdge { from: p.clone(), to: id.clone() });
                ensure_node(&mut nodes, store.as_ref(), p, "?", "".to_string());
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
        ensure_node(&mut nodes, store.as_ref(), &id, "run", label);
        edges.push(GraphEdge { from: id.clone(), to: run.trace.clone() });
        ensure_node(&mut nodes, store.as_ref(), &run.trace, "trace", "trace".to_string());
        edges.push(GraphEdge { from: id.clone(), to: run.program.clone() });
        ensure_node(&mut nodes, store.as_ref(), &run.program, "program", "program".to_string());
    }

    // Add prompt nodes and program -> prompt edges for each program's prompts
    let program_ids: Vec<String> = nodes
        .values()
        .filter(|n| n.node_type == "program")
        .map(|n| n.id.clone())
        .collect();
    for program_id in program_ids {
        let program_hash = match Hash::from_hex(&program_id) {
            Ok(h) => h,
            Err(_) => continue,
        };
        let obj = match store.get(&program_hash) {
            Ok(o) => o,
            Err(_) => continue,
        };
        if let MorphObject::Program(program) = &obj {
            for prompt_hash in &program.prompts {
                if prompt_hash.is_empty() {
                    continue;
                }
                ensure_node(&mut nodes, store.as_ref(), prompt_hash, "?", String::new());
                if !edges.iter().any(|e| e.from == program_id && e.to == *prompt_hash) {
                    edges.push(GraphEdge {
                        from: program_id.clone(),
                        to: prompt_hash.clone(),
                    });
                }
            }
        }
    }

    let nodes: Vec<GraphNode> = nodes.into_values().collect();
    Ok(Json(GraphResponse { nodes, edges }))
}

async fn api_object(
    State(state): State<AppState>,
    Path(hash_str): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let hash = Hash::from_hex(&hash_str).map_err(|_| ApiError::BadHash)?;
    let store = open_store(&state.morph_dir)?;
    let obj = store.get(&hash)?;
    let json = serde_json::to_value(&obj).map_err(|e| ApiError::Serialize(e.to_string()))?;
    Ok(Json(json))
}

enum ApiError {
    Store(morph_core::MorphError),
    BadHash,
    Serialize(String),
}

impl From<morph_core::MorphError> for ApiError {
    fn from(e: morph_core::MorphError) -> Self {
        ApiError::Store(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg) = match &self {
            ApiError::Store(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            ApiError::BadHash => (StatusCode::BAD_REQUEST, "invalid hash".into()),
            ApiError::Serialize(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.clone()),
        };
        (status, Html(msg)).into_response()
    }
}
