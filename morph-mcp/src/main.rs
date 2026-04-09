//! Cursor MCP server: primary write path from the IDE.
//! Exposes morph-core operations as MCP tools.

mod params;

use morph_core::{
    find_repo, open_store, read_repo_version, require_store_version,
    Hash, Store, STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_INIT,
};
use params::*;
use rmcp::{
    handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::*,
    tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError,
    ServerHandler, ServiceExt,
};
use std::path::PathBuf;

fn default_workspace_from_env_and_args(args: &[String]) -> Option<PathBuf> {
    std::env::var_os("MORPH_WORKSPACE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CURSOR_WORKSPACE_FOLDER").map(PathBuf::from))
        .or_else(|| std::env::var_os("WORKSPACE_FOLDER").map(PathBuf::from))
        .or_else(|| args.get(1).filter(|a| !a.starts_with('-')).map(PathBuf::from))
}

fn mcp_err(e: impl ToString) -> McpError {
    McpError::invalid_params(e.to_string(), None)
}

fn resolve_path(repo_root: &std::path::Path, p: impl Into<PathBuf>) -> PathBuf {
    let pb: PathBuf = p.into();
    if pb.is_absolute() { pb } else { repo_root.join(pb) }
}

#[derive(Clone)]
pub struct MorphServer {
    tool_router: ToolRouter<Self>,
    default_workspace: Option<PathBuf>,
}

#[tool_router]
impl MorphServer {
    fn new(default_workspace: Option<PathBuf>) -> Self {
        Self { tool_router: Self::tool_router(), default_workspace }
    }

    fn repo_store(&self, workspace_path: Option<&str>) -> Result<(PathBuf, Box<dyn Store>), String> {
        let start = workspace_path.map(PathBuf::from)
            .or_else(|| self.default_workspace.clone())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let repo_root = find_repo(&start).ok_or_else(|| {
            let tried = start.canonicalize().unwrap_or(start).display().to_string();
            format!(
                "not a morph repository (no .morph/ found from {}). \
                Fix: (1) run 'morph init', (2) set MORPH_WORKSPACE in .cursor/mcp.json, or (3) pass workspace_path.",
                tried
            )
        })?;
        let morph_dir = repo_root.join(".morph");
        require_store_version(&morph_dir, &[STORE_VERSION_INIT, STORE_VERSION_0_2, STORE_VERSION_0_3]).map_err(|e| e.to_string())?;
        let store = open_store(&morph_dir).map_err(|e| e.to_string())?;
        Ok((repo_root, store))
    }

    #[tool(description = "Initialize a Morph repository in the given path (default: current directory)")]
    async fn morph_init(&self, params: Parameters<InitParams>) -> Result<CallToolResult, McpError> {
        let path = PathBuf::from(params.0.path.as_deref().unwrap_or("."));
        morph_core::init_repo(&path).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(format!("Initialized Morph repository in {}", path.display()))]))
    }

    #[tool(description = "Record a Run object from JSON (execution receipt). Optional: trace_path, artifact_paths as JSON array.")]
    async fn morph_record_run(&self, params: Parameters<RecordRunParams>) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let run_path = resolve_path(&repo_root, &params.0.run_file);
        let trace_path = params.0.trace_file.map(|p| resolve_path(&repo_root, p));
        let artifact_paths: Vec<PathBuf> = params.0.artifact_files.unwrap_or_default()
            .into_iter().map(|p| resolve_path(&repo_root, p)).collect();
        let refs: Vec<&std::path::Path> = artifact_paths.iter().map(PathBuf::as_path).collect();
        let hash = morph_core::record_run(&store, &run_path, trace_path.as_deref(), &refs).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(hash.to_string())]))
    }

    #[tool(description = "Internal — used by the Morph recording plugin. Do NOT call this tool yourself; session recording is automatic. If called manually the result is not useful to the user.")]
    async fn morph_record_session(&self, params: Parameters<RecordSessionParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        if let Some(ref msgs) = params.0.messages {
            let conversation: Vec<morph_core::ConversationMessage> = msgs.iter()
                .map(|m| morph_core::ConversationMessage { role: m.role.clone(), content: m.content.clone() })
                .collect();
            morph_core::record_conversation(
                &store, &conversation,
                params.0.model_name.as_deref(), params.0.agent_id.as_deref(),
            ).map_err(mcp_err)?;
        } else {
            morph_core::record_session(
                &store,
                params.0.prompt.as_deref().unwrap_or(""),
                params.0.response.as_deref().unwrap_or(""),
                params.0.model_name.as_deref(), params.0.agent_id.as_deref(),
            ).map_err(mcp_err)?;
        }
        Ok(CallToolResult::success(vec![Content::text("Session recorded.")]))
    }

    #[tool(description = "Record evaluation metrics from a JSON file with a 'metrics' key")]
    async fn morph_record_eval(&self, params: Parameters<RecordEvalParams>) -> Result<CallToolResult, McpError> {
        let (repo_root, _store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let path = resolve_path(&repo_root, &params.0.file);
        let metrics = morph_core::record_eval_metrics(&path).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(serde_json::to_string_pretty(&metrics).unwrap_or_default())]))
    }

    #[tool(description = "Stage paths into the object store (paths: array of paths, default [\".\"])")]
    async fn morph_stage(&self, params: Parameters<StageParams>) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let paths: Vec<PathBuf> = params.0.paths.unwrap_or_else(|| vec![".".into()])
            .into_iter().map(PathBuf::from).collect();
        let hashes = morph_core::add_paths(&store, &repo_root, &paths).map_err(mcp_err)?;
        let out = hashes.iter().map(|h| h.to_string()).collect::<Vec<_>>().join("\n");
        Ok(CallToolResult::success(vec![Content::text(out)]))
    }

    #[tool(description = "Create a commit. Required: message. Optional: pipeline (hash), eval_suite (hash), metrics (JSON object), author, from_run (run hash for evidence-backed provenance).")]
    async fn morph_commit(&self, params: Parameters<CommitParams>) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let morph_dir = repo_root.join(".morph");
        let version = read_repo_version(&morph_dir).map_err(mcp_err)?;
        let prog_hash = params.0.pipeline.as_deref().map(Hash::from_hex).transpose().map_err(mcp_err)?;
        let suite_hash = params.0.eval_suite.as_deref().map(Hash::from_hex).transpose().map_err(mcp_err)?;
        let metrics = params.0.metrics.unwrap_or_default();
        let provenance = match params.0.from_run {
            Some(ref h) => Some(morph_core::resolve_provenance_from_run(&store, &Hash::from_hex(h).map_err(mcp_err)?).map_err(mcp_err)?),
            None => None,
        };
        let hash = morph_core::create_tree_commit_with_provenance(
            &store, &repo_root, prog_hash.as_ref(), suite_hash.as_ref(),
            metrics, params.0.message, params.0.author, Some(&version), provenance.as_ref(),
        ).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(hash.to_string())]))
    }

    #[tool(description = "Attach an annotation to an object. Required: target_hash, kind, data (JSON object). Optional: target_sub, author.")]
    async fn morph_annotate(&self, params: Parameters<AnnotateParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let target = Hash::from_hex(&params.0.target_hash).map_err(mcp_err)?;
        let ann = morph_core::create_annotation(&target, params.0.target_sub, params.0.kind, params.0.data.unwrap_or_default(), params.0.author);
        let hash = store.put(&ann).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(hash.to_string())]))
    }

    #[tool(description = "Create a new branch at current HEAD")]
    async fn morph_branch(&self, params: Parameters<BranchParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let head = morph_core::resolve_head(&store).map_err(mcp_err)?
            .ok_or_else(|| mcp_err("no commit yet"))?;
        store.ref_write(&format!("heads/{}", params.0.name), &head).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(format!("Created branch {}", params.0.name))]))
    }

    #[tool(description = "Switch HEAD to a branch or detached commit")]
    async fn morph_checkout(&self, params: Parameters<CheckoutParams>) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let ref_name = &params.0.ref_name;
        let (hash, tree_restored) = morph_core::checkout_tree(&store, &repo_root, ref_name).map_err(mcp_err)?;
        let msg = if ref_name.len() == 64 && ref_name.chars().all(|c| c.is_ascii_hexdigit()) {
            format!("Detached HEAD at {}", hash)
        } else {
            format!("Switched to branch {}", ref_name.trim_start_matches("heads/"))
        };
        let tree_msg = if tree_restored { " (working tree restored)" } else { " (no file tree in commit)" };
        Ok(CallToolResult::success(vec![Content::text(format!("{}{}", msg, tree_msg))]))
    }

    #[tool(description = "Show working-space status")]
    async fn morph_status(&self, params: Parameters<WorkspaceOnlyParams>) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let entries = morph_core::status(&store, &repo_root).map_err(mcp_err)?;
        let mut out = String::new();
        for e in &entries {
            let status = if e.in_store { "tracked" } else { "new" };
            let hash_str = e.hash.as_ref().map(|h| h.to_string()).unwrap_or_default();
            out.push_str(&format!("{} {} {}\n", status, hash_str, e.path.display()));
        }
        if out.is_empty() { out = "clean (no changes)".into(); }
        Ok(CallToolResult::success(vec![Content::text(out)]))
    }

    #[tool(description = "Show commit history from HEAD or a named ref")]
    async fn morph_log(&self, params: Parameters<LogParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let ref_name = params.0.ref_name.as_deref().unwrap_or("HEAD");
        let hashes = morph_core::log_from(&store, ref_name).map_err(mcp_err)?;
        let mut out = String::new();
        for h in &hashes {
            if let morph_core::MorphObject::Commit(c) = store.get(h).map_err(mcp_err)? {
                out.push_str(&format!("{} {}\n", h, c.message));
            }
        }
        Ok(CallToolResult::success(vec![Content::text(out)]))
    }

    #[tool(description = "Show a stored Morph object as pretty JSON")]
    async fn morph_show(&self, params: Parameters<ShowParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let hash = Hash::from_hex(&params.0.hash).map_err(mcp_err)?;
        let obj = store.get(&hash).map_err(mcp_err)?;
        let json = serde_json::to_string_pretty(&obj).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    #[tool(description = "Diff two commits (or refs) and show file-level changes")]
    async fn morph_diff(&self, params: Parameters<DiffParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let resolve = |r: &str| -> Result<Hash, McpError> {
            if r.len() == 64 && r.chars().all(|c| c.is_ascii_hexdigit()) {
                Hash::from_hex(r).map_err(mcp_err)
            } else if r == "HEAD" {
                morph_core::resolve_head(&store).map_err(mcp_err)?
                    .ok_or_else(|| mcp_err("HEAD has no commits"))
            } else {
                store.ref_read(&format!("heads/{}", r)).map_err(mcp_err)?
                    .ok_or_else(|| mcp_err(format!("unknown ref: {}", r)))
            }
        };
        let old_hash = resolve(&params.0.old_ref)?;
        let new_hash = resolve(&params.0.new_ref)?;
        let entries = morph_core::diff_commits(&store, &old_hash, &new_hash).map_err(mcp_err)?;
        let mut out = String::new();
        for e in &entries { out.push_str(&format!("{}  {}\n", e.status, e.path)); }
        if out.is_empty() { out = "no changes".into(); }
        Ok(CallToolResult::success(vec![Content::text(out)]))
    }

    #[tool(description = "Merge a branch into the current branch (requires behavioral dominance)")]
    async fn morph_merge(&self, params: Parameters<MergeParams>) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let pipeline = Hash::from_hex(&params.0.pipeline).map_err(mcp_err)?;
        let suite = params.0.eval_suite.as_deref().map(Hash::from_hex).transpose().map_err(mcp_err)?;
        let retired: Option<Vec<String>> = params.0.retire.as_deref()
            .map(|s| s.split(',').map(|r| r.trim().to_string()).collect());
        let morph_dir = repo_root.join(".morph");
        let version = morph_core::read_repo_version(&morph_dir).map_err(mcp_err)?;
        let plan = morph_core::prepare_merge(&store, &params.0.branch, suite.as_ref(), retired.as_deref()).map_err(mcp_err)?;
        let hash = morph_core::execute_merge(&store, &plan, &pipeline, params.0.metrics, params.0.message, params.0.author, Some(&repo_root), Some(&version)).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(hash.to_string())]))
    }
}

#[tool_handler]
impl ServerHandler for MorphServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: rmcp::model::Implementation {
                name: "morph-mcp".into(),
                version: concat!(env!("CARGO_PKG_VERSION"), " (built ", env!("MORPH_BUILD_DATE"), ")").into(),
                ..Default::default()
            },
            instructions: Some(
                "Morph VCS write path: init repos, record runs and evals, stage, commit, annotate, branch, checkout.".into(),
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
        let result = server
            .morph_init(Parameters(InitParams { path: Some(dir.path().to_str().unwrap().into()) }))
            .await.unwrap();
        assert!(dir.path().join(".morph").is_dir());
        assert!(extract_text(&result).contains("Initialized"));
    }

    #[tokio::test]
    async fn init_already_initialized_fails() {
        let (dir, server) = setup_repo();
        let result = server
            .morph_init(Parameters(InitParams { path: Some(dir.path().to_str().unwrap().into()) }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn record_session_returns_confirmation() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        let result = server
            .morph_record_session(Parameters(RecordSessionParams {
                prompt: Some("What is 2+2?".into()), response: Some("4".into()), messages: None,
                workspace_path: Some(ws.clone()), model_name: Some("test-model".into()), agent_id: Some("test-agent".into()),
            })).await.unwrap();
        assert!(extract_text(&result).contains("recorded"), "should return confirmation: {}", extract_text(&result));
        let store = morph_core::open_store(&std::path::Path::new(&ws).join(".morph")).unwrap();
        let runs = store.list(morph_core::ObjectType::Run).unwrap();
        assert_eq!(runs.len(), 1, "should have stored exactly one run");
    }

    #[tokio::test]
    async fn record_session_with_messages() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        let result = server
            .morph_record_session(Parameters(RecordSessionParams {
                prompt: None, response: None,
                messages: Some(vec![
                    params::MessageParam { role: "user".into(), content: "Build a server".into() },
                    params::MessageParam { role: "assistant".into(), content: "Creating files".into() },
                    params::MessageParam { role: "tool_call".into(), content: "write_file(app.py)".into() },
                    params::MessageParam { role: "tool_result".into(), content: "done".into() },
                    params::MessageParam { role: "assistant".into(), content: "Server is ready!".into() },
                ]),
                workspace_path: Some(ws.clone()), model_name: Some("qwen-3.5".into()), agent_id: Some("opencode".into()),
            })).await.unwrap();
        assert!(extract_text(&result).contains("recorded"));
        let store = morph_core::open_store(&std::path::Path::new(&ws).join(".morph")).unwrap();
        let runs = store.list(morph_core::ObjectType::Run).unwrap();
        assert_eq!(runs.len(), 1);
        let run_obj = store.get(&runs[0]).unwrap();
        if let morph_core::MorphObject::Run(run) = run_obj {
            let trace_hash = morph_core::Hash::from_hex(&run.trace).unwrap();
            if let morph_core::MorphObject::Trace(trace) = store.get(&trace_hash).unwrap() {
                assert_eq!(trace.events.len(), 5, "trace should have 5 events");
                assert_eq!(trace.events[0].kind, "user");
                assert_eq!(trace.events[2].kind, "tool_call");
            } else { panic!("expected Trace"); }
        } else { panic!("expected Run"); }
    }

    #[tokio::test]
    async fn stage_and_commit_workflow() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("hello.txt"), "hello world").unwrap();
        let stage_result = server
            .morph_stage(Parameters(StageParams { workspace_path: Some(ws.clone()), paths: Some(vec!["hello.txt".into()]) }))
            .await.unwrap();
        assert_eq!(extract_text(&stage_result).trim().len(), 64);
        let commit_result = server
            .morph_commit(Parameters(CommitParams {
                message: "first commit".into(), pipeline: None, eval_suite: None,
                workspace_path: Some(ws), metrics: None, author: Some("test-author".into()), from_run: None,
            })).await.unwrap();
        assert_eq!(extract_text(&commit_result).len(), 64);
    }

    #[tokio::test]
    async fn branch_and_checkout() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("f.txt"), "content").unwrap();
        server.morph_stage(Parameters(StageParams { workspace_path: Some(ws.clone()), paths: Some(vec![".".into()]) })).await.unwrap();
        server.morph_commit(Parameters(CommitParams { message: "initial".into(), pipeline: None, eval_suite: None, workspace_path: Some(ws.clone()), metrics: None, author: None, from_run: None })).await.unwrap();
        let branch_result = server.morph_branch(Parameters(BranchParams { name: "feature".into(), workspace_path: Some(ws.clone()) })).await.unwrap();
        assert!(extract_text(&branch_result).contains("Created branch feature"));
        let checkout_result = server.morph_checkout(Parameters(CheckoutParams { ref_name: "feature".into(), workspace_path: Some(ws) })).await.unwrap();
        assert!(extract_text(&checkout_result).contains("Switched to branch feature"));
    }

    #[tokio::test]
    async fn branch_without_commit_fails() {
        let (_dir, server) = setup_repo();
        let ws = _dir.path().to_str().unwrap().to_string();
        let result = server.morph_branch(Parameters(BranchParams { name: "feature".into(), workspace_path: Some(ws) })).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn annotate_creates_annotation() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        server
            .morph_record_session(Parameters(RecordSessionParams {
                prompt: Some("test".into()), response: Some("response".into()), messages: None,
                workspace_path: Some(ws.clone()), model_name: None, agent_id: None,
            })).await.unwrap();
        let store = morph_core::open_store(&dir.path().join(".morph")).unwrap();
        let run_hash = store.list(morph_core::ObjectType::Run).unwrap().into_iter().next().unwrap().to_string();
        let mut data = std::collections::BTreeMap::new();
        data.insert("note".to_string(), serde_json::json!("good session"));
        let ann_result = server
            .morph_annotate(Parameters(AnnotateParams {
                target_hash: run_hash, kind: "review".into(), data: Some(data),
                target_sub: None, workspace_path: Some(ws), author: Some("reviewer".into()),
            })).await.unwrap();
        assert_eq!(extract_text(&ann_result).len(), 64);
    }

    #[tokio::test]
    async fn record_eval_from_file() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("metrics.json"), r#"{"metrics": {"acc": 0.95}}"#).unwrap();
        let result = server.morph_record_eval(Parameters(RecordEvalParams { file: "metrics.json".into(), workspace_path: Some(ws) })).await.unwrap();
        assert!(extract_text(&result).contains("0.95"));
    }

    #[tokio::test]
    async fn commit_with_metrics() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("code.py"), "print('hello')").unwrap();
        server.morph_stage(Parameters(StageParams { workspace_path: Some(ws.clone()), paths: Some(vec![".".into()]) })).await.unwrap();
        let mut metrics = std::collections::BTreeMap::new();
        metrics.insert("tests_passed".to_string(), 42.0);
        metrics.insert("coverage".to_string(), 0.85);
        let result = server.morph_commit(Parameters(CommitParams {
            message: "commit with metrics".into(), pipeline: None, eval_suite: None,
            workspace_path: Some(ws), metrics: Some(metrics), author: Some("agent".into()), from_run: None,
        })).await.unwrap();
        assert_eq!(extract_text(&result).len(), 64);
    }

    #[tokio::test]
    async fn commit_with_from_run_provenance() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("code.txt"), "fn main() {}").unwrap();
        server
            .morph_record_session(Parameters(RecordSessionParams {
                prompt: Some("Build it".into()), response: Some("Built".into()), messages: None,
                workspace_path: Some(ws.clone()), model_name: Some("gpt-4".into()), agent_id: Some("cursor-agent".into()),
            })).await.unwrap();
        let store = morph_core::open_store(&dir.path().join(".morph")).unwrap();
        let run_hash = store.list(morph_core::ObjectType::Run).unwrap().into_iter().next().unwrap().to_string();
        server.morph_stage(Parameters(StageParams { workspace_path: Some(ws.clone()), paths: Some(vec![".".into()]) })).await.unwrap();
        let commit_result = server.morph_commit(Parameters(CommitParams {
            message: "evidence-backed commit".into(), pipeline: None, eval_suite: None,
            workspace_path: Some(ws), metrics: None, author: None, from_run: Some(run_hash),
        })).await.unwrap();
        assert_eq!(extract_text(&commit_result).len(), 64);
    }

    #[tokio::test]
    async fn repo_store_not_found_gives_clear_error() {
        let server = MorphServer::new(Some(PathBuf::from("/tmp/nonexistent-morph-repo-xyz")));
        let result = server.repo_store(None);
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("not a morph repository"));
    }

    #[tokio::test]
    async fn stage_default_paths() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("a.txt"), "aaa").unwrap();
        std::fs::write(dir.path().join("b.txt"), "bbb").unwrap();
        let result = server.morph_stage(Parameters(StageParams { workspace_path: Some(ws), paths: None })).await.unwrap();
        let lines: Vec<&str> = extract_text(&result).trim().lines().collect();
        assert!(lines.len() >= 2);
    }

    #[tokio::test]
    async fn status_shows_files() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("code.py"), "print('hi')").unwrap();
        let result = server.morph_status(Parameters(WorkspaceOnlyParams { workspace_path: Some(ws) })).await.unwrap();
        assert!(extract_text(&result).contains("code.py"));
    }

    #[tokio::test]
    async fn log_shows_commits() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("f.txt"), "data").unwrap();
        server.morph_stage(Parameters(StageParams { workspace_path: Some(ws.clone()), paths: Some(vec![".".into()]) })).await.unwrap();
        server.morph_commit(Parameters(CommitParams { message: "test-commit-msg".into(), pipeline: None, eval_suite: None, workspace_path: Some(ws.clone()), metrics: None, author: None, from_run: None })).await.unwrap();
        let result = server.morph_log(Parameters(LogParams { ref_name: None, workspace_path: Some(ws) })).await.unwrap();
        assert!(extract_text(&result).contains("test-commit-msg"));
    }

    #[tokio::test]
    async fn show_returns_json() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("x.txt"), "hello").unwrap();
        server.morph_stage(Parameters(StageParams { workspace_path: Some(ws.clone()), paths: Some(vec![".".into()]) })).await.unwrap();
        let commit_result = server.morph_commit(Parameters(CommitParams { message: "initial".into(), pipeline: None, eval_suite: None, workspace_path: Some(ws.clone()), metrics: None, author: None, from_run: None })).await.unwrap();
        let commit_hash = extract_text(&commit_result).to_string();
        let result = server.morph_show(Parameters(ShowParams { hash: commit_hash, workspace_path: Some(ws) })).await.unwrap();
        assert!(extract_text(&result).contains("commit"));
        assert!(extract_text(&result).contains("initial"));
    }

    #[tokio::test]
    async fn diff_between_commits() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
        server.morph_stage(Parameters(StageParams { workspace_path: Some(ws.clone()), paths: Some(vec![".".into()]) })).await.unwrap();
        let c1 = extract_text(&server.morph_commit(Parameters(CommitParams { message: "first".into(), pipeline: None, eval_suite: None, workspace_path: Some(ws.clone()), metrics: None, author: None, from_run: None })).await.unwrap()).to_string();
        std::fs::write(dir.path().join("b.txt"), "new file").unwrap();
        server.morph_stage(Parameters(StageParams { workspace_path: Some(ws.clone()), paths: Some(vec!["b.txt".into()]) })).await.unwrap();
        let c2 = extract_text(&server.morph_commit(Parameters(CommitParams { message: "second".into(), pipeline: None, eval_suite: None, workspace_path: Some(ws.clone()), metrics: None, author: None, from_run: None })).await.unwrap()).to_string();
        let result = server.morph_diff(Parameters(DiffParams { old_ref: c1, new_ref: c2, workspace_path: Some(ws) })).await.unwrap();
        assert!(extract_text(&result).contains("A"));
        assert!(extract_text(&result).contains("b.txt"));
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!(
            "morph-mcp {} (built {})\n\nMCP server for Morph. Cursor starts this process and talks over stdio.\n\
             Optional: set MORPH_WORKSPACE or pass it as first arg.\nVerify: morph-mcp --version",
            env!("CARGO_PKG_VERSION"),
            env!("MORPH_BUILD_DATE"),
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        eprintln!("morph-mcp {} (built {})", env!("CARGO_PKG_VERSION"), env!("MORPH_BUILD_DATE"));
        return Ok(());
    }
    let default_workspace = default_workspace_from_env_and_args(&args);
    let service = MorphServer::new(default_workspace).serve(stdio()).await
        .inspect_err(|e| eprintln!("morph-mcp error: {}", e))?;
    service.waiting().await?;
    Ok(())
}
