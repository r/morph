//! Axum HTTP handlers for the Morph hosted service.

use crate::org_policy::{self, OrgPolicy};
use crate::service::RepoContext;
use crate::views::*;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::Json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

// ── Shared state ────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub repos: Arc<BTreeMap<String, RepoContext>>,
    pub org_policy: Arc<RwLock<Option<OrgPolicy>>>,
    pub org_policy_path: Option<PathBuf>,
}

impl AppState {
    fn repo(&self, name: &str) -> Result<&RepoContext, ApiError> {
        self.repos
            .get(name)
            .ok_or_else(|| ApiError::RepoNotFound(name.to_string()))
    }

    fn default_repo(&self) -> Result<&RepoContext, ApiError> {
        self.repos
            .get("default")
            .or_else(|| self.repos.values().next())
            .ok_or_else(|| ApiError::RepoNotFound("no repos configured".to_string()))
    }

    fn org(&self) -> Option<OrgPolicy> {
        self.org_policy.read().ok().and_then(|g| g.clone())
    }
}

// ── Error type ──────────────────────────────────────────────────────

pub enum ApiError {
    Store(morph_core::MorphError),
    RepoNotFound(String),
    BadHash,
    Internal(String),
}

impl From<morph_core::MorphError> for ApiError {
    fn from(e: morph_core::MorphError) -> Self {
        match &e {
            morph_core::MorphError::NotFound(_) => ApiError::Store(e),
            morph_core::MorphError::InvalidHash(_) => ApiError::BadHash,
            _ => ApiError::Store(e),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, code, msg) = match &self {
            ApiError::Store(morph_core::MorphError::NotFound(h)) => {
                (StatusCode::NOT_FOUND, "not_found", format!("object not found: {}", h))
            }
            ApiError::Store(e) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string())
            }
            ApiError::RepoNotFound(name) => {
                (StatusCode::NOT_FOUND, "repo_not_found", format!("repo not found: {}", name))
            }
            ApiError::BadHash => {
                (StatusCode::BAD_REQUEST, "bad_hash", "invalid object hash".to_string())
            }
            ApiError::Internal(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", msg.clone())
            }
        };
        let body = ErrorResponse {
            error: msg,
            code: code.to_string(),
        };
        (status, Json(body)).into_response()
    }
}

// ── Static pages ────────────────────────────────────────────────────

static INDEX_HTML: &str = include_str!("../static/index.html");
static GRAPH_HTML: &str = include_str!("../static/graph.html");

pub async fn page_index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

pub async fn page_graph() -> Html<&'static str> {
    Html(GRAPH_HTML)
}

// ── Repo-scoped endpoints (/api/repos/{repo}/...) ───────────────────

pub async fn api_repo_list(
    State(state): State<AppState>,
) -> Result<Json<RepoListResponse>, ApiError> {
    let mut repos = Vec::new();
    for ctx in state.repos.values() {
        match ctx.summary() {
            Ok(s) => repos.push(s),
            Err(_) => repos.push(RepoSummary {
                name: ctx.name.clone(),
                head: None,
                current_branch: None,
                branch_count: 0,
                commit_count: 0,
                run_count: 0,
            }),
        }
    }
    Ok(Json(RepoListResponse { repos }))
}

pub async fn api_repo_summary(
    State(state): State<AppState>,
    Path(repo): Path<String>,
) -> Result<Json<RepoSummary>, ApiError> {
    let ctx = state.repo(&repo)?;
    Ok(Json(ctx.summary()?))
}

pub async fn api_branches(
    State(state): State<AppState>,
    Path(repo): Path<String>,
) -> Result<Json<BranchListResponse>, ApiError> {
    let ctx = state.repo(&repo)?;
    Ok(Json(ctx.list_branches()?))
}

pub async fn api_commits(
    State(state): State<AppState>,
    Path(repo): Path<String>,
) -> Result<Json<CommitListResponse>, ApiError> {
    let ctx = state.repo(&repo)?;
    Ok(Json(ctx.list_commits("HEAD")?))
}

pub async fn api_commit_detail(
    State(state): State<AppState>,
    Path((repo, hash)): Path<(String, String)>,
) -> Result<Json<CommitDetailResponse>, ApiError> {
    let ctx = state.repo(&repo)?;
    Ok(Json(ctx.commit_detail(&hash)?))
}

pub async fn api_runs(
    State(state): State<AppState>,
    Path(repo): Path<String>,
) -> Result<Json<RunListResponse>, ApiError> {
    let ctx = state.repo(&repo)?;
    Ok(Json(ctx.list_runs()?))
}

pub async fn api_run_detail(
    State(state): State<AppState>,
    Path((repo, hash)): Path<(String, String)>,
) -> Result<Json<RunDetailResponse>, ApiError> {
    let ctx = state.repo(&repo)?;
    Ok(Json(ctx.run_detail(&hash)?))
}

pub async fn api_trace_detail(
    State(state): State<AppState>,
    Path((repo, hash)): Path<(String, String)>,
) -> Result<Json<TraceDetailResponse>, ApiError> {
    let ctx = state.repo(&repo)?;
    Ok(Json(ctx.trace_detail(&hash)?))
}

pub async fn api_pipeline_detail(
    State(state): State<AppState>,
    Path((repo, hash)): Path<(String, String)>,
) -> Result<Json<PipelineDetailResponse>, ApiError> {
    let ctx = state.repo(&repo)?;
    Ok(Json(ctx.pipeline_detail(&hash)?))
}

pub async fn api_object(
    State(state): State<AppState>,
    Path((repo, hash)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ctx = state.repo(&repo)?;
    Ok(Json(ctx.raw_object(&hash)?))
}

pub async fn api_annotations(
    State(state): State<AppState>,
    Path((repo, hash)): Path<(String, String)>,
) -> Result<Json<AnnotationsResponse>, ApiError> {
    let ctx = state.repo(&repo)?;
    Ok(Json(ctx.annotations(&hash)?))
}

pub async fn api_policy(
    State(state): State<AppState>,
    Path(repo): Path<String>,
) -> Result<Json<PolicyResponse>, ApiError> {
    let ctx = state.repo(&repo)?;
    let org = state.org();
    Ok(Json(ctx.policy(org.as_ref())?))
}

pub async fn api_gate(
    State(state): State<AppState>,
    Path((repo, hash)): Path<(String, String)>,
) -> Result<Json<GateStatusResponse>, ApiError> {
    let ctx = state.repo(&repo)?;
    Ok(Json(ctx.gate_status(&hash)?))
}

// ── Org policy endpoints ────────────────────────────────────────────

pub async fn api_org_policy_get(
    State(state): State<AppState>,
) -> Result<Json<Option<OrgPolicyView>>, ApiError> {
    let org = state.org();
    let view = org.map(|o| OrgPolicyView {
        required_metrics: o.required_metrics,
        thresholds: o.thresholds,
        directions: o.directions,
        presets: o
            .presets
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    PolicyPresetView {
                        required_metrics: v.required_metrics,
                        thresholds: v.thresholds,
                    },
                )
            })
            .collect(),
    });
    Ok(Json(view))
}

pub async fn api_org_policy_set(
    State(state): State<AppState>,
    Json(new_policy): Json<OrgPolicy>,
) -> Result<Json<OrgPolicyView>, ApiError> {
    if let Some(ref path) = state.org_policy_path {
        org_policy::save_org_policy(path, &new_policy)
            .map_err(|e| ApiError::Internal(e))?;
    }
    let view = OrgPolicyView {
        required_metrics: new_policy.required_metrics.clone(),
        thresholds: new_policy.thresholds.clone(),
        directions: new_policy.directions.clone(),
        presets: new_policy
            .presets
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    PolicyPresetView {
                        required_metrics: v.required_metrics.clone(),
                        thresholds: v.thresholds.clone(),
                    },
                )
            })
            .collect(),
    };
    if let Ok(mut guard) = state.org_policy.write() {
        *guard = Some(new_policy);
    }
    Ok(Json(view))
}

// ── Backward-compatible endpoints (/api/log, /api/runs, etc.) ───────

pub async fn api_compat_log(
    State(state): State<AppState>,
) -> Result<Json<CommitListResponse>, ApiError> {
    let ctx = state.default_repo()?;
    Ok(Json(ctx.list_commits("HEAD")?))
}

pub async fn api_compat_runs(
    State(state): State<AppState>,
) -> Result<Json<RunListResponse>, ApiError> {
    let ctx = state.default_repo()?;
    Ok(Json(ctx.list_runs()?))
}

pub async fn api_compat_object(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ctx = state.default_repo()?;
    Ok(Json(ctx.raw_object(&hash)?))
}

pub async fn api_compat_graph(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ctx = state.default_repo()?;
    let store = ctx.open_store()?;
    let graph = crate::graph::build_graph_response(store.as_ref())?;
    Ok(Json(graph))
}
