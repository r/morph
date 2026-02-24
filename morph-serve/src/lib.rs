//! Library to serve a Morph repo for browser-based browsing.
//! Used by `morph visualize`. Reads .morph/ directly; no export.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use morph_core::{log_from, open_store, Hash, MorphObject, Store};
use std::net::SocketAddr;
use std::path::PathBuf;
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

static INDEX_HTML: &str = include_str!("../static/index.html");

#[derive(Clone)]
struct AppState {
    morph_dir: PathBuf,
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
        .route("/api/object/{hash}", get(api_object))
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
