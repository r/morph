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

    #[tool(description = "Show working-space status: files staged, tracked, and untracked")]
    async fn morph_status(
        &self,
        params: Parameters<WorkspaceOnlyParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref())
            .map_err(|e| McpError::invalid_params(e, None))?;
        let entries = morph_core::status(&store, &repo_root)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let mut out = String::new();
        for e in &entries {
            let status = if e.in_store { "tracked" } else { "new" };
            let hash_str = e.hash.as_ref().map(|h| h.to_string()).unwrap_or_default();
            out.push_str(&format!("{} {} {}\n", status, hash_str, e.path.display()));
        }
        if out.is_empty() {
            out = "clean (no changes)".to_string();
        }
        Ok(CallToolResult::success(vec![Content::text(out)]))
    }

    #[tool(description = "Show commit history from HEAD or a named ref. Returns commit hashes and messages.")]
    async fn morph_log(
        &self,
        params: Parameters<LogParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref())
            .map_err(|e| McpError::invalid_params(e, None))?;
        let ref_name = params.0.ref_name.as_deref().unwrap_or("HEAD");
        let hashes = morph_core::log_from(&store, ref_name)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let mut out = String::new();
        for h in &hashes {
            let obj = store.get(h).map_err(|e| McpError::invalid_params(e.to_string(), None))?;
            if let morph_core::MorphObject::Commit(c) = obj {
                out.push_str(&format!("{} {}\n", h, c.message));
            }
        }
        Ok(CallToolResult::success(vec![Content::text(out)]))
    }

    #[tool(description = "Show a stored Morph object as pretty JSON (commit, run, trace, pipeline, etc.)")]
    async fn morph_show(
        &self,
        params: Parameters<ShowParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref())
            .map_err(|e| McpError::invalid_params(e, None))?;
        let hash = Hash::from_hex(&params.0.hash)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let obj = store.get(&hash)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let json = serde_json::to_string_pretty(&obj)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    #[tool(description = "Diff two commits (or refs) and show file-level changes. Returns lines like 'A path', 'M path', 'D path'.")]
    async fn morph_diff(
        &self,
        params: Parameters<DiffParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref())
            .map_err(|e| McpError::invalid_params(e, None))?;
        let resolve = |r: &str| -> Result<Hash, String> {
            if r.len() == 64 && r.chars().all(|c| c.is_ascii_hexdigit()) {
                Hash::from_hex(r).map_err(|e| e.to_string())
            } else if r == "HEAD" {
                morph_core::resolve_head(&store)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "HEAD has no commits".to_string())
            } else {
                store.ref_read(&format!("heads/{}", r))
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("unknown ref: {}", r))
            }
        };
        let old_hash = resolve(&params.0.old_ref).map_err(|e| McpError::invalid_params(e, None))?;
        let new_hash = resolve(&params.0.new_ref).map_err(|e| McpError::invalid_params(e, None))?;
        let entries = morph_core::diff_commits(&store, &old_hash, &new_hash)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let mut out = String::new();
        for e in &entries {
            out.push_str(&format!("{}  {}\n", e.status, e.path));
        }
        if out.is_empty() {
            out = "no changes".to_string();
        }
        Ok(CallToolResult::success(vec![Content::text(out)]))
    }

    #[tool(description = "Merge a branch into the current branch (requires behavioral dominance). Required: branch, message, pipeline (hash), metrics (JSON object of metric name/value). Optional: eval_suite, author, retire (comma-separated metric names to retire).")]
    async fn morph_merge(
        &self,
        params: Parameters<MergeParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref())
            .map_err(|e| McpError::invalid_params(e, None))?;
        let pipeline = Hash::from_hex(&params.0.pipeline)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let suite = params.0.eval_suite
            .as_deref()
            .map(Hash::from_hex)
            .transpose()
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let retired: Option<Vec<String>> = params.0.retire
            .as_deref()
            .map(|s| s.split(',').map(|r| r.trim().to_string()).collect());
        let morph_dir = repo_root.join(".morph");
        let version = morph_core::read_repo_version(&morph_dir)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let plan = morph_core::prepare_merge(
            &store,
            &params.0.branch,
            suite.as_ref(),
            retired.as_deref(),
        ).map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let hash = morph_core::execute_merge(
            &store,
            &plan,
            &pipeline,
            params.0.metrics,
            params.0.message,
            params.0.author,
            Some(&repo_root),
            Some(&version),
        ).map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::text(hash.to_string())]))
    }
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
struct WorkspaceOnlyParams {
    #[serde(default)]
    workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LogParams {
    #[serde(default)]
    ref_name: Option<String>,
    #[serde(default)]
    workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ShowParams {
    hash: String,
    #[serde(default)]
    workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DiffParams {
    old_ref: String,
    new_ref: String,
    #[serde(default)]
    workspace_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct MergeParams {
    branch: String,
    message: String,
    pipeline: String,
    #[serde(default)]
    metrics: std::collections::BTreeMap<String, f64>,
    #[serde(default)]
    eval_suite: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    retire: Option<String>,
    #[serde(default)]
    workspace_path: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::wrapper::Parameters;
    use rmcp::model::RawContent;

    fn setup_repo() -> (tempfile::TempDir, MorphServer) {
        let dir = tempfile::tempdir().unwrap();
        morph_core::init_repo(dir.path()).unwrap();
        let server = MorphServer::new(Some(dir.path().to_path_buf()));
        (dir, server)
    }

    fn extract_text(result: &CallToolResult) -> &str {
        match &*result.content[0] {
            RawContent::Text(t) => &t.text,
            _ => panic!("expected text content"),
        }
    }

    #[tokio::test]
    async fn init_creates_morph_dir() {
        let dir = tempfile::tempdir().unwrap();
        let server = MorphServer::new(None);
        let path_str = dir.path().to_str().unwrap().to_string();
        let result = server
            .morph_init(Parameters(InitParams {
                path: Some(path_str),
            }))
            .await
            .unwrap();
        assert!(dir.path().join(".morph").is_dir());
        assert!(extract_text(&result).contains("Initialized"));
    }

    #[tokio::test]
    async fn init_already_initialized_fails() {
        let (dir, server) = setup_repo();
        let path_str = dir.path().to_str().unwrap().to_string();
        let result = server
            .morph_init(Parameters(InitParams {
                path: Some(path_str),
            }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn record_session_returns_hash() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        let result = server
            .morph_record_session(Parameters(RecordSessionParams {
                prompt: "What is 2+2?".into(),
                response: "4".into(),
                workspace_path: Some(ws),
                model_name: Some("test-model".into()),
                agent_id: Some("test-agent".into()),
            }))
            .await
            .unwrap();
        let text = extract_text(&result);
        assert_eq!(text.len(), 64, "expected 64-char hash, got: {}", text);
    }

    #[tokio::test]
    async fn stage_and_commit_workflow() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("hello.txt"), "hello world").unwrap();

        let stage_result = server
            .morph_stage(Parameters(StageParams {
                workspace_path: Some(ws.clone()),
                paths: Some(vec!["hello.txt".into()]),
            }))
            .await
            .unwrap();
        assert_eq!(extract_text(&stage_result).trim().len(), 64);

        let commit_result = server
            .morph_commit(Parameters(CommitParams {
                message: "first commit".into(),
                pipeline: None,
                eval_suite: None,
                workspace_path: Some(ws),
                metrics: None,
                author: Some("test-author".into()),
                from_run: None,
            }))
            .await
            .unwrap();
        assert_eq!(extract_text(&commit_result).len(), 64);
    }

    #[tokio::test]
    async fn branch_and_checkout() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("f.txt"), "content").unwrap();

        server
            .morph_stage(Parameters(StageParams {
                workspace_path: Some(ws.clone()),
                paths: Some(vec![".".into()]),
            }))
            .await
            .unwrap();
        server
            .morph_commit(Parameters(CommitParams {
                message: "initial".into(),
                pipeline: None,
                eval_suite: None,
                workspace_path: Some(ws.clone()),
                metrics: None,
                author: None,
                from_run: None,
            }))
            .await
            .unwrap();

        let branch_result = server
            .morph_branch(Parameters(BranchParams {
                name: "feature".into(),
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();
        assert!(extract_text(&branch_result).contains("Created branch feature"));

        let checkout_result = server
            .morph_checkout(Parameters(CheckoutParams {
                ref_name: "feature".into(),
                workspace_path: Some(ws),
            }))
            .await
            .unwrap();
        assert!(extract_text(&checkout_result).contains("Switched to branch feature"));
    }

    #[tokio::test]
    async fn branch_without_commit_fails() {
        let (_dir, server) = setup_repo();
        let ws = _dir.path().to_str().unwrap().to_string();
        let result = server
            .morph_branch(Parameters(BranchParams {
                name: "feature".into(),
                workspace_path: Some(ws),
            }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn annotate_creates_annotation() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();

        let session_result = server
            .morph_record_session(Parameters(RecordSessionParams {
                prompt: "test".into(),
                response: "response".into(),
                workspace_path: Some(ws.clone()),
                model_name: None,
                agent_id: None,
            }))
            .await
            .unwrap();
        let run_hash = extract_text(&session_result).to_string();

        let mut data = std::collections::BTreeMap::new();
        data.insert("note".to_string(), serde_json::json!("good session"));
        let ann_result = server
            .morph_annotate(Parameters(AnnotateParams {
                target_hash: run_hash,
                kind: "review".into(),
                data: Some(data),
                target_sub: None,
                workspace_path: Some(ws),
                author: Some("reviewer".into()),
            }))
            .await
            .unwrap();
        assert_eq!(extract_text(&ann_result).len(), 64);
    }

    #[tokio::test]
    async fn record_eval_from_file() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        let metrics_file = dir.path().join("metrics.json");
        std::fs::write(&metrics_file, r#"{"metrics": {"acc": 0.95}}"#).unwrap();

        let result = server
            .morph_record_eval(Parameters(RecordEvalParams {
                file: "metrics.json".into(),
                workspace_path: Some(ws),
            }))
            .await
            .unwrap();
        assert!(extract_text(&result).contains("0.95"));
    }

    #[tokio::test]
    async fn commit_with_metrics() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("code.py"), "print('hello')").unwrap();

        server
            .morph_stage(Parameters(StageParams {
                workspace_path: Some(ws.clone()),
                paths: Some(vec![".".into()]),
            }))
            .await
            .unwrap();

        let mut metrics = std::collections::BTreeMap::new();
        metrics.insert("tests_passed".to_string(), 42.0);
        metrics.insert("coverage".to_string(), 0.85);
        let result = server
            .morph_commit(Parameters(CommitParams {
                message: "commit with metrics".into(),
                pipeline: None,
                eval_suite: None,
                workspace_path: Some(ws),
                metrics: Some(metrics),
                author: Some("agent".into()),
                from_run: None,
            }))
            .await
            .unwrap();
        assert_eq!(extract_text(&result).len(), 64);
    }

    #[tokio::test]
    async fn commit_with_from_run_provenance() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("code.txt"), "fn main() {}").unwrap();

        let session_result = server
            .morph_record_session(Parameters(RecordSessionParams {
                prompt: "Build it".into(),
                response: "Built".into(),
                workspace_path: Some(ws.clone()),
                model_name: Some("gpt-4".into()),
                agent_id: Some("cursor-agent".into()),
            }))
            .await
            .unwrap();
        let run_hash = extract_text(&session_result).to_string();

        server
            .morph_stage(Parameters(StageParams {
                workspace_path: Some(ws.clone()),
                paths: Some(vec![".".into()]),
            }))
            .await
            .unwrap();

        let commit_result = server
            .morph_commit(Parameters(CommitParams {
                message: "evidence-backed commit".into(),
                pipeline: None,
                eval_suite: None,
                workspace_path: Some(ws),
                metrics: None,
                author: None,
                from_run: Some(run_hash),
            }))
            .await
            .unwrap();
        assert_eq!(extract_text(&commit_result).len(), 64);
    }

    #[tokio::test]
    async fn repo_store_not_found_gives_clear_error() {
        let server = MorphServer::new(Some(PathBuf::from("/tmp/nonexistent-morph-repo-xyz")));
        let result = server.repo_store(None);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.contains("not a morph repository"), "got: {}", err);
    }

    #[tokio::test]
    async fn stage_default_paths() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("a.txt"), "aaa").unwrap();
        std::fs::write(dir.path().join("b.txt"), "bbb").unwrap();

        let result = server
            .morph_stage(Parameters(StageParams {
                workspace_path: Some(ws),
                paths: None,
            }))
            .await
            .unwrap();
        let lines: Vec<&str> = extract_text(&result).trim().lines().collect();
        assert!(lines.len() >= 2, "expected at least 2 staged files");
    }

    #[tokio::test]
    async fn status_shows_files() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("code.py"), "print('hi')").unwrap();

        let result = server
            .morph_status(Parameters(WorkspaceOnlyParams {
                workspace_path: Some(ws),
            }))
            .await
            .unwrap();
        assert!(extract_text(&result).contains("code.py"));
    }

    #[tokio::test]
    async fn log_shows_commits() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("f.txt"), "data").unwrap();

        server
            .morph_stage(Parameters(StageParams {
                workspace_path: Some(ws.clone()),
                paths: Some(vec![".".into()]),
            }))
            .await
            .unwrap();
        server
            .morph_commit(Parameters(CommitParams {
                message: "test-commit-msg".into(),
                pipeline: None,
                eval_suite: None,
                workspace_path: Some(ws.clone()),
                metrics: None,
                author: None,
                from_run: None,
            }))
            .await
            .unwrap();

        let result = server
            .morph_log(Parameters(LogParams {
                ref_name: None,
                workspace_path: Some(ws),
            }))
            .await
            .unwrap();
        assert!(extract_text(&result).contains("test-commit-msg"));
    }

    #[tokio::test]
    async fn show_returns_json() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("x.txt"), "hello").unwrap();

        server
            .morph_stage(Parameters(StageParams {
                workspace_path: Some(ws.clone()),
                paths: Some(vec![".".into()]),
            }))
            .await
            .unwrap();
        let commit_result = server
            .morph_commit(Parameters(CommitParams {
                message: "initial".into(),
                pipeline: None,
                eval_suite: None,
                workspace_path: Some(ws.clone()),
                metrics: None,
                author: None,
                from_run: None,
            }))
            .await
            .unwrap();
        let commit_hash = extract_text(&commit_result).to_string();

        let result = server
            .morph_show(Parameters(ShowParams {
                hash: commit_hash,
                workspace_path: Some(ws),
            }))
            .await
            .unwrap();
        let text = extract_text(&result);
        assert!(text.contains("commit"));
        assert!(text.contains("initial"));
    }

    #[tokio::test]
    async fn diff_between_commits() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("a.txt"), "v1").unwrap();

        server
            .morph_stage(Parameters(StageParams {
                workspace_path: Some(ws.clone()),
                paths: Some(vec![".".into()]),
            }))
            .await
            .unwrap();
        let c1_result = server
            .morph_commit(Parameters(CommitParams {
                message: "first".into(),
                pipeline: None,
                eval_suite: None,
                workspace_path: Some(ws.clone()),
                metrics: None,
                author: None,
                from_run: None,
            }))
            .await
            .unwrap();
        let c1 = extract_text(&c1_result).to_string();

        std::fs::write(dir.path().join("b.txt"), "new file").unwrap();
        server
            .morph_stage(Parameters(StageParams {
                workspace_path: Some(ws.clone()),
                paths: Some(vec!["b.txt".into()]),
            }))
            .await
            .unwrap();
        let c2_result = server
            .morph_commit(Parameters(CommitParams {
                message: "second".into(),
                pipeline: None,
                eval_suite: None,
                workspace_path: Some(ws.clone()),
                metrics: None,
                author: None,
                from_run: None,
            }))
            .await
            .unwrap();
        let c2 = extract_text(&c2_result).to_string();

        let result = server
            .morph_diff(Parameters(DiffParams {
                old_ref: c1,
                new_ref: c2,
                workspace_path: Some(ws),
            }))
            .await
            .unwrap();
        let text = extract_text(&result);
        assert!(text.contains("A"), "expected 'A' for added file");
        assert!(text.contains("b.txt"));
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
