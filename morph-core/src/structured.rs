//! Structured trace metadata layer — on top of raw [`crate::tap`].
//!
//! Raw trace data is not enough for downstream replay/eval generation.
//! Tools like tap had to re-infer "what kind of task is this? where is it
//! localized? what is the final artifact?" from low-level events every
//! time. This module computes those structured summaries once, so that
//! MCP clients, CLI users, and eval harnesses can operate on a compact
//! JSON structure instead of re-scanning event blobs.
//!
//! ## What the structured layer exposes
//!
//! For a given trace/run we derive (heuristically, best-effort):
//!
//! * [`TaskPhase`] — create_code / modify_code / fix_bug / diagnose /
//!   verify / explain.
//! * [`TaskScope`] — single_function / single_file / multi_file / broad.
//! * `target_files` / `target_symbols` — the files/functions the task
//!   is focused on.
//! * [`ArtifactType`] — function_only / unified_diff / full_file /
//!   tool_execution / explanation.
//! * `task_goal` — the semantic ask (short string).
//! * `verification_actions` — test/build/demo commands separated out
//!   from the task itself.
//! * changed / preserved / restored construct summaries.
//!
//! ## Why separating task vs. verification matters
//!
//! Replay systems need to replay the *task* (the requested change) but
//! not necessarily re-execute every verification step the author ran
//! (tests, demo scripts, manual spot-checks). Exposing them as distinct
//! fields lets replayers keep the coding ask while dropping the
//! verification burden when it's not reproducible in the target env.
//!
//! ## Why `task_scope` matters
//!
//! For localized coding tasks, function-level artifacts and
//! function-level context produce dramatically better replay/eval
//! quality than whole-file or diff-heavy context. When `task_scope` is
//! `single_function`, downstream tools should:
//!
//! * show the function source (not the whole file)
//! * ask the model to return only the function (not a full file)
//! * judge only on the function's behavior
//!
//! ## Language generality
//!
//! The metadata schema here is language-agnostic. Symbol extraction and
//! slicing is delegated to [`crate::language::LanguageAdapter`]; this
//! module never imports a language-specific implementation directly,
//! and Python is just the first concrete adapter.

use crate::language::{adapter_for_filename, LanguageAdapter, PythonLanguageAdapter};
use crate::objects::{MorphObject, Trace};
use crate::store::{MorphError, ObjectType, Store};
use crate::tap::{extract_task, TapStep, TapTask};
use crate::Hash;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

// ---------- Schema types ----------

/// Broad category of what the user asked the agent to do. Heuristic.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    /// User asked to implement something new.
    CreateCode,
    /// User asked to change/refactor existing code.
    ModifyCode,
    /// User asked to fix a bug.
    FixBug,
    /// User asked to investigate or describe behavior.
    Diagnose,
    /// User asked to run tests / verify existing behavior.
    Verify,
    /// User asked for an explanation without code changes.
    Explain,
    /// Could not classify.
    Other,
}

/// Localization of the task. See module-level docs for why this matters.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskScope {
    /// The task targets a single function or method.
    SingleFunction,
    /// The task touches one file but multiple symbols, or scope not
    /// narrowed to a single function.
    SingleFile,
    /// The task edits multiple files in a related area.
    MultiFile,
    /// Large / cross-cutting change.
    Broad,
    /// Scope could not be determined.
    Unknown,
}

/// Shape of the final output produced by the agent for a trace turn.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    /// Final artifact is a single function body.
    FunctionOnly,
    /// Final artifact is a unified diff / patch.
    UnifiedDiff,
    /// Final artifact is a full file body.
    FullFile,
    /// Final artifact was achieved via tool invocation (e.g. edit_file
    /// calls) without an inline code block in the response.
    ToolExecution,
    /// Final artifact is prose / explanation only.
    Explanation,
    /// Could not determine.
    Unknown,
}

/// Compact summary row used when listing recent traces.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraceSummary {
    pub trace_id: String,
    pub run_hash: String,
    pub timestamp: String,
    pub prompt_preview: String,
    pub task_phase: TaskPhase,
    pub task_scope: TaskScope,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub target_files: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub target_symbols: Vec<String>,
}

/// Full task structure for replay / eval-case generation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskStructure {
    pub trace_id: String,
    pub run_hash: String,
    pub task_phase: TaskPhase,
    pub task_scope: TaskScope,
    pub final_artifact_type: ArtifactType,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub target_files: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub target_symbols: Vec<String>,
    pub task_goal: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub verification_actions: Vec<VerificationAction>,
}

/// Target file + function context to feed a replay/eval prompt.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetContext {
    pub trace_id: String,
    pub run_hash: String,
    pub task_scope: TaskScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_symbol: Option<String>,
    /// Scoped snippet: the function body when `task_scope == single_function`,
    /// otherwise the full file content (if known), otherwise `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scoped_snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// The final artifact produced by the agent in a trace.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FinalArtifact {
    pub trace_id: String,
    pub run_hash: String,
    pub artifact_type: ArtifactType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_function_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_file_snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_patch_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_symbol: Option<String>,
}

/// Semantic summaries of what the agent changed, preserved, and restored.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeSemantics {
    pub trace_id: String,
    pub run_hash: String,
    pub changed_construct_summary: String,
    pub preserved_construct_summary: String,
    pub restored_behavior_summary: String,
}

/// A single verification action (test/build/demo command).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerificationAction {
    /// Human description (e.g. "cargo test -p morph-core").
    pub action: String,
    /// "command" (shell/CLI), "tool" (agent tool), or "text" (mentioned
    /// in response but not executed as a traceable tool call).
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_output: Option<String>,
}

/// Full verification-step detail bundle for one trace.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerificationSteps {
    pub trace_id: String,
    pub run_hash: String,
    pub verification_actions: Vec<VerificationAction>,
}

// ---------- Classification heuristics ----------

fn classify_task_phase(prompt: &str, response: &str) -> TaskPhase {
    let p = prompt.to_ascii_lowercase();
    if contains_any(&p, &["fix ", "bug", "broken", "regress", "crash", "not working", "doesn't work", "incorrect", "wrong ", "error"]) {
        return TaskPhase::FixBug;
    }
    if contains_any(&p, &["implement", "create ", "add ", "build ", "new ", "write ", "scaffold", "generate"]) {
        return TaskPhase::CreateCode;
    }
    if contains_any(&p, &["refactor", "rename", "rewrite", "modify", "update ", "change ", "extend ", "replace"]) {
        return TaskPhase::ModifyCode;
    }
    if contains_any(&p, &["why is", "why does", "explain", "describe", "what does", "how does", "walk me through", "diagnose"]) {
        return TaskPhase::Diagnose;
    }
    if contains_any(&p, &["run test", "run the tests", "verify", "check ", "make sure", "sanity check"]) {
        return TaskPhase::Verify;
    }

    // Fallbacks based on response shape.
    if response.trim().is_empty() {
        return TaskPhase::Other;
    }
    // If the response is pure prose with no code, classify as Explain.
    if !response.contains("```") && !response.contains("def ") && !response.contains("fn ") {
        return TaskPhase::Explain;
    }
    TaskPhase::ModifyCode
}

fn contains_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

/// Pull file paths out of free-form text (extensions we recognize) plus
/// backtick-quoted tokens that look like paths.
fn extract_file_paths_from_text(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    // Backticks `path/to/file.ext`
    for chunk in text.split('`').skip(1).step_by(2) {
        if looks_like_path(chunk) && !seen.contains(chunk) {
            seen.insert(chunk.to_string());
            out.push(chunk.to_string());
        }
    }

    // Bare token matches with known extensions.
    for token in text.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '\'' | '"')) {
        let t = token.trim_matches(|c: char| matches!(c, '.' | ':' | '!' | '?'));
        if looks_like_path(t) && !seen.contains(t) {
            seen.insert(t.to_string());
            out.push(t.to_string());
        }
    }
    out
}

fn looks_like_path(s: &str) -> bool {
    const EXTS: &[&str] = &[
        ".py", ".pyi", ".rs", ".js", ".ts", ".tsx", ".jsx", ".go", ".java",
        ".kt", ".rb", ".c", ".cc", ".cpp", ".h", ".hpp", ".cs", ".swift",
        ".php", ".sh", ".toml", ".yaml", ".yml", ".json", ".md",
    ];
    if s.is_empty() || s.contains(' ') || s.len() > 200 {
        return false;
    }
    EXTS.iter().any(|e| s.to_ascii_lowercase().ends_with(e))
}

fn classify_task_scope(target_files: &[String], target_symbols: &[String]) -> TaskScope {
    let n = target_files.len();
    match n {
        0 => {
            if !target_symbols.is_empty() {
                TaskScope::SingleFunction
            } else {
                TaskScope::Unknown
            }
        }
        1 => {
            if !target_symbols.is_empty() && target_symbols.len() <= 2 {
                TaskScope::SingleFunction
            } else {
                TaskScope::SingleFile
            }
        }
        2..=5 => TaskScope::MultiFile,
        _ => TaskScope::Broad,
    }
}

/// Extract the final response code block if it looks like a function.
fn classify_artifact_type(step: &TapStep) -> ArtifactType {
    let response = &step.response;

    if is_unified_diff(response) {
        return ArtifactType::UnifiedDiff;
    }

    let blocks = extract_code_blocks(response);
    if !blocks.is_empty() {
        // Look for single function definition.
        if blocks.iter().any(|b| is_single_function_block(b)) {
            return ArtifactType::FunctionOnly;
        }
        return ArtifactType::FullFile;
    }

    if !step.tool_calls.is_empty() || !step.file_edits.is_empty() {
        return ArtifactType::ToolExecution;
    }

    if !response.trim().is_empty() {
        return ArtifactType::Explanation;
    }

    ArtifactType::Unknown
}

fn is_unified_diff(text: &str) -> bool {
    (text.contains("\n--- ") && text.contains("\n+++ "))
        || text.contains("@@ -") && text.contains(" +")
}

fn extract_code_blocks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    let mut current = String::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if in_block {
                out.push(std::mem::take(&mut current));
                in_block = false;
            } else {
                in_block = true;
            }
            continue;
        }
        if in_block {
            current.push_str(line);
            current.push('\n');
        }
    }
    out
}

fn is_single_function_block(body: &str) -> bool {
    let py = PythonLanguageAdapter::new();
    let syms = py.extract_symbols(body);
    let funcs: Vec<_> = syms
        .iter()
        .filter(|s| s.kind == "function" || s.kind == "async_function")
        .collect();
    let classes: Vec<_> = syms.iter().filter(|s| s.kind == "class").collect();
    funcs.len() == 1 && classes.is_empty()
}

// ---------- Verification extraction ----------

/// Keywords that mark a shell/tool action as a verification step.
const VERIFY_CMD_PREFIXES: &[&str] = &[
    "cargo test",
    "cargo build",
    "cargo check",
    "cargo run",
    "pytest",
    "python -m pytest",
    "python -m unittest",
    "npm test",
    "npm run test",
    "npm run build",
    "yarn test",
    "pnpm test",
    "go test",
    "go build",
    "mvn test",
    "gradle test",
    "bundle exec rspec",
    "rspec",
    "./gradlew test",
    "make test",
];

fn is_verification_command(cmd: &str) -> bool {
    let c = cmd.trim().to_ascii_lowercase();
    VERIFY_CMD_PREFIXES.iter().any(|p| c.starts_with(p))
}

fn extract_verification_actions(task: &TapTask) -> Vec<VerificationAction> {
    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for step in &task.steps {
        for tc in &step.tool_calls {
            let cmd = tc
                .input
                .clone()
                .unwrap_or_else(|| tc.name.clone().unwrap_or_default());
            if cmd.is_empty() {
                continue;
            }
            let name = tc.name.as_deref().unwrap_or("");
            let cmd_lower = cmd.to_ascii_lowercase();
            let is_shell = matches!(
                name,
                "shell" | "run_command" | "run_shell_command" | "bash" | "execute" | "run"
            );
            let looks_like_cmd = is_shell || is_verification_command(&cmd_lower);
            if !looks_like_cmd {
                continue;
            }
            if !is_verification_command(&cmd_lower) {
                continue;
            }
            let key = cmd.clone();
            if seen.insert(key.clone()) {
                let summary = tc.output.as_ref().map(|o| {
                    if o.len() > 400 {
                        format!("{}...", &o[..o.floor_char_boundary(400)])
                    } else {
                        o.clone()
                    }
                });
                let exit = if tc.error.is_some() { Some("error".into()) } else { None };
                out.push(VerificationAction {
                    action: cmd,
                    kind: "command".into(),
                    exit_status: exit,
                    summary_output: summary,
                });
            }
        }

        // Also look for commands mentioned inline in the response (e.g.
        // "Run `cargo test` to verify"). We mark those as kind="text".
        for backtick in step.response.split('`').skip(1).step_by(2) {
            if is_verification_command(backtick) && !seen.contains(backtick) {
                seen.insert(backtick.to_string());
                out.push(VerificationAction {
                    action: backtick.to_string(),
                    kind: "text".into(),
                    exit_status: None,
                    summary_output: None,
                });
            }
        }
    }
    out
}

// ---------- Target file / symbol detection ----------

/// Compose the collection of target files referenced across the trace:
/// file_reads, file_edits, and paths mentioned in the user prompt.
fn collect_target_files(task: &TapTask) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for step in &task.steps {
        for fe in &step.file_edits {
            if let Some(p) = fe.path.as_ref() {
                if seen.insert(p.clone()) {
                    files.push(p.clone());
                }
            }
        }
        for fr in &step.file_reads {
            if let Some(p) = fr.path.as_ref() {
                if seen.insert(p.clone()) {
                    files.push(p.clone());
                }
            }
        }
        for p in extract_file_paths_from_text(&step.prompt) {
            if seen.insert(p.clone()) {
                files.push(p);
            }
        }
    }
    files
}

/// Best-effort target symbols given file contents (if available) + prompt
/// hints. Uses [`LanguageAdapter::detect_target_symbol`] on each file and
/// falls back to `extract_symbol_references` on the prompt.
fn collect_target_symbols(task: &TapTask, target_files: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    let combined_prompt = task
        .steps
        .iter()
        .map(|s| s.prompt.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    for file in target_files {
        let adapter = match adapter_for_filename(file) {
            Some(a) => a,
            None => continue,
        };
        let content = find_file_content(task, file);
        if let Some(src) = content {
            if let Some(tgt) = adapter.detect_target_symbol(&src, Some(&combined_prompt)) {
                if seen.insert(tgt.clone()) {
                    out.push(tgt);
                }
            }
        }
    }

    // Fallback: symbol references pulled from the prompt itself (adapter
    // picked by first known target file, else Python).
    if out.is_empty() {
        let adapter: Box<dyn LanguageAdapter> = target_files
            .iter()
            .find_map(|f| adapter_for_filename(f))
            .unwrap_or_else(|| Box::new(PythonLanguageAdapter::new()));
        for sym in adapter.extract_symbol_references(&combined_prompt) {
            if seen.insert(sym.clone()) {
                out.push(sym);
            }
        }
    }
    out
}

fn find_file_content(task: &TapTask, path: &str) -> Option<String> {
    // Later file_edits take precedence over earlier file_reads because
    // the final artifact reflects the agent's latest write.
    let mut latest: Option<String> = None;
    for step in &task.steps {
        for fr in &step.file_reads {
            if fr.path.as_deref() == Some(path) {
                if let Some(c) = &fr.content {
                    latest = Some(c.clone());
                }
            }
        }
        for fe in &step.file_edits {
            if fe.path.as_deref() == Some(path) {
                if let Some(c) = &fe.content {
                    latest = Some(c.clone());
                }
            }
        }
    }
    latest
}

// ---------- Final artifact extraction ----------

fn extract_final_artifact(task: &TapTask) -> FinalArtifact {
    let empty_step = TapStep {
        prompt: String::new(),
        response: String::new(),
        tool_calls: Vec::new(),
        file_reads: Vec::new(),
        file_edits: Vec::new(),
        events: Vec::new(),
    };
    let last_step = task.steps.last().unwrap_or(&empty_step);
    let artifact_type = classify_artifact_type(last_step);

    let target_files = collect_target_files(task);
    let target_symbols = collect_target_symbols(task, &target_files);

    let related_file = target_files.first().cloned();
    let related_symbol = target_symbols.first().cloned();

    let mut final_function_text = None;
    let mut final_file_snippet = None;
    let mut final_patch_summary = None;

    match artifact_type {
        ArtifactType::FunctionOnly => {
            // Pick the first code block that looks like a single function.
            for block in extract_code_blocks(&last_step.response) {
                if is_single_function_block(&block) {
                    final_function_text = Some(block);
                    break;
                }
            }
            // Also try slicing from file_edit content using target_symbol.
            if final_function_text.is_none() {
                if let (Some(path), Some(symbol)) = (related_file.as_deref(), related_symbol.as_deref()) {
                    if let Some(src) = find_file_content(task, path) {
                        if let Some(adapter) = adapter_for_filename(path) {
                            final_function_text = adapter.slice_symbol(&src, symbol);
                        }
                    }
                }
            }
        }
        ArtifactType::UnifiedDiff => {
            // Extract the first diff-like region from the response.
            let diff = extract_diff_region(&last_step.response);
            final_patch_summary = Some(diff);
        }
        ArtifactType::FullFile => {
            // First code block.
            let blocks = extract_code_blocks(&last_step.response);
            final_file_snippet = blocks.into_iter().next();
        }
        ArtifactType::ToolExecution => {
            // Prefer latest edit content.
            if let Some(path) = &related_file {
                final_file_snippet = find_file_content(task, path);
            }
        }
        ArtifactType::Explanation | ArtifactType::Unknown => {}
    }

    FinalArtifact {
        trace_id: task.trace_hash.clone(),
        run_hash: task.run_hash.clone(),
        artifact_type,
        final_function_text,
        final_file_snippet,
        final_patch_summary,
        related_file,
        related_symbol,
    }
}

fn extract_diff_region(text: &str) -> String {
    // Return the first contiguous chunk of lines containing diff markers.
    let mut out = Vec::new();
    let mut started = false;
    for line in text.lines() {
        let is_diff_line = line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("@@ ")
            || line.starts_with('+')
            || line.starts_with('-');
        if is_diff_line {
            started = true;
            out.push(line);
        } else if started && line.trim().is_empty() {
            out.push(line);
        } else if started {
            break;
        }
    }
    out.join("\n")
}

// ---------- Semantic summaries ----------

fn first_sentence(text: &str) -> String {
    let t = text.trim();
    if t.is_empty() {
        return String::new();
    }
    let end = t
        .find(|c: char| matches!(c, '.' | '\n' | '?' | '!'))
        .map(|i| i + 1)
        .unwrap_or(t.len());
    t[..end.min(t.len())].trim().to_string()
}

fn extract_change_semantics(task: &TapTask) -> ChangeSemantics {
    let prompt = task
        .steps
        .iter()
        .map(|s| s.prompt.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let response = task
        .steps
        .last()
        .map(|s| s.response.as_str())
        .unwrap_or("");

    let changed = if let Some(chunk) = find_after(&prompt, "change") {
        format!("Change: {}", first_sentence(chunk))
    } else if let Some(chunk) = find_after(&prompt, "fix") {
        format!("Fix: {}", first_sentence(chunk))
    } else if let Some(chunk) = find_after(&prompt, "add") {
        format!("Add: {}", first_sentence(chunk))
    } else if !prompt.trim().is_empty() {
        format!("Requested: {}", first_sentence(&prompt))
    } else {
        String::new()
    };

    let preserved = if let Some(chunk) = find_after_ci(&prompt, "without changing") {
        format!("Preserve: {}", first_sentence(chunk))
    } else if let Some(chunk) = find_after_ci(&prompt, "don't touch") {
        format!("Preserve: {}", first_sentence(chunk))
    } else if let Some(chunk) = find_after_ci(&prompt, "keep") {
        format!("Preserve: {}", first_sentence(chunk))
    } else {
        String::new()
    };

    let restored = if let Some(chunk) = find_after_ci(&prompt, "used to work") {
        format!("Restore: {}", first_sentence(chunk))
    } else if let Some(chunk) = find_after_ci(&prompt, "regress") {
        format!("Restore: {}", first_sentence(chunk))
    } else if let Some(chunk) = find_after_ci(response, "now works") {
        format!("Restored: {}", first_sentence(chunk))
    } else {
        String::new()
    };

    ChangeSemantics {
        trace_id: task.trace_hash.clone(),
        run_hash: task.run_hash.clone(),
        changed_construct_summary: changed,
        preserved_construct_summary: preserved,
        restored_behavior_summary: restored,
    }
}

fn find_after<'a>(text: &'a str, needle: &str) -> Option<&'a str> {
    let i = text.find(needle)?;
    Some(&text[i + needle.len()..])
}

fn find_after_ci<'a>(text: &'a str, needle: &str) -> Option<&'a str> {
    let lower = text.to_ascii_lowercase();
    let i = lower.find(&needle.to_ascii_lowercase())?;
    Some(&text[i + needle.len()..])
}

// ---------- Public API ----------

/// Compute a compact [`TraceSummary`] for a run. Heuristic.
pub fn summarize_trace(store: &dyn Store, run_hash: &Hash) -> Result<TraceSummary, MorphError> {
    let task = extract_task(store, run_hash)?;
    let structure = derive_task_structure(&task);
    let prompt = task
        .steps
        .first()
        .map(|s| s.prompt.as_str())
        .unwrap_or("");
    let preview = preview_text(prompt, 160);
    Ok(TraceSummary {
        trace_id: task.trace_hash.clone(),
        run_hash: task.run_hash.clone(),
        timestamp: task.timestamp.clone(),
        prompt_preview: preview,
        task_phase: structure.task_phase,
        task_scope: structure.task_scope,
        target_files: structure.target_files,
        target_symbols: structure.target_symbols,
    })
}

fn preview_text(s: &str, max: usize) -> String {
    let one_line = s.replace('\n', " ").replace('\r', " ");
    let trimmed = one_line.trim();
    if trimmed.len() <= max {
        trimmed.to_string()
    } else {
        let end = trimmed.floor_char_boundary(max);
        format!("{}...", &trimmed[..end])
    }
}

/// Return a [`TaskStructure`] for the given run.
pub fn task_structure(store: &dyn Store, run_hash: &Hash) -> Result<TaskStructure, MorphError> {
    let task = extract_task(store, run_hash)?;
    Ok(derive_task_structure(&task))
}

fn derive_task_structure(task: &TapTask) -> TaskStructure {
    let first = task.steps.first();
    let last = task.steps.last();
    let prompt = first.map(|s| s.prompt.as_str()).unwrap_or("");
    let response = last.map(|s| s.response.as_str()).unwrap_or("");

    let phase = classify_task_phase(prompt, response);
    let target_files = collect_target_files(task);
    let target_symbols = collect_target_symbols(task, &target_files);
    let scope = classify_task_scope(&target_files, &target_symbols);
    let empty_step = TapStep {
        prompt: String::new(),
        response: String::new(),
        tool_calls: Vec::new(),
        file_reads: Vec::new(),
        file_edits: Vec::new(),
        events: Vec::new(),
    };
    let artifact_type = classify_artifact_type(last.unwrap_or(&empty_step));

    let task_goal = first_sentence(prompt);
    let verification_actions = extract_verification_actions(task);

    TaskStructure {
        trace_id: task.trace_hash.clone(),
        run_hash: task.run_hash.clone(),
        task_phase: phase,
        task_scope: scope,
        final_artifact_type: artifact_type,
        target_files,
        target_symbols,
        task_goal,
        verification_actions,
    }
}

/// Target file/function context for a run.
pub fn target_context(store: &dyn Store, run_hash: &Hash) -> Result<TargetContext, MorphError> {
    let task = extract_task(store, run_hash)?;
    let target_files = collect_target_files(&task);
    let target_symbols = collect_target_symbols(&task, &target_files);
    let scope = classify_task_scope(&target_files, &target_symbols);

    let target_file = target_files.first().cloned();
    let target_symbol = target_symbols.first().cloned();

    let mut scoped_snippet: Option<String> = None;
    let mut language: Option<String> = None;

    if let Some(path) = target_file.as_deref() {
        let content = find_file_content(&task, path);
        if let Some(adapter) = adapter_for_filename(path) {
            language = Some(adapter.name().to_string());
            if matches!(scope, TaskScope::SingleFunction) {
                if let (Some(src), Some(sym)) = (content.as_deref(), target_symbol.as_deref()) {
                    scoped_snippet = adapter.slice_symbol(src, sym);
                }
            }
        }
        if scoped_snippet.is_none() {
            scoped_snippet = content;
        }
    }

    Ok(TargetContext {
        trace_id: task.trace_hash.clone(),
        run_hash: task.run_hash.clone(),
        task_scope: scope,
        target_file,
        target_symbol,
        scoped_snippet,
        language,
    })
}

/// Final artifact for a run.
pub fn final_artifact(store: &dyn Store, run_hash: &Hash) -> Result<FinalArtifact, MorphError> {
    let task = extract_task(store, run_hash)?;
    Ok(extract_final_artifact(&task))
}

/// Change / preserved / restored semantic summaries for a run.
pub fn change_semantics(store: &dyn Store, run_hash: &Hash) -> Result<ChangeSemantics, MorphError> {
    let task = extract_task(store, run_hash)?;
    Ok(extract_change_semantics(&task))
}

/// Verification actions for a run (commands, tool executions, mentions).
pub fn verification_steps(store: &dyn Store, run_hash: &Hash) -> Result<VerificationSteps, MorphError> {
    let task = extract_task(store, run_hash)?;
    let actions = extract_verification_actions(&task);
    Ok(VerificationSteps {
        trace_id: task.trace_hash.clone(),
        run_hash: task.run_hash.clone(),
        verification_actions: actions,
    })
}

/// Return up to `limit` trace summaries, newest-first by timestamp.
pub fn recent_trace_summaries(
    store: &dyn Store,
    limit: usize,
) -> Result<Vec<TraceSummary>, MorphError> {
    let run_hashes = store.list(ObjectType::Run)?;
    let mut summaries: Vec<TraceSummary> = Vec::new();
    for h in &run_hashes {
        match summarize_trace(store, h) {
            Ok(s) => summaries.push(s),
            Err(_) => continue,
        }
    }
    summaries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    if summaries.len() > limit {
        summaries.truncate(limit);
    }
    Ok(summaries)
}

/// Helper for tools that need to resolve a trace hash back to a run hash.
/// We pick the most recent run whose trace matches.
pub fn find_run_by_trace(store: &dyn Store, trace_hash: &Hash) -> Result<Option<Hash>, MorphError> {
    let trace_hex = trace_hash.to_string();
    let runs = store.list(ObjectType::Run)?;
    for rh in &runs {
        if let MorphObject::Run(run) = store.get(rh)? {
            if run.trace == trace_hex {
                return Ok(Some(rh.clone()));
            }
        }
    }
    Ok(None)
}

// trace-hash-only convenience overloads so callers can pass either a run
// hash or a trace hash; the structured layer is keyed on the run.
#[allow(dead_code)]
fn resolve_run_hash(store: &dyn Store, hash: &Hash) -> Result<Hash, MorphError> {
    match store.get(hash) {
        Ok(MorphObject::Run(_)) => Ok(hash.clone()),
        Ok(MorphObject::Trace(_)) => find_run_by_trace(store, hash)?
            .ok_or_else(|| MorphError::Serialization(format!("no run points to trace {}", hash))),
        Ok(_) => Err(MorphError::Serialization(format!(
            "hash {} is neither a Run nor a Trace",
            hash
        ))),
        Err(e) => Err(e),
    }
}

/// Unused-but-exported for completeness: does a trace have structured
/// events (tool/file kinds)?
#[allow(dead_code)]
pub fn trace_has_structured_events(trace: &Trace) -> bool {
    trace.events.iter().any(|e| {
        matches!(
            e.kind.as_str(),
            "tool_call" | "tool_use" | "function_call" | "tool_result"
                | "tool_output" | "function_result" | "file_read" | "read_file"
                | "file_edit" | "edit_file" | "write_file" | "file_write"
        )
    })
}

/// Access to the payload map for trace lookups via structured metadata.
#[allow(dead_code)]
pub(crate) fn payload_keys(map: &BTreeMap<String, serde_json::Value>) -> Vec<String> {
    map.keys().cloned().collect()
}

// ---------- Tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::*;
    use crate::store::Store;

    fn setup_store() -> (tempfile::TempDir, Box<dyn Store>) {
        let dir = tempfile::tempdir().unwrap();
        crate::repo::init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();
        (dir, store)
    }

    fn event(seq: u64, kind: &str, payload: BTreeMap<String, serde_json::Value>) -> TraceEvent {
        TraceEvent {
            id: format!("evt_{}", seq),
            seq,
            ts: format!("2026-04-17T12:00:{:02}+00:00", seq.min(59)),
            kind: kind.to_string(),
            payload,
        }
    }

    fn text_payload(text: &str) -> BTreeMap<String, serde_json::Value> {
        let mut p = BTreeMap::new();
        p.insert("text".into(), serde_json::json!(text));
        p
    }

    fn file_payload(path: &str, content: &str) -> BTreeMap<String, serde_json::Value> {
        let mut p = BTreeMap::new();
        p.insert("path".into(), serde_json::json!(path));
        p.insert("content".into(), serde_json::json!(content));
        p
    }

    fn tool_call_payload(name: &str, input: &str) -> BTreeMap<String, serde_json::Value> {
        let mut p = BTreeMap::new();
        p.insert("name".into(), serde_json::json!(name));
        p.insert("input".into(), serde_json::json!(input));
        p
    }

    fn store_run(store: &dyn Store, events: Vec<TraceEvent>, model: &str) -> Hash {
        let trace = MorphObject::Trace(Trace { events });
        let trace_hash = store.put(&trace).unwrap();
        let pipeline_hash = store.put(&crate::identity::identity_pipeline()).unwrap();
        let run = MorphObject::Run(Run {
            pipeline: pipeline_hash.to_string(),
            commit: None,
            environment: RunEnvironment {
                model: model.into(),
                version: "1.0".into(),
                parameters: BTreeMap::new(),
                toolchain: BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: BTreeMap::new(),
            trace: trace_hash.to_string(),
            agent: AgentInfo {
                id: "cursor".into(),
                version: "1.0".into(),
                policy: None,
                instance_id: None,
            },
            contributors: None,
            morph_version: None,
        });
        store.put(&run).unwrap()
    }

    // --- pocket-tasks regression fixtures ---

    const POCKET_TASKS_V1: &str = "\
def list_tasks(db):
    rows = db.fetch('tasks')
    return [r.title for r in rows]


def add_task(db, title):
    db.insert('tasks', title)
";

    const POCKET_TASKS_BUGGY: &str = "\
def list_tasks(db):
    rows = db.fetch('tasks')
    return [r.title for r in rows[1:]]


def add_task(db, title):
    db.insert('tasks', title)
";

    const POCKET_TASKS_FIXED: &str = "\
def list_tasks(db):
    rows = db.fetch('tasks')
    return [r.title for r in rows]


def add_task(db, title):
    db.insert('tasks', title)
";

    fn pocket_tasks_create_events() -> Vec<TraceEvent> {
        vec![
            event(0, "user", text_payload("Create a pocket_tasks/main.py with list_tasks and add_task functions.")),
            event(
                1,
                "assistant",
                text_payload(
                    "Here is the initial implementation:\n\n```python\ndef list_tasks(db):\n    rows = db.fetch('tasks')\n    return [r.title for r in rows]\n\ndef add_task(db, title):\n    db.insert('tasks', title)\n```",
                ),
            ),
            event(2, "file_edit", file_payload("pocket_tasks/main.py", POCKET_TASKS_V1)),
        ]
    }

    fn pocket_tasks_bug_events() -> Vec<TraceEvent> {
        vec![
            event(0, "user", text_payload("Update `list_tasks` in `pocket_tasks/main.py` to skip the first task")),
            event(1, "assistant", text_payload("I'll update `list_tasks` to skip the first row.")),
            event(2, "file_edit", file_payload("pocket_tasks/main.py", POCKET_TASKS_BUGGY)),
        ]
    }

    fn pocket_tasks_fix_events() -> Vec<TraceEvent> {
        vec![
            event(
                0,
                "user",
                text_payload("Fix the bug in `list_tasks` in `pocket_tasks/main.py` — it should not skip the first task."),
            ),
            event(
                1,
                "assistant",
                text_payload(
                    "Fixed. The function now returns all rows:\n\n```python\ndef list_tasks(db):\n    rows = db.fetch('tasks')\n    return [r.title for r in rows]\n```\n\nRun `pytest pocket_tasks/` to verify.",
                ),
            ),
            event(2, "file_read", file_payload("pocket_tasks/main.py", POCKET_TASKS_BUGGY)),
            event(3, "file_edit", file_payload("pocket_tasks/main.py", POCKET_TASKS_FIXED)),
            event(4, "tool_call", tool_call_payload("shell", "pytest pocket_tasks/")),
            event(
                5,
                "tool_result",
                {
                    let mut p = BTreeMap::new();
                    p.insert("output".into(), serde_json::json!("====== 3 passed ======"));
                    p
                },
            ),
        ]
    }

    // --- classification tests ---

    #[test]
    fn task_phase_fix_bug_detected() {
        assert_eq!(
            classify_task_phase("Fix the bug in list_tasks", ""),
            TaskPhase::FixBug
        );
        assert_eq!(
            classify_task_phase("this function is broken", ""),
            TaskPhase::FixBug
        );
    }

    #[test]
    fn task_phase_create_code_detected() {
        assert_eq!(
            classify_task_phase("Implement a new cache module", ""),
            TaskPhase::CreateCode
        );
    }

    #[test]
    fn task_phase_modify_code_detected() {
        assert_eq!(
            classify_task_phase("Refactor list_tasks to use a generator", ""),
            TaskPhase::ModifyCode
        );
    }

    #[test]
    fn task_phase_diagnose_detected() {
        assert_eq!(
            classify_task_phase("Why does this crash on empty input?", "some text"),
            TaskPhase::FixBug // "crash" triggers fix_bug first
        );
        assert_eq!(
            classify_task_phase("Explain how the scheduler works.", "some text"),
            TaskPhase::Diagnose
        );
    }

    #[test]
    fn task_phase_verify_detected() {
        assert_eq!(
            classify_task_phase("Run the tests and confirm they all pass", "..."),
            TaskPhase::Verify
        );
    }

    #[test]
    fn task_scope_single_function() {
        let files = vec!["a.py".to_string()];
        let syms = vec!["list_tasks".to_string()];
        assert_eq!(classify_task_scope(&files, &syms), TaskScope::SingleFunction);
    }

    #[test]
    fn task_scope_single_file() {
        let files = vec!["a.py".to_string()];
        let syms: Vec<String> = vec![];
        assert_eq!(classify_task_scope(&files, &syms), TaskScope::SingleFile);
    }

    #[test]
    fn task_scope_multi_file() {
        let files = vec!["a.py".into(), "b.py".into(), "c.py".into()];
        let syms: Vec<String> = vec![];
        assert_eq!(classify_task_scope(&files, &syms), TaskScope::MultiFile);
    }

    #[test]
    fn task_scope_broad() {
        let files: Vec<String> = (0..10).map(|i| format!("f{}.py", i)).collect();
        assert_eq!(classify_task_scope(&files, &vec![]), TaskScope::Broad);
    }

    #[test]
    fn target_symbol_extracted_from_prompt_plus_source() {
        let (_dir, store) = setup_store();
        let run_hash = store_run(store.as_ref(), pocket_tasks_fix_events(), "gpt-4");
        let structure = task_structure(store.as_ref(), &run_hash).unwrap();
        assert!(
            structure.target_symbols.iter().any(|s| s == "list_tasks"),
            "expected list_tasks in {:?}",
            structure.target_symbols
        );
        assert_eq!(structure.target_files, vec!["pocket_tasks/main.py".to_string()]);
        assert_eq!(structure.task_phase, TaskPhase::FixBug);
        assert_eq!(structure.task_scope, TaskScope::SingleFunction);
        assert_eq!(structure.final_artifact_type, ArtifactType::FunctionOnly);
    }

    #[test]
    fn target_context_returns_function_slice_for_single_function_scope() {
        let (_dir, store) = setup_store();
        let run_hash = store_run(store.as_ref(), pocket_tasks_fix_events(), "gpt-4");
        let ctx = target_context(store.as_ref(), &run_hash).unwrap();

        assert_eq!(ctx.task_scope, TaskScope::SingleFunction);
        assert_eq!(ctx.target_file.as_deref(), Some("pocket_tasks/main.py"));
        assert_eq!(ctx.target_symbol.as_deref(), Some("list_tasks"));
        assert_eq!(ctx.language.as_deref(), Some("python"));
        let snippet = ctx.scoped_snippet.expect("snippet");
        assert!(snippet.contains("def list_tasks(db):"));
        assert!(!snippet.contains("def add_task"));
    }

    #[test]
    fn final_artifact_function_only_from_response() {
        let (_dir, store) = setup_store();
        let run_hash = store_run(store.as_ref(), pocket_tasks_fix_events(), "gpt-4");
        let artifact = final_artifact(store.as_ref(), &run_hash).unwrap();
        assert_eq!(artifact.artifact_type, ArtifactType::FunctionOnly);
        let text = artifact.final_function_text.expect("function text");
        assert!(text.contains("def list_tasks(db):"));
        assert_eq!(artifact.related_symbol.as_deref(), Some("list_tasks"));
        assert_eq!(artifact.related_file.as_deref(), Some("pocket_tasks/main.py"));
    }

    #[test]
    fn final_artifact_create_code_uses_edit_content() {
        let (_dir, store) = setup_store();
        let run_hash = store_run(store.as_ref(), pocket_tasks_create_events(), "gpt-4");
        let structure = task_structure(store.as_ref(), &run_hash).unwrap();
        assert_eq!(structure.task_phase, TaskPhase::CreateCode);
        let artifact = final_artifact(store.as_ref(), &run_hash).unwrap();
        // Multiple functions in the block → full_file, not function_only.
        assert_eq!(artifact.artifact_type, ArtifactType::FullFile);
        assert!(artifact.final_file_snippet.expect("snippet").contains("def list_tasks"));
    }

    #[test]
    fn verification_actions_found() {
        let (_dir, store) = setup_store();
        let run_hash = store_run(store.as_ref(), pocket_tasks_fix_events(), "gpt-4");
        let v = verification_steps(store.as_ref(), &run_hash).unwrap();
        assert!(
            v.verification_actions.iter().any(|a| a.action.starts_with("pytest")),
            "got {:?}",
            v.verification_actions
        );
        // should have kind=command from tool_call
        assert!(v
            .verification_actions
            .iter()
            .any(|a| a.kind == "command" && a.summary_output.is_some()));
    }

    #[test]
    fn change_semantics_populated() {
        let (_dir, store) = setup_store();
        let run_hash = store_run(store.as_ref(), pocket_tasks_fix_events(), "gpt-4");
        let sem = change_semantics(store.as_ref(), &run_hash).unwrap();
        assert!(
            sem.changed_construct_summary.to_ascii_lowercase().contains("fix")
                || sem.changed_construct_summary.to_ascii_lowercase().contains("list_tasks"),
            "got: {}",
            sem.changed_construct_summary
        );
    }

    #[test]
    fn recent_trace_summaries_ordered_newest_first() {
        let (_dir, store) = setup_store();
        let old = vec![
            TraceEvent { id: "a".into(), seq: 0, ts: "2020-01-01T00:00:00Z".into(), kind: "user".into(), payload: text_payload("old one") },
            TraceEvent { id: "b".into(), seq: 1, ts: "2020-01-01T00:00:01Z".into(), kind: "assistant".into(), payload: text_payload("old response") },
        ];
        let new = vec![
            TraceEvent { id: "a".into(), seq: 0, ts: "2026-04-17T12:00:00Z".into(), kind: "user".into(), payload: text_payload("Fix the list_tasks bug") },
            TraceEvent { id: "b".into(), seq: 1, ts: "2026-04-17T12:00:01Z".into(), kind: "assistant".into(), payload: text_payload("done") },
        ];
        store_run(store.as_ref(), old, "gpt-4");
        store_run(store.as_ref(), new, "gpt-4");

        let sums = recent_trace_summaries(store.as_ref(), 10).unwrap();
        assert_eq!(sums.len(), 2);
        assert!(sums[0].timestamp > sums[1].timestamp);
        assert!(sums[0].prompt_preview.contains("list_tasks"));
    }

    #[test]
    fn unified_diff_detection() {
        assert!(is_unified_diff(
            "--- a/x.py\n+++ b/x.py\n@@ -1 +1 @@\n-foo\n+bar"
        ));
        assert!(!is_unified_diff("def foo(): pass"));
    }

    #[test]
    fn extract_code_blocks_basic() {
        let txt = "some text\n```python\ndef x():\n    pass\n```\nmore";
        let blocks = extract_code_blocks(txt);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("def x():"));
    }

    #[test]
    fn file_path_extraction() {
        let paths = extract_file_paths_from_text(
            "Please update `app/main.py` and check tests in tests/test_main.py",
        );
        assert!(paths.iter().any(|p| p == "app/main.py"));
        assert!(paths.iter().any(|p| p == "tests/test_main.py"));
    }

    #[test]
    fn task_goal_is_first_sentence() {
        let events = vec![
            event(
                0,
                "user",
                text_payload("Fix the bug in list_tasks. It skips the first task."),
            ),
            event(1, "assistant", text_payload("Fixed.")),
        ];
        let (_dir, store) = setup_store();
        let run_hash = store_run(store.as_ref(), events, "gpt-4");
        let s = task_structure(store.as_ref(), &run_hash).unwrap();
        assert_eq!(s.task_goal, "Fix the bug in list_tasks.");
    }

    #[test]
    fn trace_summary_populated() {
        let (_dir, store) = setup_store();
        let run_hash = store_run(store.as_ref(), pocket_tasks_fix_events(), "gpt-4");
        let summary = summarize_trace(store.as_ref(), &run_hash).unwrap();
        assert_eq!(summary.task_phase, TaskPhase::FixBug);
        assert_eq!(summary.task_scope, TaskScope::SingleFunction);
        assert!(summary.target_files.iter().any(|f| f == "pocket_tasks/main.py"));
        assert!(summary.target_symbols.iter().any(|s| s == "list_tasks"));
        assert!(summary.prompt_preview.contains("Fix the bug"));
    }

    #[test]
    fn non_run_hash_errors_cleanly() {
        let (_dir, store) = setup_store();
        // A blob is not a run.
        let blob = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({}) });
        let hash = store.put(&blob).unwrap();
        assert!(task_structure(store.as_ref(), &hash).is_err());
    }

    #[test]
    fn pocket_tasks_full_workflow_all_three_phases() {
        let (_dir, store) = setup_store();
        let create = store_run(store.as_ref(), pocket_tasks_create_events(), "gpt-4");
        let bug = store_run(store.as_ref(), pocket_tasks_bug_events(), "gpt-4");
        let fix = store_run(store.as_ref(), pocket_tasks_fix_events(), "gpt-4");

        let s_create = task_structure(store.as_ref(), &create).unwrap();
        let s_bug = task_structure(store.as_ref(), &bug).unwrap();
        let s_fix = task_structure(store.as_ref(), &fix).unwrap();

        assert_eq!(s_create.task_phase, TaskPhase::CreateCode);
        assert_eq!(s_bug.task_phase, TaskPhase::ModifyCode);
        assert_eq!(s_fix.task_phase, TaskPhase::FixBug);

        // fix task should be the single-function localized one
        assert_eq!(s_fix.task_scope, TaskScope::SingleFunction);
        assert!(s_fix.target_symbols.iter().any(|s| s == "list_tasks"));
    }
}
