//! Morph hosted service: shared inspection and policy layer.
//!
//! Serves one or more Morph repositories over HTTP with stable JSON APIs
//! for browsing commits, runs, traces, pipelines, behavioral status,
//! certifications, and org-level policy.

pub(crate) mod graph;
pub mod handlers;
pub mod org_policy;
pub mod service;
pub mod views;

use axum::routing::get;
use axum::Router;
use handlers::AppState;
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

#[cfg(test)]
mod tests;
