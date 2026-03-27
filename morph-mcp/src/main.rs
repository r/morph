//! Cursor MCP server: primary write path from the IDE.
//! Exposes morph-core operations as MCP tools.

use morph_core::{find_repo, open_store, read_repo_version, require_store_version, Hash, Store, STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_INIT};
use rmcp::{
    handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::*,
    tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError,
    ServerHandler, ServiceExt,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;

/// Resolve default workspace: env MORPH_WORKSPACE, then CURSOR_WORKSPACE_FOLDER / WORKSPACE_FOLDER, then optional first arg.
fn default_workspace_from_env_and_args(args: &[String]) -> Option<PathBuf> {
    std::env::var_os("MORPH_WORKSPACE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CURSOR_WORKSPACE_FOLDER").map(PathBuf::from))
        .or_else(|| std::env::var_os("WORKSPACE_FOLDER").map(PathBuf::from))
        .or_else(|| {
            // First non-flag arg can be workspace path (e.g. morph-mcp /path or morph-mcp .)
            args.get(1).filter(|a| !a.starts_with('-')).map(PathBuf::from)
        })
}

#[derive(Clone)]
pub struct MorphServer {
    tool_router: ToolRouter<Self>,
    /// Default workspace when tool call omits workspace_path (from env or args at startup).
    default_workspace: Option<PathBuf>,
}

#[tool_router]
impl MorphServer {
    fn new(default_workspace: Option<PathBuf>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            default_workspace,
        }
    }

    fn repo_store(&self, workspace_path: Option<&str>) -> Result<(PathBuf, Box<dyn Store>), String> {
        let start = workspace_path
            .map(PathBuf::from)
            .or_else(|| self.default_workspace.clone())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let repo_root = find_repo(&start).ok_or_else(|| {
            let tried = start.canonicalize().unwrap_or(start).display().to_string();
            format!(
                "not a morph repository (no .morph/ found from {}). \
                Fix: (1) run 'morph init' in the project root, or (2) set MORPH_WORKSPACE in .cursor/mcp.json to the project root, or (3) pass workspace_path with the full path to the project root.",
                tried
            )
        })?;
        let morph_dir = repo_root.join(".morph");
        require_store_version(&morph_dir, &[STORE_VERSION_INIT, STORE_VERSION_0_2, STORE_VERSION_0_3]).map_err(|e| e.to_string())?;
        let store = open_store(&morph_dir).map_err(|e| e.to_string())?;
        Ok((repo_root, store))
    }

    #[tool(description = "Initialize a Morph repository in the given path (default: current directory)")]
    async fn morph_init(
        &self,
        params: Parameters<InitParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let path = params.0.path.unwrap_or_else(|| ".".to_string());
        let path = PathBuf::from(&path);
        morph_core::init_repo(&path).map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Initialized Morph repository in {}",
            path.display()
        ))]))
    }

    #[tool(description = "Record a Run object from JSON (execution receipt). Optional: trace_path, artifact_paths as JSON array.")]
    async fn morph_record_run(
        &self,
        params: Parameters<RecordRunParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref())
            .map_err(|e| McpError::invalid_params(e, None))?;
        let run_path = PathBuf::from(&params.0.run_file);
        let run_path = if run_path.is_absolute() {
            run_path
        } else {
            repo_root.join(run_path)
        };
        let trace_path = params.0.trace_file.map(|p| {
            let pb = PathBuf::from(p);
            if pb.is_absolute() {
                pb
            } else {
                repo_root.join(pb)
            }
        });
        let artifact_paths: Vec<_> = params
            .0
            .artifact_files
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                let pb = PathBuf::from(p);
                if pb.is_absolute() {
                    pb
                } else {
                    repo_root.join(pb)
                }
            })
            .collect();
        let refs: Vec<&std::path::Path> = artifact_paths.iter().map(PathBuf::as_path).collect();
        let hash = morph_core::record_run(&store, &run_path, trace_path.as_deref(), &refs)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::text(hash.to_string())]))
    }

    #[tool(description = "Record a single prompt/response session into Morph (Run + Trace). Call this at the end of every task/turn with the user's exact prompt and your complete response text (do not truncate). Optional: workspace_path (if tool reports 'not a morph repository'), model_name, agent_id.")]
    async fn morph_record_session(
        &self,
        params: Parameters<RecordSessionParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref())
            .map_err(|e| McpError::invalid_params(e, None))?;
        let hash = morph_core::record_session(
            &store,
            &params.0.prompt,
            &params.0.response,
            params.0.model_name.as_deref(),
            params.0.agent_id.as_deref(),
        )
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::text(hash.to_string())]))
    }

    #[tool(description = "Record evaluation metrics from a JSON file with a 'metrics' key")]
    async fn morph_record_eval(
        &self,
        params: Parameters<RecordEvalParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let (repo_root, _store) = self.repo_store(params.0.workspace_path.as_deref())
            .map_err(|e| McpError::invalid_params(e, None))?;
        let path = PathBuf::from(&params.0.file);
        let path = if path.is_absolute() { path } else { repo_root.join(path) };
        let metrics = morph_core::record_eval_metrics(&path)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let json = serde_json::to_string_pretty(&metrics).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    #[tool(description = "Stage paths into the object store (paths: array of paths, default [\".\"])")]
    async fn morph_stage(
        &self,
        params: Parameters<StageParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref())
            .map_err(|e| McpError::invalid_params(e, None))?;
        let paths: Vec<PathBuf> = params
            .0
            .paths
            .unwrap_or_else(|| vec![".".into()])
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let hashes = morph_core::add_paths(&store, &repo_root, &paths)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let out = hashes.iter().map(|h| h.to_string()).collect::<Vec<_>>().join("\n");
        Ok(CallToolResult::success(vec![Content::text(out)]))
    }

    #[tool(description = "Create a commit. Required: message. Optional: pipeline (hash), eval_suite (hash), metrics (JSON object), author, from_run (run hash for evidence-backed provenance).")]
    async fn morph_commit(
        &self,
        params: Parameters<CommitParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref())
            .map_err(|e| McpError::invalid_params(e, None))?;
        let morph_dir = repo_root.join(".morph");
        let version = read_repo_version(&morph_dir).map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let prog_hash = params.0.pipeline
            .as_deref()
            .map(Hash::from_hex)
            .transpose()
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let suite_hash = params.0.eval_suite
            .as_deref()
            .map(Hash::from_hex)
            .transpose()
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let metrics = params
            .0
            .metrics
            .unwrap_or_default();
        let provenance = match params.0.from_run {
            Some(ref run_hash_str) => {
                let run_hash = Hash::from_hex(run_hash_str)
                    .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                Some(morph_core::resolve_provenance_from_run(&store, &run_hash)
                    .map_err(|e| McpError::invalid_params(e.to_string(), None))?)
            }
            None => None,
        };
        let hash = morph_core::create_tree_commit_with_provenance(
            &store,
            &repo_root,
            prog_hash.as_ref(),
            suite_hash.as_ref(),
            metrics,
            params.0.message,
            params.0.author,
            Some(&version),
            provenance.as_ref(),
        )
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::text(hash.to_string())]))
    }

    #[tool(description = "Attach an annotation to an object. Required: target_hash, kind, data (JSON object). Optional: target_sub (e.g. event_id), author.")]
    async fn morph_annotate(
        &self,
        params: Parameters<AnnotateParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref())
            .map_err(|e| McpError::invalid_params(e, None))?;
        let target = Hash::from_hex(&params.0.target_hash).map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let ann = morph_core::create_annotation(
            &target,
            params.0.target_sub,
            params.0.kind,
            params.0.data.unwrap_or_default(),
            params.0.author,
        );
        let hash = store.put(&ann).map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::text(hash.to_string())]))
    }

    #[tool(description = "Create a new branch at current HEAD")]
    async fn morph_branch(
        &self,
        params: Parameters<BranchParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref())
            .map_err(|e| McpError::invalid_params(e, None))?;
        let head = morph_core::resolve_head(&store)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?
            .ok_or_else(|| McpError::invalid_params("no commit yet".to_string(), None))?;
        store
            .ref_write(&format!("heads/{}", params.0.name), &head)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Created branch {}",
            params.0.name
        ))]))
    }

    #[tool(description = "Switch HEAD to a branch or detached commit (ref_name: branch name or 64-char hash)")]
    async fn morph_checkout(
        &self,
        params: Parameters<CheckoutParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref())
            .map_err(|e| McpError::invalid_params(e, None))?;
        let ref_name = &params.0.ref_name;
        let (hash, tree_restored) = morph_core::checkout_tree(&store, &repo_root, ref_name)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let msg = if ref_name.len() == 64 && ref_name.chars().all(|c| c.is_ascii_hexdigit()) {
            format!("Detached HEAD at {}", hash)
        } else {
            format!("Switched to branch {}", ref_name.trim_start_matches("heads/"))
        };
        let tree_msg = if tree_restored {
            " (working tree restored)"
        } else {
            " (no file tree in commit; working tree unchanged)"
        };
        Ok(CallToolResult::success(vec![Content::text(format!("{}{}", msg, tree_msg))]))
    }
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
struct InitParams {
    path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RecordRunParams {
    run_file: String,
    #[serde(default)]
    workspace_path: Option<String>,
    #[serde(default)]
    trace_file: Option<String>,
    #[serde(default)]
    artifact_files: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RecordSessionParams {
    prompt: String,
    response: String,
    #[serde(default)]
    workspace_path: Option<String>,
    #[serde(default)]
    model_name: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RecordEvalParams {
    file: String,
    #[serde(default)]
    workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct StageParams {
    #[serde(default)]
    workspace_path: Option<String>,
    #[serde(default)]
    paths: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CommitParams {
    message: String,
    #[serde(default, alias = "program")]
    pipeline: Option<String>,
    #[serde(default)]
    eval_suite: Option<String>,
    #[serde(default)]
    workspace_path: Option<String>,
    #[serde(default)]
    metrics: Option<std::collections::BTreeMap<String, f64>>,
    #[serde(default)]
    author: Option<String>,
    /// Run hash to derive provenance from (evidence_refs, env_constraints, contributors).
    #[serde(default)]
    from_run: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AnnotateParams {
    target_hash: String,
    kind: String,
    #[serde(default)]
    data: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    #[serde(default)]
    target_sub: Option<String>,
    #[serde(default)]
    workspace_path: Option<String>,
    #[serde(default)]
    author: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BranchParams {
    name: String,
    #[serde(default)]
    workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CheckoutParams {
    ref_name: String,
    #[serde(default)]
    workspace_path: Option<String>,
}

#[tool_handler]
impl ServerHandler for MorphServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: rmcp::model::Implementation {
                name: "morph-mcp".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            },
            instructions: Some(
                "Morph VCS write path: init repos, record runs and evals, stage, commit, annotate, branch, checkout."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!(
            "morph-mcp {}\n\n\
             MCP server for Morph. Not meant to be run directly from a terminal.\n\
             Cursor (or another MCP host) starts this process and talks to it over stdio.\n\n\
             Usage: configured in Cursor MCP settings (e.g. ~/.cursor/mcp.json).\n\
             Optional: set MORPH_WORKSPACE to the repo root, or pass it as first arg.\n\
             Verify install: morph-mcp --version",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        eprintln!("morph-mcp {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let default_workspace = default_workspace_from_env_and_args(&args);
    let service = MorphServer::new(default_workspace)
        .serve(stdio())
        .await
        .inspect_err(|e| eprintln!("morph-mcp error: {}", e))?;
    service.waiting().await?;
    Ok(())
}
