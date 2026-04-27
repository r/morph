//! Cursor MCP server: primary write path from the IDE.
//! Exposes morph-core operations as MCP tools.

mod params;

use morph_core::{
    find_repo, open_store, read_repo_version, require_store_version,
    Hash, Store, STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_0_4, STORE_VERSION_0_5, STORE_VERSION_INIT,
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
        require_store_version(&morph_dir, &[STORE_VERSION_INIT, STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_0_4, STORE_VERSION_0_5]).map_err(|e| e.to_string())?;
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
                .map(|m| morph_core::ConversationMessage {
                    role: m.role.clone(),
                    content: m.content.clone(),
                    metadata: m.metadata.clone().unwrap_or_default(),
                    timestamp: m.timestamp.clone(),
                })
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

    #[tool(description = "Show changes relative to last commit (git-style status) and accumulated Morph activity")]
    async fn morph_status(&self, params: Parameters<WorkspaceOnlyParams>) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let changes = morph_core::working_status(&store, &repo_root).map_err(mcp_err)?;
        let summary = morph_core::activity_summary(&store, &repo_root).map_err(mcp_err)?;
        let mut out = String::new();

        if changes.is_empty() && summary.runs == 0 && summary.traces == 0 && summary.prompts == 0 {
            out = "nothing to commit, working tree clean".into();
        } else {
            if !changes.is_empty() {
                out.push_str("Changes not staged for commit:\n\n");
                for entry in &changes {
                    let tag = match entry.status {
                        morph_core::DiffStatus::Added => "new file",
                        morph_core::DiffStatus::Modified => "modified",
                        morph_core::DiffStatus::Deleted => "deleted",
                    };
                    out.push_str(&format!("\t{:>12}:   {}\n", tag, entry.path));
                }
                out.push('\n');
            }
            if summary.runs > 0 || summary.traces > 0 || summary.prompts > 0 {
                let mut parts = Vec::new();
                if summary.runs > 0 { parts.push(format!("{} run{}", summary.runs, if summary.runs == 1 { "" } else { "s" })); }
                if summary.traces > 0 { parts.push(format!("{} trace{}", summary.traces, if summary.traces == 1 { "" } else { "s" })); }
                if summary.prompts > 0 { parts.push(format!("{} prompt{}", summary.prompts, if summary.prompts == 1 { "" } else { "s" })); }
                out.push_str(&format!("Morph activity: {}\n", parts.join(", ")));
            }
        }
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

    fn resolve_run_hash(&self, store: &dyn Store, hash_str: &str) -> Result<Hash, McpError> {
        let h = Hash::from_hex(hash_str).map_err(mcp_err)?;
        match store.get(&h).map_err(mcp_err)? {
            morph_core::MorphObject::Run(_) => Ok(h),
            morph_core::MorphObject::Trace(_) => morph_core::find_run_by_trace(store, &h)
                .map_err(mcp_err)?
                .ok_or_else(|| mcp_err(format!("no run points to trace {}", hash_str))),
            _ => Err(mcp_err(format!("hash {} is neither a Run nor a Trace", hash_str))),
        }
    }

    #[tool(
        description = "Use this when you need to browse recent Morph traces and find relevant coding sessions. Returns a compact JSON array of trace summaries (run hash, timestamp, prompt preview, task_phase, task_scope, target_files, target_symbols), newest first. Use the returned run hashes to drill in with morph_get_trace_task_structure / morph_get_trace_target_context / morph_get_trace_final_artifact."
    )]
    async fn morph_get_recent_trace_summaries(
        &self,
        params: Parameters<RecentTracesParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let limit = params.0.limit.unwrap_or(10);
        let summaries = morph_core::recent_trace_summaries(store.as_ref(), limit).map_err(mcp_err)?;
        let json = serde_json::to_string(&summaries).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    #[tool(
        description = "Use this when generating a replay prompt or eval case and you need to know what kind of coding task the trace contains. Returns the structured task: task_phase (create_code/modify_code/fix_bug/diagnose/verify/explain), task_scope (single_function/single_file/multi_file/broad), final_artifact_type, target_files, target_symbols, task_goal, and verification_actions. Accepts either a run hash or a trace hash."
    )]
    async fn morph_get_trace_task_structure(
        &self,
        params: Parameters<TraceHashParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let run_hash = self.resolve_run_hash(store.as_ref(), &params.0.hash)?;
        let out = morph_core::task_structure(store.as_ref(), &run_hash).map_err(mcp_err)?;
        let json = serde_json::to_string(&out).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    #[tool(
        description = "Use this when you need the relevant file/function context for replay or eval generation instead of guessing from raw events. Returns target_file, target_symbol, language, scope classification, and a scoped code snippet (the function body when task_scope == single_function, otherwise the file content). Accepts either a run hash or a trace hash."
    )]
    async fn morph_get_trace_target_context(
        &self,
        params: Parameters<TraceHashParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let run_hash = self.resolve_run_hash(store.as_ref(), &params.0.hash)?;
        let out = morph_core::target_context(store.as_ref(), &run_hash).map_err(mcp_err)?;
        let json = serde_json::to_string(&out).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    #[tool(
        description = "Use this when you need the final edited function, file snippet, or artifact produced for a trace. Returns artifact_type plus one of final_function_text (when artifact_type == function_only), final_file_snippet (full_file/tool_execution), or final_patch_summary (unified_diff), along with related file and symbol. Accepts either a run hash or a trace hash."
    )]
    async fn morph_get_trace_final_artifact(
        &self,
        params: Parameters<TraceHashParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let run_hash = self.resolve_run_hash(store.as_ref(), &params.0.hash)?;
        let out = morph_core::final_artifact(store.as_ref(), &run_hash).map_err(mcp_err)?;
        let json = serde_json::to_string(&out).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    #[tool(
        description = "Use this when you need to understand what changed, what should remain, and what behavior was restored or modified. Returns changed_construct_summary, preserved_construct_summary, restored_behavior_summary. Best-effort heuristic summaries derived from prompt + response. Accepts either a run hash or a trace hash."
    )]
    async fn morph_get_trace_change_semantics(
        &self,
        params: Parameters<TraceHashParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let run_hash = self.resolve_run_hash(store.as_ref(), &params.0.hash)?;
        let out = morph_core::change_semantics(store.as_ref(), &run_hash).map_err(mcp_err)?;
        let json = serde_json::to_string(&out).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    #[tool(
        description = "Use this when you need commands/tests/demo steps that were used to verify a coding change. Returns a list of verification_actions with { action, kind ('command'|'text'|'tool'), optional exit_status, optional summary_output }. Replay systems should preserve task_goal while optionally dropping these. Accepts either a run hash or a trace hash."
    )]
    async fn morph_get_trace_verification_steps(
        &self,
        params: Parameters<TraceHashParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let run_hash = self.resolve_run_hash(store.as_ref(), &params.0.hash)?;
        let out = morph_core::verification_steps(store.as_ref(), &run_hash).map_err(mcp_err)?;
        let json = serde_json::to_string(&out).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
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
                    params::MessageParam { role: "user".into(), content: "Build a server".into(), metadata: None, timestamp: None },
                    params::MessageParam { role: "assistant".into(), content: "Creating files".into(), metadata: None, timestamp: None },
                    params::MessageParam { role: "tool_call".into(), content: "write_file(app.py)".into(), metadata: None, timestamp: None },
                    params::MessageParam { role: "tool_result".into(), content: "done".into(), metadata: None, timestamp: None },
                    params::MessageParam { role: "assistant".into(), content: "Server is ready!".into(), metadata: None, timestamp: None },
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
    async fn repo_store_accepts_latest_store_version_after_upgrade() {
        // Cycle 35: MCP must accept the new latest version 0.5 (after
        // a full upgrade chain). Was 0.4 pre-PR4.
        let dir = tempfile::tempdir().unwrap();
        morph_core::init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        morph_core::migrate_0_0_to_0_2(&morph_dir).unwrap();
        morph_core::migrate_0_2_to_0_3(&morph_dir).unwrap();
        morph_core::migrate_0_3_to_0_4(&morph_dir).unwrap();
        morph_core::migrate_0_4_to_0_5(&morph_dir).unwrap();
        let version = morph_core::read_repo_version(&morph_dir).unwrap();
        assert_eq!(version, STORE_VERSION_0_5);

        let server = MorphServer::new(Some(dir.path().to_path_buf()));
        let result = server.repo_store(None);
        assert!(result.is_ok(), "MCP should accept store version 0.5; got: {:?}", result.err());
    }

    #[tokio::test]
    async fn repo_store_still_accepts_legacy_version_0_4() {
        // Cycle 36: MCP must keep accepting 0.4 repos so users with
        // older `.morph/` directories aren't locked out before they
        // upgrade. The version gate is "any known version up to 0.5".
        let dir = tempfile::tempdir().unwrap();
        morph_core::init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        morph_core::migrate_0_0_to_0_2(&morph_dir).unwrap();
        morph_core::migrate_0_2_to_0_3(&morph_dir).unwrap();
        morph_core::migrate_0_3_to_0_4(&morph_dir).unwrap();
        let version = morph_core::read_repo_version(&morph_dir).unwrap();
        assert_eq!(version, STORE_VERSION_0_4);

        let server = MorphServer::new(Some(dir.path().to_path_buf()));
        let result = server.repo_store(None);
        assert!(
            result.is_ok(),
            "MCP must continue to accept store version 0.4 after PR4; got: {:?}",
            result.err()
        );
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

    fn build_pocket_tasks_fix_trace(store: &dyn Store) -> Hash {
        use morph_core::objects::*;
        let mut events = Vec::new();
        let mut add_event = |seq: u64, kind: &str, payload: std::collections::BTreeMap<String, serde_json::Value>| {
            events.push(TraceEvent {
                id: format!("evt_{}", seq),
                seq,
                ts: format!("2026-04-17T12:00:{:02}+00:00", seq),
                kind: kind.into(),
                payload,
            });
        };
        let mut p0 = std::collections::BTreeMap::new();
        p0.insert("text".into(), serde_json::json!("Fix the bug in `list_tasks` in `pocket_tasks/main.py` — it should not skip the first task."));
        add_event(0, "user", p0);

        let mut p1 = std::collections::BTreeMap::new();
        p1.insert(
            "text".into(),
            serde_json::json!(
                "Fixed. The function now returns all rows:\n\n```python\ndef list_tasks(db):\n    rows = db.fetch('tasks')\n    return [r.title for r in rows]\n```\n\nRun `pytest pocket_tasks/` to verify."
            ),
        );
        add_event(1, "assistant", p1);

        let mut p2 = std::collections::BTreeMap::new();
        p2.insert("path".into(), serde_json::json!("pocket_tasks/main.py"));
        p2.insert(
            "content".into(),
            serde_json::json!(
                "def list_tasks(db):\n    rows = db.fetch('tasks')\n    return [r.title for r in rows[1:]]\n"
            ),
        );
        add_event(2, "file_read", p2);

        let mut p3 = std::collections::BTreeMap::new();
        p3.insert("path".into(), serde_json::json!("pocket_tasks/main.py"));
        p3.insert(
            "content".into(),
            serde_json::json!(
                "def list_tasks(db):\n    rows = db.fetch('tasks')\n    return [r.title for r in rows]\n"
            ),
        );
        add_event(3, "file_edit", p3);

        let mut p4 = std::collections::BTreeMap::new();
        p4.insert("name".into(), serde_json::json!("shell"));
        p4.insert("input".into(), serde_json::json!("pytest pocket_tasks/"));
        add_event(4, "tool_call", p4);

        let mut p5 = std::collections::BTreeMap::new();
        p5.insert("output".into(), serde_json::json!("3 passed"));
        add_event(5, "tool_result", p5);

        let trace = morph_core::MorphObject::Trace(Trace { events });
        let trace_hash = store.put(&trace).unwrap();
        let pipeline_hash = store.put(&morph_core::identity_pipeline()).unwrap();
        let run = morph_core::MorphObject::Run(Run {
            pipeline: pipeline_hash.to_string(),
            commit: None,
            environment: RunEnvironment {
                model: "gpt-4".into(),
                version: "1.0".into(),
                parameters: std::collections::BTreeMap::new(),
                toolchain: std::collections::BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: std::collections::BTreeMap::new(),
            trace: trace_hash.to_string(),
            agent: AgentInfo { id: "cursor".into(), version: "1.0".into(), policy: None, instance_id: None },
            contributors: None,
            morph_version: None,
        });
        store.put(&run).unwrap()
    }

    #[tokio::test]
    async fn structured_mcp_tools_end_to_end() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        let store = morph_core::open_store(&dir.path().join(".morph")).unwrap();
        let run_hash = build_pocket_tasks_fix_trace(store.as_ref());
        let run_hex = run_hash.to_string();

        let summaries = server
            .morph_get_recent_trace_summaries(Parameters(RecentTracesParams {
                limit: Some(5),
                workspace_path: Some(ws.clone()),
            })).await.unwrap();
        let text = extract_text(&summaries);
        assert!(text.contains("list_tasks"), "summaries: {}", text);
        assert!(text.contains("fix_bug"), "summaries should include task_phase fix_bug: {}", text);

        let structure = server
            .morph_get_trace_task_structure(Parameters(TraceHashParams {
                hash: run_hex.clone(),
                workspace_path: Some(ws.clone()),
            })).await.unwrap();
        let structure_text = extract_text(&structure);
        assert!(structure_text.contains("\"task_phase\":\"fix_bug\""));
        assert!(structure_text.contains("\"task_scope\":\"single_function\""));
        assert!(structure_text.contains("pocket_tasks/main.py"));

        let context = server
            .morph_get_trace_target_context(Parameters(TraceHashParams {
                hash: run_hex.clone(),
                workspace_path: Some(ws.clone()),
            })).await.unwrap();
        let context_text = extract_text(&context);
        assert!(context_text.contains("def list_tasks(db):"), "got {}", context_text);
        assert!(context_text.contains("\"language\":\"python\""));

        let artifact = server
            .morph_get_trace_final_artifact(Parameters(TraceHashParams {
                hash: run_hex.clone(),
                workspace_path: Some(ws.clone()),
            })).await.unwrap();
        let artifact_text = extract_text(&artifact);
        assert!(artifact_text.contains("\"artifact_type\":\"function_only\""));
        assert!(artifact_text.contains("final_function_text"));

        let semantics = server
            .morph_get_trace_change_semantics(Parameters(TraceHashParams {
                hash: run_hex.clone(),
                workspace_path: Some(ws.clone()),
            })).await.unwrap();
        assert!(extract_text(&semantics).contains("changed_construct_summary"));

        let verify = server
            .morph_get_trace_verification_steps(Parameters(TraceHashParams {
                hash: run_hex,
                workspace_path: Some(ws),
            })).await.unwrap();
        let verify_text = extract_text(&verify);
        assert!(verify_text.contains("pytest pocket_tasks/"), "got {}", verify_text);
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
