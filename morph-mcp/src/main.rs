//! Cursor MCP server: primary write path from the IDE.
//! Exposes morph-core operations as MCP tools.

mod params;

use morph_core::{
    build_status_json, find_repo, open_store, read_repo_version, require_store_version, resolve_revision,
    Hash, MorphObject, Store, STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_0_4, STORE_VERSION_0_5, STORE_VERSION_INIT,
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

/// Resolve a user-supplied identifier to a stored object hash.
/// Accepts full 64-char hashes, hex prefixes (>=4), `HEAD`, branch
/// names, tag names, or `refs/...` paths. Single source of truth so
/// every MCP tool accepts the same shapes the CLI does.
fn resolve_rev(store: &dyn Store, s: &str) -> Result<Hash, McpError> {
    resolve_revision(store, s).map_err(mcp_err)
}

fn short_hash(h: &str) -> String {
    h.chars().take(8).collect()
}

fn morph_object_type_str(obj: &MorphObject) -> &'static str {
    match obj {
        MorphObject::Blob(_) => "blob",
        MorphObject::Tree(_) => "tree",
        MorphObject::Pipeline(_) => "pipeline",
        MorphObject::Run(_) => "run",
        MorphObject::Trace(_) => "trace",
        MorphObject::Artifact(_) => "artifact",
        MorphObject::EvalSuite(_) => "eval_suite",
        MorphObject::Commit(_) => "commit",
        MorphObject::Annotation(_) => "annotation",
        MorphObject::TraceRollup(_) => "trace_rollup",
    }
}

fn json_text(v: serde_json::Value) -> Content {
    Content::text(serde_json::to_string_pretty(&v).unwrap_or_default())
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

    #[tool(description = "Record a Run object from JSON (execution receipt). Optional: trace_path, artifact_paths as JSON array. Returns the run hash on the first line.")]
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

    /// Phase 3b: parse already-captured test runner stdout into a
    /// metrics map. Pair this with `morph_commit` to attach
    /// behavioral evidence to the commit you're about to make.
    #[tool(description = "Parse a captured test-runner stdout string into Morph's canonical metric map. Required: stdout (string), runner (cargo|pytest|vitest|jest|go|auto). Optional: record (bool, also writes a Run linked to HEAD); workspace_path. When record=true, returns the run hash; otherwise returns the metric map JSON.")]
    async fn morph_eval_from_output(
        &self,
        params: Parameters<EvalFromOutputParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let runner = params.0.runner.as_deref().unwrap_or("auto").to_string();
        let stdout = params.0.stdout;
        let metrics = morph_core::parse_with_runner(&runner, &stdout, params.0.command.as_deref());
        if params.0.record.unwrap_or(false) {
            let hash = morph_core::record_eval_run(
                store.as_ref(),
                &metrics,
                &runner,
                params.0.command.as_deref(),
                Some(&stdout),
                None,
            ).map_err(mcp_err)?;
            Ok(CallToolResult::success(vec![Content::text(hash.to_string())]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&metrics).unwrap_or_default(),
            )]))
        }
    }

    /// Phase 3b: execute a test command, parse its stdout, and store
    /// a metrics-bearing Run linked to HEAD. Designed for headless
    /// agents that can shell out — the returned hash composes with
    /// `morph_commit { from_run: <hash> }`.
    #[tool(description = "Execute a test command, capture stdout, parse metrics, and write a Run object linked to HEAD. Required: command (array of argv strings). Optional: runner (cargo|pytest|vitest|jest|go|auto), cwd (path relative to repo root), workspace_path. Returns the run hash for use with `morph_commit { from_run }`.")]
    async fn morph_eval_run(
        &self,
        params: Parameters<EvalRunParams>,
    ) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let runner = params.0.runner.as_deref().unwrap_or("auto").to_string();
        if params.0.command.is_empty() {
            return Err(mcp_err("`command` array cannot be empty"));
        }
        let cwd_path = params.0.cwd.as_deref().map(std::path::Path::new);
        let outcome = morph_core::run_test_command(
            store.as_ref(),
            &repo_root,
            &params.0.command,
            &runner,
            cwd_path,
        ).map_err(mcp_err)?;
        let mut out = outcome.run_hash.to_string();
        if outcome.metrics.is_empty() {
            out.push_str(&format!(
                "\nwarning: no metrics extracted from `{}` (runner={}). \
                 Try setting `runner` explicitly or check the command output.",
                params.0.command.join(" "),
                runner,
            ));
        }
        Ok(CallToolResult::success(vec![Content::text(out)]))
    }

    /// Phase 4b: append acceptance cases from YAML/cucumber files
    /// to the repo's eval suite. Returns the new suite hash.
    #[tool(description = "Append acceptance cases from YAML/cucumber files to an EvalSuite. Required: paths (array of file or directory paths). Optional: suite (existing suite hash to extend; defaults to policy.default_eval_suite), no_default (build a fresh suite), no_set_default (don't update policy). Returns the new suite hash.")]
    async fn morph_add_eval_case(
        &self,
        params: Parameters<AddEvalCaseParams>,
    ) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        if params.0.paths.is_empty() {
            return Err(mcp_err("`paths` cannot be empty"));
        }
        let resolved: Vec<PathBuf> = params.0.paths.iter().map(|p| {
            let pp = std::path::Path::new(p);
            if pp.is_absolute() {
                pp.to_path_buf()
            } else {
                repo_root.join(pp)
            }
        }).collect();
        let cases = morph_core::add_cases_from_paths(&resolved).map_err(mcp_err)?;
        if cases.is_empty() {
            return Err(mcp_err(format!(
                "no acceptance cases found in: [{}]",
                params.0.paths.join(", ")
            )));
        }
        let morph_dir = repo_root.join(".morph");
        let policy = morph_core::read_policy(&morph_dir).map_err(mcp_err)?;
        let prev: Option<Hash> = if params.0.no_default.unwrap_or(false) {
            None
        } else if let Some(s) = params.0.suite.as_deref() {
            Some(Hash::from_hex(s).map_err(mcp_err)?)
        } else {
            match policy.default_eval_suite.as_deref() {
                Some(h) => Some(Hash::from_hex(h).map_err(mcp_err)?),
                None => None,
            }
        };
        let new_hash = morph_core::build_or_extend_suite(store.as_ref(), prev, &cases).map_err(mcp_err)?;
        if !params.0.no_set_default.unwrap_or(false) {
            let mut updated = policy.clone();
            updated.default_eval_suite = Some(new_hash.to_string());
            morph_core::write_policy(&morph_dir, &updated).map_err(mcp_err)?;
        }
        let body = format!(
            "{}\nadded {} case(s); default_eval_suite={}",
            new_hash,
            cases.len(),
            if params.0.no_set_default.unwrap_or(false) { "unchanged" } else { "updated" }
        );
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }

    /// Phase 4b: bulk-ingest a directory of specs/features into a
    /// fresh suite, replacing the default.
    #[tool(description = "Bulk-build a fresh EvalSuite from YAML/cucumber files and (by default) make it the policy's default. Required: paths. Optional: no_set_default. Returns the suite hash.")]
    async fn morph_eval_suite_from_specs(
        &self,
        params: Parameters<EvalSuiteFromSpecsParams>,
    ) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        if params.0.paths.is_empty() {
            return Err(mcp_err("`paths` cannot be empty"));
        }
        let resolved: Vec<PathBuf> = params.0.paths.iter().map(|p| {
            let pp = std::path::Path::new(p);
            if pp.is_absolute() {
                pp.to_path_buf()
            } else {
                repo_root.join(pp)
            }
        }).collect();
        let cases = morph_core::add_cases_from_paths(&resolved).map_err(mcp_err)?;
        if cases.is_empty() {
            return Err(mcp_err(format!(
                "no acceptance cases found in: [{}]",
                params.0.paths.join(", ")
            )));
        }
        let new_hash = morph_core::build_or_extend_suite(store.as_ref(), None, &cases).map_err(mcp_err)?;
        let morph_dir = repo_root.join(".morph");
        if !params.0.no_set_default.unwrap_or(false) {
            let mut policy = morph_core::read_policy(&morph_dir).map_err(mcp_err)?;
            policy.default_eval_suite = Some(new_hash.to_string());
            morph_core::write_policy(&morph_dir, &policy).map_err(mcp_err)?;
        }
        Ok(CallToolResult::success(vec![Content::text(format!(
            "{}\nbuilt fresh suite with {} case(s)",
            new_hash,
            cases.len()
        ))]))
    }

    /// Phase 4b: print the contents of a suite (default or by hash)
    /// as JSON for downstream tooling.
    #[tool(description = "Show the contents of an EvalSuite as JSON. Optional: suite (hash; defaults to policy.default_eval_suite).")]
    async fn morph_eval_suite_show(
        &self,
        params: Parameters<EvalSuiteShowParams>,
    ) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let morph_dir = repo_root.join(".morph");
        let policy = morph_core::read_policy(&morph_dir).map_err(mcp_err)?;
        let target = match params.0.suite.as_deref() {
            Some(s) => Hash::from_hex(s).map_err(mcp_err)?,
            None => match policy.default_eval_suite.as_deref() {
                Some(h) => Hash::from_hex(h).map_err(mcp_err)?,
                None => {
                    return Err(mcp_err(
                        "no `suite` supplied and policy.default_eval_suite is unset",
                    ));
                }
            },
        };
        let obj = store.get(&target).map_err(mcp_err)?;
        let suite = match obj {
            morph_core::MorphObject::EvalSuite(s) => s,
            _ => return Err(mcp_err(format!("object {} is not an EvalSuite", target))),
        };
        let json = serde_json::json!({
            "hash": target.to_string(),
            "case_count": suite.cases.len(),
            "metric_count": suite.metrics.len(),
            "cases": suite.cases,
            "metrics": suite.metrics,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap_or_default(),
        )]))
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

    #[tool(description = "Create a commit. Required: message. Optional: pipeline (hash), eval_suite (hash), metrics (JSON object), author, from_run (run hash for evidence-backed provenance), allow_empty_metrics (bypass policy), new_cases (comma-separated acceptance-case ids).")]
    async fn morph_commit(&self, params: Parameters<CommitParams>) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let morph_dir = repo_root.join(".morph");
        let version = read_repo_version(&morph_dir).map_err(mcp_err)?;
        let prog_hash = params.0.pipeline.as_deref().map(Hash::from_hex).transpose().map_err(mcp_err)?;
        let policy = morph_core::read_policy(&morph_dir).map_err(mcp_err)?;
        let suite_hash: Option<Hash> = match params.0.eval_suite.as_deref() {
            Some(s) => Some(Hash::from_hex(s).map_err(mcp_err)?),
            None => match policy.default_eval_suite.as_deref() {
                Some(s) => Some(Hash::from_hex(s).map_err(mcp_err)?),
                None => None,
            },
        };
        let metrics = params.0.metrics.unwrap_or_default();
        let allow_empty = params.0.allow_empty_metrics.unwrap_or(false);
        if !allow_empty {
            let missing = morph_core::missing_required_metrics(&policy, &metrics);
            if !missing.is_empty() {
                return Err(mcp_err(format!(
                    "policy requires metrics that are missing: [{}]. \
                     Pass `metrics`, run `morph_eval_run` / `morph_eval_from_output`, \
                     or set `allow_empty_metrics=true`.",
                    missing.join(", ")
                )));
            }
        }
        let provenance = match params.0.from_run {
            Some(ref h) => Some(morph_core::resolve_provenance_from_run(&store, &resolve_rev(store.as_ref(), h)?).map_err(mcp_err)?),
            None => None,
        };
        let metrics_were_empty = metrics.is_empty();
        let hash = morph_core::create_tree_commit_with_provenance(
            &store, &repo_root, prog_hash.as_ref(), suite_hash.as_ref(),
            metrics, params.0.message, params.0.author, Some(&version), provenance.as_ref(),
        ).map_err(mcp_err)?;

        if let Some(cases_arg) = params.0.new_cases.as_deref() {
            let cases = morph_core::parse_introduces_cases_arg(cases_arg);
            if let Some(ann) = morph_core::build_introduces_cases_annotation(&hash, &cases, None) {
                store.put(&ann).map_err(mcp_err)?;
            }
        }

        let mut out = hash.to_string();
        if metrics_were_empty {
            out.push_str(
                "\nwarning: commit has no observed_metrics. Morph cannot enforce \
                 behavioral merge gating without evidence. Pass `metrics`, \
                 run `morph_eval_run`, or set a policy via `morph policy init`.",
            );
        }
        Ok(CallToolResult::success(vec![Content::text(out)]))
    }

    #[tool(description = "Attach an annotation to an object. `target_hash` accepts full hashes, prefixes, `HEAD`, branch names, or tag names. Returns the annotation hash on the first line. Required: target_hash, kind, data (JSON object). Optional: target_sub, author.")]
    async fn morph_annotate(&self, params: Parameters<AnnotateParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let target = resolve_rev(store.as_ref(), &params.0.target_hash)?;
        let ann = morph_core::create_annotation(&target, params.0.target_sub, params.0.kind.clone(), params.0.data.unwrap_or_default(), params.0.author);
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

    #[tool(description = "Switch HEAD to a branch, tag, prefix, or detached commit. Accepts any revision the CLI accepts.")]
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

    #[tool(description = "Repository state as a structured JSON envelope: `{repo, branch, detached, head: {hash, short, message, author, timestamp, metrics}, working_tree: {clean, added, modified, deleted}, staging, activity, eval_suite, required_metrics, merge}`. Designed for agents to reason about what's changed and what evidence is missing.")]
    async fn morph_status(&self, params: Parameters<WorkspaceOnlyParams>) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let body = build_status_json(&repo_root, store.as_ref()).map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![json_text(body)]))
    }

    /// Phase 5b: cheap, structured "what's still missing?" check for
    /// agents and stop-hooks. Returns a JSON array; an empty array
    /// means the working state is fully covered by behavioral
    /// evidence as far as Morph can tell.
    #[tool(description = "Return a structured list of behavioral-evidence gaps for the current repo: empty HEAD metrics, empty default eval suite, or a dirty working tree without a metric-bearing run since the last commit. Output is `{\"gaps\": [{kind, hint}]}` JSON. Cheap enough to call before every commit.")]
    async fn morph_eval_gaps(
        &self,
        params: Parameters<WorkspaceOnlyParams>,
    ) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let morph_dir = repo_root.join(".morph");
        let changes = morph_core::working_status(&store, &repo_root).map_err(mcp_err)?;
        let gaps = morph_core::compute_eval_gaps(&morph_dir, store.as_ref(), changes.len() as u64).map_err(mcp_err)?;
        let body = serde_json::json!({
            "gaps": gaps,
            "count": gaps.len(),
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&body).unwrap_or_default(),
        )]))
    }

    #[tool(description = "Show commit history from HEAD or a named ref. Returns a JSON envelope `{commits: [{hash, short, message, author, timestamp, parents, metrics}]}` newest first.")]
    async fn morph_log(&self, params: Parameters<LogParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let ref_name = params.0.ref_name.as_deref().unwrap_or("HEAD");
        let hashes = morph_core::log_from(&store, ref_name).map_err(mcp_err)?;
        let mut commits = Vec::with_capacity(hashes.len());
        for h in &hashes {
            if let MorphObject::Commit(c) = store.get(h).map_err(mcp_err)? {
                let h_str = h.to_string();
                let metrics = morph_core::effective_metrics_for_commit(store.as_ref(), h, &c)
                    .map_err(mcp_err)?;
                commits.push(serde_json::json!({
                    "hash": h_str,
                    "short": short_hash(&h_str),
                    "message": c.message,
                    "author": c.author,
                    "timestamp": c.timestamp,
                    "parents": c.parents,
                    "metrics": metrics,
                }));
            }
        }
        let body = serde_json::json!({
            "ref": ref_name,
            "commits": commits,
            "count": hashes.len(),
        });
        Ok(CallToolResult::success(vec![json_text(body)]))
    }

    #[tool(description = "Show a stored Morph object as pretty JSON. Accepts full hashes, prefixes (>=4 hex), `HEAD`, branch names, or tag names.")]
    async fn morph_show(&self, params: Parameters<ShowParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let hash = resolve_rev(store.as_ref(), &params.0.hash)?;
        let obj = store.get(&hash).map_err(mcp_err)?;
        let body = serde_json::json!({
            "input": params.0.hash,
            "hash": hash.to_string(),
            "short": short_hash(&hash.to_string()),
            "type": morph_object_type_str(&obj),
            "object": obj,
        });
        Ok(CallToolResult::success(vec![json_text(body)]))
    }

    #[tool(description = "Diff two commits (or refs). Accepts full hashes, prefixes, `HEAD`, branch names, or tag names. Returns a JSON envelope with `from`, `to`, and `changes: [{status, path}]`.")]
    async fn morph_diff(&self, params: Parameters<DiffParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let old_hash = resolve_rev(store.as_ref(), &params.0.old_ref)?;
        let new_hash = resolve_rev(store.as_ref(), &params.0.new_ref)?;
        let entries = morph_core::diff_commits(&store, &old_hash, &new_hash).map_err(mcp_err)?;
        let changes: Vec<_> = entries.iter().map(|e| serde_json::json!({
            "status": e.status.to_string(),
            "path": e.path,
        })).collect();
        let body = serde_json::json!({
            "from": { "ref": params.0.old_ref, "hash": old_hash.to_string(), "short": short_hash(&old_hash.to_string()) },
            "to":   { "ref": params.0.new_ref, "hash": new_hash.to_string(), "short": short_hash(&new_hash.to_string()) },
            "changes": changes,
            "count": entries.len(),
        });
        Ok(CallToolResult::success(vec![json_text(body)]))
    }

    fn resolve_run_hash(&self, store: &dyn Store, hash_str: &str) -> Result<Hash, McpError> {
        let h = resolve_rev(store, hash_str)?;
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

    /// Agent-friendly read-side helpers that mirror the human CLI:
    /// `morph head`, `morph identify`, `morph annotations`, `morph
    /// run list`, `morph branch`, `morph refs`, `morph remote list`.
    /// All of these return JSON envelopes so agents don't have to
    /// scrape text.

    #[tool(description = "Return structured info about the current HEAD: `{hash, short, branch, detached, message, author, timestamp, parents, metrics}`. Errors when no commits exist yet.")]
    async fn morph_head(&self, params: Parameters<WorkspaceOnlyParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let head = morph_core::resolve_head(&store).map_err(mcp_err)?
            .ok_or_else(|| mcp_err("no commit yet on this branch"))?;
        let branch = morph_core::current_branch(&store).map_err(mcp_err)?;
        let commit = match store.get(&head).map_err(mcp_err)? {
            MorphObject::Commit(c) => c,
            _ => return Err(mcp_err("HEAD does not point to a commit")),
        };
        let h_str = head.to_string();
        let metrics = morph_core::effective_metrics_for_commit(store.as_ref(), &head, &commit)
            .map_err(mcp_err)?;
        let body = serde_json::json!({
            "hash": h_str,
            "short": short_hash(&h_str),
            "branch": branch,
            "detached": branch.is_none(),
            "message": commit.message,
            "author": commit.author,
            "timestamp": commit.timestamp,
            "parents": commit.parents,
            "metrics": metrics,
        });
        Ok(CallToolResult::success(vec![json_text(body)]))
    }

    #[tool(description = "Resolve any revision (full hash, prefix, `HEAD`, branch name, tag name) to its full hash and object type. Returns `{input, hash, short, type, message?, author?, timestamp?}`. Use this when you have a name and need a stable hash for downstream tools.")]
    async fn morph_identify(&self, params: Parameters<IdentifyParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let resolved = resolve_rev(store.as_ref(), &params.0.revision)?;
        let obj = store.get(&resolved).map_err(mcp_err)?;
        let kind = morph_object_type_str(&obj);
        let h_str = resolved.to_string();
        let mut body = serde_json::json!({
            "input": params.0.revision,
            "hash": h_str,
            "short": short_hash(&h_str),
            "type": kind,
        });
        if let MorphObject::Commit(c) = &obj {
            body["message"] = serde_json::Value::String(c.message.clone());
            body["author"] = serde_json::Value::String(c.author.clone());
            body["timestamp"] = serde_json::Value::String(c.timestamp.clone());
        }
        Ok(CallToolResult::success(vec![json_text(body)]))
    }

    #[tool(description = "List every annotation attached to a target object. Accepts any revision the CLI accepts. Returns `{target, annotations: [{hash, short, kind, author, target_sub, data}], count}`.")]
    async fn morph_annotations(&self, params: Parameters<AnnotationsParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let target = resolve_rev(store.as_ref(), &params.0.target_hash)?;
        let anns = morph_core::list_annotations(&store, &target, params.0.target_sub.as_deref()).map_err(mcp_err)?;
        let entries: Vec<_> = anns.iter().map(|(h, a)| {
            let h_str = h.to_string();
            serde_json::json!({
                "hash": h_str,
                "short": short_hash(&h_str),
                "kind": a.kind,
                "author": a.author,
                "target": a.target,
                "target_sub": a.target_sub,
                "timestamp": a.timestamp,
                "data": a.data,
            })
        }).collect();
        let body = serde_json::json!({
            "target": target.to_string(),
            "target_short": short_hash(&target.to_string()),
            "annotations": entries,
            "count": anns.len(),
        });
        Ok(CallToolResult::success(vec![json_text(body)]))
    }

    #[tool(description = "List runs in the store, newest first. Returns `{runs: [{hash, short, agent_id, agent_version, model, pipeline, commit?, has_metrics, metrics?}], count}`. Pair with `morph_show {hash}` to drill into one.")]
    async fn morph_run_list(&self, params: Parameters<WorkspaceOnlyParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let runs = store.list(morph_core::ObjectType::Run).map_err(mcp_err)?;
        let entries: Vec<_> = runs.iter().rev().map(|h| {
            let h_str = h.to_string();
            let mut entry = serde_json::json!({
                "hash": h_str,
                "short": short_hash(&h_str),
            });
            if let Ok(MorphObject::Run(r)) = store.get(h) {
                entry["agent_id"] = serde_json::Value::String(r.agent.id.clone());
                entry["agent_version"] = serde_json::Value::String(r.agent.version.clone());
                entry["model"] = serde_json::Value::String(r.environment.model.clone());
                entry["pipeline"] = serde_json::Value::String(r.pipeline.clone());
                if let Some(c) = &r.commit {
                    entry["commit"] = serde_json::Value::String(c.clone());
                }
                entry["has_metrics"] = serde_json::Value::Bool(!r.metrics.is_empty());
                if !r.metrics.is_empty() {
                    if let Ok(m) = serde_json::to_value(&r.metrics) {
                        entry["metrics"] = m;
                    }
                }
            }
            entry
        }).collect();
        let body = serde_json::json!({ "runs": entries, "count": runs.len() });
        Ok(CallToolResult::success(vec![json_text(body)]))
    }

    #[tool(description = "List branches with their tip hashes. Returns `{current, detached, branches: [{name, hash, short, current}], count}`.")]
    async fn morph_branch_list(&self, params: Parameters<WorkspaceOnlyParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let current = morph_core::current_branch(&store).map_err(mcp_err)?;
        let refs = morph_core::list_refs(store.as_ref()).map_err(mcp_err)?;
        let mut branches = Vec::new();
        for (name, hash) in &refs {
            if let Some(short) = name.strip_prefix("heads/") {
                let h_str = hash.to_string();
                branches.push(serde_json::json!({
                    "name": short,
                    "hash": h_str,
                    "short": short_hash(&h_str),
                    "current": current.as_deref() == Some(short),
                }));
            }
        }
        let body = serde_json::json!({
            "current": current,
            "detached": current.is_none(),
            "branches": branches,
            "count": branches.len(),
        });
        Ok(CallToolResult::success(vec![json_text(body)]))
    }

    #[tool(description = "List every ref (branches, tags, HEAD) with its hash. Returns `{refs: [{name, hash, short}], count}`.")]
    async fn morph_refs(&self, params: Parameters<WorkspaceOnlyParams>) -> Result<CallToolResult, McpError> {
        let (_repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let refs = morph_core::list_refs(store.as_ref()).map_err(mcp_err)?;
        let entries: Vec<_> = refs.iter().map(|(name, hash)| {
            let h_str = hash.to_string();
            serde_json::json!({
                "name": name,
                "hash": h_str,
                "short": short_hash(&h_str),
            })
        }).collect();
        let body = serde_json::json!({ "refs": entries, "count": refs.len() });
        Ok(CallToolResult::success(vec![json_text(body)]))
    }

    #[tool(description = "List configured remotes. Returns `{remotes: [{name, path}], count}`.")]
    async fn morph_remote_list(&self, params: Parameters<WorkspaceOnlyParams>) -> Result<CallToolResult, McpError> {
        let (repo_root, _store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let remotes = morph_core::read_remotes(&repo_root.join(".morph")).map_err(mcp_err)?;
        let entries: Vec<_> = remotes.iter().map(|(name, spec)| serde_json::json!({
            "name": name,
            "path": spec.path,
        })).collect();
        let body = serde_json::json!({ "remotes": entries, "count": remotes.len() });
        Ok(CallToolResult::success(vec![json_text(body)]))
    }

    #[tool(description = "Merge a branch into the current branch (requires behavioral dominance). When `retire` is set, an attributed `review` node is auto-injected into the merged pipeline; supply `retire_reason` to record why.")]
    async fn morph_merge(&self, params: Parameters<MergeParams>) -> Result<CallToolResult, McpError> {
        let (repo_root, store) = self.repo_store(params.0.workspace_path.as_deref()).map_err(mcp_err)?;
        let pipeline = Hash::from_hex(&params.0.pipeline).map_err(mcp_err)?;
        let suite = params.0.eval_suite.as_deref().map(Hash::from_hex).transpose().map_err(mcp_err)?;
        let retired: Option<Vec<String>> = params.0.retire.as_deref()
            .map(|s| s.split(',').map(|r| r.trim().to_string()).collect());
        let morph_dir = repo_root.join(".morph");
        let version = morph_core::read_repo_version(&morph_dir).map_err(mcp_err)?;
        let mut plan = morph_core::prepare_merge(&store, &params.0.branch, suite.as_ref(), retired.as_deref()).map_err(mcp_err)?;
        plan.retire_reason = params.0.retire_reason.clone();
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
        // Phase 2a: tests predate the opinionated default policy and
        // commit without tests_total/tests_passed. Reset to an empty
        // policy so each test can opt in to enforcement explicitly
        // when it cares.
        let permissive = morph_core::RepoPolicy::default();
        morph_core::write_policy(&dir.path().join(".morph"), &permissive).unwrap();
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
                message: "first commit".into(),
                workspace_path: Some(ws), author: Some("test-author".into()),
                allow_empty_metrics: Some(true),
                ..Default::default()
            })).await.unwrap();
        let first_line = extract_text(&commit_result).lines().next().unwrap_or("").to_string();
        assert_eq!(first_line.len(), 64);
    }

    #[tokio::test]
    async fn branch_and_checkout() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("f.txt"), "content").unwrap();
        server.morph_stage(Parameters(StageParams { workspace_path: Some(ws.clone()), paths: Some(vec![".".into()]) })).await.unwrap();
        server.morph_commit(Parameters(CommitParams { message: "initial".into(), workspace_path: Some(ws.clone()), allow_empty_metrics: Some(true), ..Default::default() })).await.unwrap();
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
            message: "commit with metrics".into(),
            workspace_path: Some(ws), metrics: Some(metrics), author: Some("agent".into()),
            ..Default::default()
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
            message: "evidence-backed commit".into(),
            workspace_path: Some(ws), from_run: Some(run_hash),
            allow_empty_metrics: Some(true),
            ..Default::default()
        })).await.unwrap();
        let first_line = extract_text(&commit_result).lines().next().unwrap_or("").to_string();
        assert_eq!(first_line.len(), 64);
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
    async fn status_includes_evidence_block_with_suite_and_runs() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();

        // Build a 1-case suite so the evidence summary has a real
        // case count to report.
        std::fs::create_dir_all(dir.path().join("specs")).unwrap();
        std::fs::write(
            dir.path().join("specs/login.yaml"),
            "- name: login_alpha\n",
        )
        .unwrap();
        server
            .morph_add_eval_case(Parameters(AddEvalCaseParams {
                paths: vec!["specs/login.yaml".into()],
                suite: None,
                no_default: None,
                no_set_default: None,
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();

        // Stage + commit once so HEAD has metrics.
        std::fs::write(dir.path().join("foo.txt"), "v1").unwrap();
        server
            .morph_stage(Parameters(StageParams {
                workspace_path: Some(ws.clone()),
                paths: Some(vec![".".into()]),
            }))
            .await
            .unwrap();
        let mut metrics = std::collections::BTreeMap::new();
        metrics.insert("tests_passed".into(), 5.0);
        metrics.insert("tests_total".into(), 5.0);
        server
            .morph_commit(Parameters(CommitParams {
                message: "with metrics".into(),
                workspace_path: Some(ws.clone()),
                metrics: Some(metrics),
                ..Default::default()
            }))
            .await
            .unwrap();

        // Record one Run with metrics so the "recent runs" line has
        // signal too.
        server
            .morph_eval_from_output(Parameters(EvalFromOutputParams {
                stdout: "test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s\n".into(),
                runner: Some("cargo".into()),
                command: None,
                record: Some(true),
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();

        let result = server
            .morph_status(Parameters(WorkspaceOnlyParams {
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();
        let text = extract_text(&result);
        let v: serde_json::Value = serde_json::from_str(text).expect("status returns JSON");

        assert_eq!(v["branch"], "main", "status JSON branch: {v}");
        assert_eq!(v["head"]["metrics"]["tests_passed"], 5.0, "head metrics: {v}");
        assert_eq!(v["eval_suite"]["case_count"], 1, "suite case count: {v}");
        assert!(
            v["activity"]["runs"].as_u64().unwrap_or(0) >= 1,
            "expected at least one run in activity: {v}"
        );
    }

    #[tokio::test]
    async fn log_shows_commits() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("f.txt"), "data").unwrap();
        server.morph_stage(Parameters(StageParams { workspace_path: Some(ws.clone()), paths: Some(vec![".".into()]) })).await.unwrap();
        server.morph_commit(Parameters(CommitParams { message: "test-commit-msg".into(), workspace_path: Some(ws.clone()), allow_empty_metrics: Some(true), ..Default::default() })).await.unwrap();
        let result = server.morph_log(Parameters(LogParams { ref_name: None, workspace_path: Some(ws) })).await.unwrap();
        assert!(extract_text(&result).contains("test-commit-msg"));
    }

    #[tokio::test]
    async fn show_returns_json() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("x.txt"), "hello").unwrap();
        server.morph_stage(Parameters(StageParams { workspace_path: Some(ws.clone()), paths: Some(vec![".".into()]) })).await.unwrap();
        let commit_result = server.morph_commit(Parameters(CommitParams { message: "initial".into(), workspace_path: Some(ws.clone()), allow_empty_metrics: Some(true), ..Default::default() })).await.unwrap();
        let commit_hash = extract_text(&commit_result).lines().next().unwrap_or("").to_string();
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
    async fn eval_from_output_returns_metrics_json() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        let result = server
            .morph_eval_from_output(Parameters(EvalFromOutputParams {
                stdout: "test result: ok. 5 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s\n".into(),
                runner: Some("cargo".into()),
                command: None,
                record: Some(false),
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();
        let text = extract_text(&result);
        let parsed: std::collections::BTreeMap<String, f64> =
            serde_json::from_str(text).expect("expected metrics json");
        assert_eq!(parsed.get("tests_passed").copied(), Some(5.0));
        assert_eq!(parsed.get("tests_failed").copied(), Some(1.0));
        assert_eq!(parsed.get("tests_total").copied(), Some(6.0));
    }

    #[tokio::test]
    async fn eval_from_output_record_links_run_to_head() {
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
        let head = extract_text(
            &server
                .morph_commit(Parameters(CommitParams {
                    message: "base".into(),
                    workspace_path: Some(ws.clone()),
                    allow_empty_metrics: Some(true),
                    ..Default::default()
                }))
                .await
                .unwrap(),
        )
        .lines()
        .next()
        .unwrap_or("")
        .to_string();

        let result = server
            .morph_eval_from_output(Parameters(EvalFromOutputParams {
                stdout: "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n".into(),
                runner: Some("auto".into()),
                command: Some("cargo test".into()),
                record: Some(true),
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();
        let run_hash = extract_text(&result).trim().to_string();
        assert_eq!(run_hash.len(), 64, "expected 64-char run hash, got {run_hash:?}");

        let store = morph_core::open_store(&dir.path().join(".morph")).unwrap();
        let obj = store.get(&morph_core::Hash::from_hex(&run_hash).unwrap()).unwrap();
        match obj {
            morph_core::MorphObject::Run(r) => {
                assert_eq!(r.commit.as_deref(), Some(head.as_str()));
                assert_eq!(r.metrics.get("tests_passed").copied(), Some(3.0));
            }
            _ => panic!("expected a Run object"),
        }
    }

    #[tokio::test]
    async fn eval_run_executes_command_and_records_run() {
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
        server
            .morph_commit(Parameters(CommitParams {
                message: "base".into(),
                workspace_path: Some(ws.clone()),
                allow_empty_metrics: Some(true),
                ..Default::default()
            }))
            .await
            .unwrap();

        let result = server
            .morph_eval_run(Parameters(EvalRunParams {
                command: vec![
                    "printf".into(),
                    "test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s\n".into(),
                ],
                runner: Some("cargo".into()),
                cwd: None,
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();
        let body = extract_text(&result);
        let run_hash = body.lines().next().unwrap_or("").to_string();
        assert_eq!(run_hash.len(), 64, "expected 64-char run hash, got {body:?}");
        assert!(!body.contains("warning: no metrics"), "should have parsed metrics: {body}");

        let store = morph_core::open_store(&dir.path().join(".morph")).unwrap();
        let obj = store.get(&morph_core::Hash::from_hex(&run_hash).unwrap()).unwrap();
        match obj {
            morph_core::MorphObject::Run(r) => {
                assert_eq!(r.metrics.get("tests_passed").copied(), Some(7.0));
            }
            _ => panic!("expected a Run object"),
        }
    }

    #[tokio::test]
    async fn add_eval_case_creates_default_suite() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::create_dir_all(dir.path().join("specs")).unwrap();
        std::fs::write(
            dir.path().join("specs/login.yaml"),
            "- name: login_alpha\n  steps:\n    - morph: [status]\n",
        )
        .unwrap();

        let result = server
            .morph_add_eval_case(Parameters(AddEvalCaseParams {
                paths: vec!["specs/login.yaml".into()],
                suite: None,
                no_default: None,
                no_set_default: None,
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();
        let body = extract_text(&result);
        let suite_hash = body.lines().next().unwrap_or("").to_string();
        assert_eq!(suite_hash.len(), 64, "expected 64-char suite hash, got: {body}");

        // Default eval suite is now wired to the new hash.
        let policy = morph_core::read_policy(&dir.path().join(".morph")).unwrap();
        assert_eq!(policy.default_eval_suite.as_deref(), Some(suite_hash.as_str()));
    }

    #[tokio::test]
    async fn eval_suite_show_lists_cases() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::create_dir_all(dir.path().join("specs")).unwrap();
        std::fs::write(
            dir.path().join("specs/a.yaml"),
            "- name: case_one\n- name: case_two\n",
        )
        .unwrap();
        server
            .morph_add_eval_case(Parameters(AddEvalCaseParams {
                paths: vec!["specs/a.yaml".into()],
                suite: None,
                no_default: None,
                no_set_default: None,
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();

        let result = server
            .morph_eval_suite_show(Parameters(EvalSuiteShowParams {
                suite: None,
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();
        let text = extract_text(&result);
        assert!(text.contains("case_one"), "missing case_one in: {text}");
        assert!(text.contains("case_two"), "missing case_two in: {text}");
        let json: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(json["case_count"], serde_json::json!(2));
    }

    #[tokio::test]
    async fn eval_suite_from_specs_replaces_default() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::create_dir_all(dir.path().join("specs")).unwrap();
        std::fs::write(dir.path().join("specs/x.yaml"), "- name: x_only\n").unwrap();
        std::fs::write(dir.path().join("specs/y.yaml"), "- name: y_only\n").unwrap();

        let first = extract_text(
            &server
                .morph_eval_suite_from_specs(Parameters(EvalSuiteFromSpecsParams {
                    paths: vec!["specs".into()],
                    no_set_default: None,
                    workspace_path: Some(ws.clone()),
                }))
                .await
                .unwrap(),
        )
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
        assert_eq!(first.len(), 64);

        // Removing y.yaml and re-running gives a smaller suite — proves
        // suite-from-specs builds fresh, not append.
        std::fs::remove_file(dir.path().join("specs/y.yaml")).unwrap();
        let second_body = extract_text(
            &server
                .morph_eval_suite_from_specs(Parameters(EvalSuiteFromSpecsParams {
                    paths: vec!["specs".into()],
                    no_set_default: None,
                    workspace_path: Some(ws.clone()),
                }))
                .await
                .unwrap(),
        )
        .to_string();
        let second = second_body.lines().next().unwrap_or("").to_string();
        assert_eq!(second.len(), 64);
        assert_ne!(first, second, "rebuild with fewer cases must change the hash");

        let show = extract_text(
            &server
                .morph_eval_suite_show(Parameters(EvalSuiteShowParams {
                    suite: None,
                    workspace_path: Some(ws.clone()),
                }))
                .await
                .unwrap(),
        )
        .to_string();
        assert!(show.contains("x_only"));
        assert!(!show.contains("y_only"));
    }

    #[tokio::test]
    async fn eval_gaps_returns_structured_list() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();

        // Fresh repo: no suite, no commits → at least
        // empty_default_suite is reported.
        let result = server
            .morph_eval_gaps(Parameters(WorkspaceOnlyParams {
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();
        let body = extract_text(&result);
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        let kinds: Vec<&str> = v["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|g| g["kind"].as_str())
            .collect();
        assert!(kinds.contains(&"empty_default_suite"), "kinds={kinds:?}");

        // Add a suite + commit with metrics → no gaps reported.
        std::fs::create_dir_all(dir.path().join("specs")).unwrap();
        std::fs::write(dir.path().join("specs/a.yaml"), "- name: alpha\n").unwrap();
        server
            .morph_add_eval_case(Parameters(AddEvalCaseParams {
                paths: vec!["specs/a.yaml".into()],
                suite: None,
                no_default: None,
                no_set_default: None,
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();
        std::fs::write(dir.path().join("foo.txt"), "v1").unwrap();
        server
            .morph_stage(Parameters(StageParams {
                workspace_path: Some(ws.clone()),
                paths: Some(vec![".".into()]),
            }))
            .await
            .unwrap();
        let mut metrics = std::collections::BTreeMap::new();
        metrics.insert("tests_passed".into(), 1.0);
        metrics.insert("tests_total".into(), 1.0);
        server
            .morph_commit(Parameters(CommitParams {
                message: "with metrics".into(),
                workspace_path: Some(ws.clone()),
                metrics: Some(metrics),
                ..Default::default()
            }))
            .await
            .unwrap();

        let result = server
            .morph_eval_gaps(Parameters(WorkspaceOnlyParams {
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(extract_text(&result)).unwrap();
        assert_eq!(v["count"], serde_json::json!(0), "expected zero gaps after evidence is registered, got: {v:?}");
    }

    #[tokio::test]
    async fn diff_between_commits() {
        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
        server.morph_stage(Parameters(StageParams { workspace_path: Some(ws.clone()), paths: Some(vec![".".into()]) })).await.unwrap();
        let c1 = extract_text(&server.morph_commit(Parameters(CommitParams { message: "first".into(), workspace_path: Some(ws.clone()), allow_empty_metrics: Some(true), ..Default::default() })).await.unwrap()).lines().next().unwrap_or("").to_string();
        std::fs::write(dir.path().join("b.txt"), "new file").unwrap();
        server.morph_stage(Parameters(StageParams { workspace_path: Some(ws.clone()), paths: Some(vec!["b.txt".into()]) })).await.unwrap();
        let c2 = extract_text(&server.morph_commit(Parameters(CommitParams { message: "second".into(), workspace_path: Some(ws.clone()), allow_empty_metrics: Some(true), ..Default::default() })).await.unwrap()).lines().next().unwrap_or("").to_string();
        let result = server.morph_diff(Parameters(DiffParams { old_ref: c1, new_ref: c2, workspace_path: Some(ws) })).await.unwrap();
        assert!(extract_text(&result).contains("A"));
        assert!(extract_text(&result).contains("b.txt"));
    }

    /// Paper §4.3: when `morph_merge` is called with `retire` and a
    /// `retire_reason`, the merged pipeline must carry an attributed
    /// `review` node whose `params.reason` matches the supplied text.
    /// Mirrors the CLI acceptance specs but exercises the MCP plumbing
    /// directly so agent-driven retirements stay auditable.
    #[tokio::test]
    async fn merge_with_retire_reason_synthesizes_attributed_review_node() {
        use morph_core::objects::{
            EvalMetric, EvalSuite, MorphObject, Pipeline, PipelineGraph, PipelineNode,
        };

        let (dir, server) = setup_repo();
        let ws = dir.path().to_str().unwrap().to_string();
        let morph_dir = dir.path().join(".morph");

        let store = morph_core::open_store(&morph_dir).unwrap();
        let pipeline_obj = Pipeline {
            graph: PipelineGraph {
                nodes: vec![PipelineNode {
                    id: "n1".into(),
                    kind: "identity".into(),
                    ref_: None,
                    params: std::collections::BTreeMap::new(),
                    env: None,
                }],
                edges: vec![],
            },
            prompts: vec![],
            eval_suite: None,
            attribution: None,
            provenance: None,
        };
        let pipeline_hash = store
            .put(&MorphObject::Pipeline(pipeline_obj))
            .unwrap()
            .to_string();

        let suite_obj = EvalSuite {
            cases: vec![],
            metrics: vec![
                EvalMetric::new("acc", "mean", 0.0),
                EvalMetric::new("old_metric", "mean", 0.0),
            ],
        };
        let suite_hash = store
            .put(&MorphObject::EvalSuite(suite_obj))
            .unwrap()
            .to_string();

        std::fs::write(dir.path().join("a.txt"), "aaa").unwrap();
        server
            .morph_stage(Parameters(StageParams {
                workspace_path: Some(ws.clone()),
                paths: Some(vec![".".into()]),
            }))
            .await
            .unwrap();
        let mut head_metrics = std::collections::BTreeMap::new();
        head_metrics.insert("acc".into(), 0.9);
        head_metrics.insert("old_metric".into(), 0.8);
        server
            .morph_commit(Parameters(CommitParams {
                message: "main commit".into(),
                workspace_path: Some(ws.clone()),
                pipeline: Some(pipeline_hash.clone()),
                eval_suite: Some(suite_hash.clone()),
                metrics: Some(head_metrics),
                allow_empty_metrics: Some(true),
                ..Default::default()
            }))
            .await
            .unwrap();

        server
            .morph_branch(Parameters(BranchParams {
                name: "feature".into(),
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();
        server
            .morph_checkout(Parameters(CheckoutParams {
                ref_name: "feature".into(),
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();
        std::fs::write(dir.path().join("b.txt"), "bbb").unwrap();
        server
            .morph_stage(Parameters(StageParams {
                workspace_path: Some(ws.clone()),
                paths: Some(vec![".".into()]),
            }))
            .await
            .unwrap();
        let mut feat_metrics = std::collections::BTreeMap::new();
        feat_metrics.insert("acc".into(), 0.85);
        feat_metrics.insert("old_metric".into(), 0.7);
        server
            .morph_commit(Parameters(CommitParams {
                message: "feature commit".into(),
                workspace_path: Some(ws.clone()),
                pipeline: Some(pipeline_hash.clone()),
                eval_suite: Some(suite_hash.clone()),
                metrics: Some(feat_metrics),
                allow_empty_metrics: Some(true),
                ..Default::default()
            }))
            .await
            .unwrap();

        server
            .morph_checkout(Parameters(CheckoutParams {
                ref_name: "main".into(),
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();

        let mut merged_metrics = std::collections::BTreeMap::new();
        merged_metrics.insert("acc".into(), 0.92);
        let merge_result = server
            .morph_merge(Parameters(MergeParams {
                branch: "feature".into(),
                message: "retired merge via mcp".into(),
                pipeline: pipeline_hash.clone(),
                metrics: merged_metrics,
                eval_suite: None,
                author: Some("mcp-agent".into()),
                retire: Some("old_metric".into()),
                retire_reason: Some("model swap dropped this metric".into()),
                workspace_path: Some(ws.clone()),
            }))
            .await
            .unwrap();
        let merge_hash = extract_text(&merge_result).trim().to_string();
        assert_eq!(merge_hash.len(), 64, "merge should return a 64-hex hash");

        let store = morph_core::open_store(&morph_dir).unwrap();
        let merge_obj = store.get(&Hash::from_hex(&merge_hash).unwrap()).unwrap();
        let merge_commit = match merge_obj {
            MorphObject::Commit(c) => c,
            _ => panic!("merge hash did not resolve to a Commit"),
        };
        assert_ne!(
            merge_commit.pipeline, pipeline_hash,
            "merge pipeline must differ from input (review node was injected)"
        );

        let merged_pipeline = match store
            .get(&Hash::from_hex(&merge_commit.pipeline).unwrap())
            .unwrap()
        {
            MorphObject::Pipeline(p) => p,
            _ => panic!("merge commit's pipeline did not resolve to a Pipeline"),
        };

        let review = merged_pipeline
            .graph
            .nodes
            .iter()
            .find(|n| n.kind == "review")
            .expect("merged pipeline must have a review node when --retire is set");
        assert!(
            review.id.starts_with("review-retirement-"),
            "synthesized review node should have a deterministic id, got {:?}",
            review.id
        );
        let reason = review
            .params
            .get("reason")
            .and_then(|v| v.as_str())
            .expect("review node should carry params.reason");
        assert_eq!(
            reason, "model swap dropped this metric",
            "retire_reason from the MCP request must reach params.reason"
        );
        let retired_param = review
            .params
            .get("retired_metrics")
            .and_then(|v| v.as_array())
            .expect("review node should carry params.retired_metrics");
        let retired_names: Vec<&str> =
            retired_param.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(retired_names, vec!["old_metric"]);

        let attribution = merged_pipeline
            .attribution
            .as_ref()
            .expect("attribution map must be set when injecting a review node");
        let entry = attribution
            .get(&review.id)
            .expect("attribution map must include the synthesized review node id");
        assert_eq!(
            entry.agent_id, "mcp-agent",
            "attribution must record the merge author from the MCP `author` field"
        );
    }
}
