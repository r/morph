//! Tap: trace extraction and transformation for evaluation.
//!
//! Reads Morph traces and runs from the store, groups events into coherent
//! tasks, and produces structured output suitable for evaluation frameworks.
//! All evaluation-specific logic lives here — Morph's recording layer stays
//! general-purpose.

use crate::objects::{MorphObject, Run, Trace, TraceEvent};
use crate::store::{MorphError, ObjectType, Store};
use crate::Hash;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A single step extracted from a trace: one user turn + the assistant's
/// response and any tool interactions that follow.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TapStep {
    pub prompt: String,
    pub response: String,
    pub tool_calls: Vec<TapToolCall>,
    pub file_reads: Vec<TapFileEvent>,
    pub file_edits: Vec<TapFileEvent>,
    pub events: Vec<TapEvent>,
}

/// Normalized representation of a trace event for export.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TapEvent {
    pub seq: u64,
    pub kind: String,
    pub ts: String,
    pub text: Option<String>,
    pub payload: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TapToolCall {
    pub name: Option<String>,
    pub input: Option<String>,
    pub output: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TapFileEvent {
    pub path: Option<String>,
    pub content: Option<String>,
}

/// Token usage data extracted from run environment parameters.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TapTokenUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

impl TapTokenUsage {
    fn from_parameters(params: &BTreeMap<String, serde_json::Value>) -> Self {
        let get = |key: &str| params.get(key).and_then(|v| v.as_u64());
        let input = get("input_tokens");
        let output = get("output_tokens");
        let total = get("total_tokens")
            .or_else(|| input.zip(output).map(|(i, o)| i + o));
        TapTokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: get("cache_read_tokens"),
            cache_write_tokens: get("cache_write_tokens"),
            total_tokens: total,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.input_tokens.is_none() && self.output_tokens.is_none() && self.total_tokens.is_none()
    }
}

/// A complete task extracted from one run: all steps with metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TapTask {
    pub run_hash: String,
    pub trace_hash: String,
    pub model: String,
    pub agent: String,
    pub agent_version: String,
    pub timestamp: String,
    pub steps: Vec<TapStep>,
    pub event_count: usize,
    pub step_count: usize,
    #[serde(default, skip_serializing_if = "TapTokenUsage::is_empty")]
    pub token_usage: TapTokenUsage,
}

/// Diagnostic report for a trace.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TapDiagnostic {
    pub run_hash: String,
    pub trace_hash: String,
    pub event_count: usize,
    pub event_kinds: BTreeMap<String, usize>,
    pub has_prompt: bool,
    pub has_response: bool,
    pub has_tool_calls: bool,
    pub has_file_reads: bool,
    pub has_file_edits: bool,
    pub has_tool_output: bool,
    pub prompt_count: usize,
    pub response_empty: bool,
    pub step_count: usize,
    pub issues: Vec<String>,
    pub prompt_lengths: Vec<usize>,
    pub response_lengths: Vec<usize>,
    pub model: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "TapTokenUsage::is_empty")]
    pub token_usage: TapTokenUsage,
}

/// Summary statistics across all runs in a repo.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TapSummary {
    pub total_runs: usize,
    pub total_traces: usize,
    pub total_events: usize,
    pub event_kind_counts: BTreeMap<String, usize>,
    pub model_counts: BTreeMap<String, usize>,
    pub agent_counts: BTreeMap<String, usize>,
    pub runs_with_metrics: usize,
    pub multi_step_runs: usize,
    pub empty_response_runs: usize,
    pub issues: Vec<String>,
}

/// Detailed statistics for a single trace (for debugging/inspection).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TapTraceStats {
    pub trace_hash: String,
    pub event_count: usize,
    pub event_kinds: BTreeMap<String, usize>,
    pub normalized_kinds: BTreeMap<String, usize>,
    pub has_structured_events: bool,
    pub prompt_lengths: Vec<usize>,
    pub response_lengths: Vec<usize>,
    pub timestamp_range: Option<(String, String)>,
    pub payload_keys: BTreeMap<String, usize>,
}

/// Filter criteria for selecting runs.
#[derive(Clone, Debug, Default)]
pub struct TapFilter {
    pub model: Option<String>,
    pub agent: Option<String>,
    pub min_steps: Option<usize>,
    pub has_tool_calls: Option<bool>,
}

fn normalize_kind(kind: &str) -> &str {
    match kind {
        "user" | "prompt" => "prompt",
        "assistant" | "response" => "response",
        _ => kind,
    }
}

fn is_prompt(kind: &str) -> bool {
    normalize_kind(kind) == "prompt"
}

fn is_response(kind: &str) -> bool {
    normalize_kind(kind) == "response"
}

fn is_tool_call(kind: &str) -> bool {
    matches!(kind, "tool_call" | "tool_use" | "function_call")
}

fn is_tool_result(kind: &str) -> bool {
    matches!(kind, "tool_result" | "tool_output" | "function_result")
}

fn is_file_read(kind: &str) -> bool {
    matches!(kind, "file_read" | "read_file")
}

fn is_file_edit(kind: &str) -> bool {
    matches!(kind, "file_edit" | "edit_file" | "write_file" | "file_write")
}

fn event_text(event: &TraceEvent) -> Option<&str> {
    event.payload.get("text").and_then(|v| v.as_str())
}

fn extract_tap_event(event: &TraceEvent) -> TapEvent {
    TapEvent {
        seq: event.seq,
        kind: event.kind.clone(),
        ts: event.ts.clone(),
        text: event_text(event).map(|s| s.to_string()),
        payload: event.payload.clone(),
    }
}

fn extract_tool_call(event: &TraceEvent) -> TapToolCall {
    TapToolCall {
        name: event.payload.get("name").and_then(|v| v.as_str()).map(|s| s.to_string())
            .or_else(|| event.payload.get("tool").and_then(|v| v.as_str()).map(|s| s.to_string())),
        input: event.payload.get("input").and_then(|v| v.as_str()).map(|s| s.to_string())
            .or_else(|| event.payload.get("text").and_then(|v| v.as_str()).map(|s| s.to_string())),
        output: None,
        error: None,
    }
}

fn extract_tool_result(event: &TraceEvent) -> (Option<String>, Option<String>) {
    let output = event.payload.get("output").and_then(|v| v.as_str()).map(|s| s.to_string())
        .or_else(|| event.payload.get("text").and_then(|v| v.as_str()).map(|s| s.to_string()));
    let error = event.payload.get("error").and_then(|v| v.as_str()).map(|s| s.to_string());
    (output, error)
}

fn extract_file_event(event: &TraceEvent) -> TapFileEvent {
    TapFileEvent {
        path: event.payload.get("path").and_then(|v| v.as_str()).map(|s| s.to_string())
            .or_else(|| event.payload.get("file").and_then(|v| v.as_str()).map(|s| s.to_string())),
        content: event.payload.get("content").and_then(|v| v.as_str()).map(|s| s.to_string())
            .or_else(|| event.payload.get("text").and_then(|v| v.as_str()).map(|s| s.to_string())),
    }
}

/// Group trace events into steps. Each step starts with a prompt/user event
/// and includes all following events until the next prompt/user event.
///
/// Tool results are paired with their preceding tool call. If a tool result
/// has a `call_id` or `tool_call_id` in its payload, it matches the tool call
/// with the same `id`; otherwise it falls back to the most recent unpaired
/// tool call.
fn group_into_steps(events: &[TraceEvent]) -> Vec<TapStep> {
    let mut steps: Vec<TapStep> = Vec::new();
    let mut current: Option<TapStep> = None;

    for event in events {
        let kind = &event.kind;

        if is_prompt(kind) {
            if let Some(step) = current.take() {
                steps.push(step);
            }
            current = Some(TapStep {
                prompt: event_text(event).unwrap_or("").to_string(),
                response: String::new(),
                tool_calls: Vec::new(),
                file_reads: Vec::new(),
                file_edits: Vec::new(),
                events: vec![extract_tap_event(event)],
            });
        } else if let Some(ref mut step) = current {
            step.events.push(extract_tap_event(event));

            if is_response(kind) {
                let text = event_text(event).unwrap_or("");
                if !text.is_empty() {
                    if !step.response.is_empty() {
                        step.response.push_str("\n\n");
                    }
                    step.response.push_str(text);
                }
            } else if is_tool_call(kind) {
                step.tool_calls.push(extract_tool_call(event));
            } else if is_tool_result(kind) {
                let (output, error) = extract_tool_result(event);
                pair_tool_result(step, event, output, error);
            } else if is_file_read(kind) {
                step.file_reads.push(extract_file_event(event));
            } else if is_file_edit(kind) {
                step.file_edits.push(extract_file_event(event));
            }
        } else {
            // Events before any prompt — create a synthetic step
            if is_response(kind) {
                let step = TapStep {
                    prompt: String::new(),
                    response: event_text(event).unwrap_or("").to_string(),
                    tool_calls: Vec::new(),
                    file_reads: Vec::new(),
                    file_edits: Vec::new(),
                    events: vec![extract_tap_event(event)],
                };
                current = Some(step);
            }
        }
    }

    if let Some(step) = current {
        steps.push(step);
    }

    steps
}

/// Pair a tool result with its originating tool call. Prefers matching by
/// `call_id`/`tool_call_id` in payload; falls back to last unpaired call.
fn pair_tool_result(
    step: &mut TapStep,
    event: &TraceEvent,
    output: Option<String>,
    error: Option<String>,
) {
    let call_id = event.payload.get("call_id")
        .or_else(|| event.payload.get("tool_call_id"))
        .and_then(|v| v.as_str());

    let idx = if let Some(cid) = call_id {
        step.tool_calls.iter().position(|tc| {
            tc.name.as_deref() == Some(cid) || tc.input.as_deref() == Some(cid)
        })
    } else {
        None
    };

    let idx = idx.or_else(|| {
        step.tool_calls.iter().rposition(|tc| tc.output.is_none())
    });

    if let Some(i) = idx {
        let tc = &mut step.tool_calls[i];
        if tc.output.is_none() { tc.output = output; }
        if tc.error.is_none() { tc.error = error; }
    }
}

/// Load a Run and its Trace from the store, returning both.
fn load_run_and_trace(store: &dyn Store, run_hash: &Hash) -> Result<(Run, Trace, Hash), MorphError> {
    let obj = store.get(run_hash)?;
    let run = match obj {
        MorphObject::Run(r) => r,
        _ => return Err(MorphError::Serialization(format!("object {} is not a Run", run_hash))),
    };
    let trace_hash = Hash::from_hex(&run.trace)
        .map_err(|_| MorphError::InvalidHash(run.trace.clone()))?;
    let trace = match store.get(&trace_hash)? {
        MorphObject::Trace(t) => t,
        _ => return Err(MorphError::Serialization(format!(
            "object {} (run.trace) is not a Trace", run.trace
        ))),
    };
    Ok((run, trace, trace_hash))
}

/// Extract a complete task from a single run.
pub fn extract_task(store: &dyn Store, run_hash: &Hash) -> Result<TapTask, MorphError> {
    let (run, trace, trace_hash) = load_run_and_trace(store, run_hash)?;
    let steps = group_into_steps(&trace.events);
    let event_count = trace.events.len();
    let step_count = steps.len();

    let timestamp = trace.events.first()
        .map(|e| e.ts.clone())
        .unwrap_or_default();

    let token_usage = TapTokenUsage::from_parameters(&run.environment.parameters);

    Ok(TapTask {
        run_hash: run_hash.to_string(),
        trace_hash: trace_hash.to_string(),
        model: run.environment.model.clone(),
        agent: run.agent.id.clone(),
        agent_version: run.agent.version.clone(),
        timestamp,
        steps,
        event_count,
        step_count,
        token_usage,
    })
}

/// Diagnose a single run/trace for recording quality issues.
pub fn diagnose_run(store: &dyn Store, run_hash: &Hash) -> Result<TapDiagnostic, MorphError> {
    let (run, trace, trace_hash) = load_run_and_trace(store, run_hash)?;
    let steps = group_into_steps(&trace.events);

    let mut event_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut has_prompt = false;
    let mut has_response = false;
    let mut has_tool_calls = false;
    let mut has_file_reads = false;
    let mut has_file_edits = false;
    let mut has_tool_output = false;
    let mut prompt_count = 0;
    let mut response_empty = false;
    let mut prompt_lengths = Vec::new();
    let mut response_lengths = Vec::new();

    for event in &trace.events {
        *event_kinds.entry(event.kind.clone()).or_insert(0) += 1;

        if is_prompt(&event.kind) {
            has_prompt = true;
            prompt_count += 1;
            let text = event_text(event).unwrap_or("");
            prompt_lengths.push(text.len());
        }
        if is_response(&event.kind) {
            has_response = true;
            let text = event_text(event).unwrap_or("");
            response_lengths.push(text.len());
            if text.is_empty() || is_placeholder_response(text) {
                response_empty = true;
            }
        }
        if is_tool_call(&event.kind) { has_tool_calls = true; }
        if is_file_read(&event.kind) { has_file_reads = true; }
        if is_file_edit(&event.kind) { has_file_edits = true; }
        if is_tool_result(&event.kind) { has_tool_output = true; }
    }

    let mut issues = Vec::new();

    if !has_prompt {
        issues.push("no prompt/user event found".into());
    }
    if !has_response {
        issues.push("no response/assistant event found".into());
    }
    if response_empty {
        issues.push("response text is empty (hook may not have captured it)".into());
    }
    if !has_tool_calls && !has_file_reads && !has_file_edits {
        issues.push("no tool/file events — trace is prompt-response only (shallow)".into());
    }
    if run.environment.model == "cursor" || run.environment.model == "unknown" {
        issues.push(format!("model name is '{}' — not the actual LLM model", run.environment.model));
    }
    if run.metrics.is_empty() {
        issues.push("no metrics attached to run".into());
    }
    if prompt_lengths.iter().any(|&l| l < 10) {
        issues.push("very short prompt (<10 chars) — may be a recording artifact".into());
    }

    let token_usage = TapTokenUsage::from_parameters(&run.environment.parameters);

    Ok(TapDiagnostic {
        run_hash: run_hash.to_string(),
        trace_hash: trace_hash.to_string(),
        event_count: trace.events.len(),
        event_kinds,
        has_prompt,
        has_response,
        has_tool_calls,
        has_file_reads,
        has_file_edits,
        has_tool_output,
        prompt_count,
        response_empty,
        step_count: steps.len(),
        issues,
        prompt_lengths,
        response_lengths,
        model: run.environment.model.clone(),
        agent: run.agent.id.clone(),
        token_usage,
    })
}

fn is_placeholder_response(text: &str) -> bool {
    text == "(task completed; response not captured by hook)"
        || text.starts_with("(task completed")
        || text == "(no response)"
}

/// Summarize all runs in a repository for diagnostic overview.
pub fn summarize_repo(store: &dyn Store) -> Result<TapSummary, MorphError> {
    let run_hashes = store.list(ObjectType::Run)?;
    let trace_hashes = store.list(ObjectType::Trace)?;

    let mut total_events = 0;
    let mut event_kind_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut model_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut agent_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut runs_with_metrics = 0;
    let mut multi_step_runs = 0;
    let mut empty_response_runs = 0;
    let mut issues = Vec::new();

    for run_hash in &run_hashes {
        match load_run_and_trace(store, run_hash) {
            Ok((run, trace, _)) => {
                total_events += trace.events.len();

                for event in &trace.events {
                    *event_kind_counts.entry(normalize_kind(&event.kind).to_string()).or_insert(0) += 1;
                }

                *model_counts.entry(run.environment.model.clone()).or_insert(0) += 1;
                *agent_counts.entry(run.agent.id.clone()).or_insert(0) += 1;

                if !run.metrics.is_empty() {
                    runs_with_metrics += 1;
                }

                let steps = group_into_steps(&trace.events);
                if steps.len() > 1 {
                    multi_step_runs += 1;
                }

                let has_empty_response = trace.events.iter()
                    .filter(|e| is_response(&e.kind))
                    .any(|e| {
                        let text = event_text(e).unwrap_or("");
                        text.is_empty() || text == "(task completed; response not captured by hook)"
                    });
                if has_empty_response {
                    empty_response_runs += 1;
                }
            }
            Err(e) => {
                issues.push(format!("run {} failed to load: {}", run_hash, e));
            }
        }
    }

    let has_any_structured = event_kind_counts.iter().any(|(k, &c)| {
        c > 0 && (is_tool_call(k) || is_tool_result(k) || is_file_read(k) || is_file_edit(k))
    });
    if !has_any_structured {
        issues.push("no tool/file events in any trace — all traces are shallow prompt-response".into());
    }

    let bad_model_count = model_counts.get("cursor").copied().unwrap_or(0)
        + model_counts.get("unknown").copied().unwrap_or(0);
    if bad_model_count > 0 {
        issues.push(format!(
            "{} runs have model='cursor' or 'unknown' — actual LLM model not recorded",
            bad_model_count
        ));
    }

    if empty_response_runs > 0 {
        issues.push(format!(
            "{} runs have empty or placeholder responses",
            empty_response_runs
        ));
    }

    Ok(TapSummary {
        total_runs: run_hashes.len(),
        total_traces: trace_hashes.len(),
        total_events,
        event_kind_counts,
        model_counts,
        agent_counts,
        runs_with_metrics,
        multi_step_runs,
        empty_response_runs,
        issues,
    })
}

/// Export format for evaluation: one entry per task, structured for promptfoo
/// or similar frameworks.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TapEvalCase {
    pub run_hash: String,
    pub model: String,
    pub agent: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub expected_response: String,
    pub step_index: usize,
    pub total_steps: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<TapToolCall>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub file_reads: Vec<TapFileEvent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub file_edits: Vec<TapFileEvent>,
}

/// Export mode determines how traces are converted to eval cases.
#[derive(Clone, Debug)]
pub enum ExportMode {
    /// Each step becomes a separate eval case with just its prompt.
    PromptOnly,
    /// Each step includes file context from prior steps.
    WithContext,
    /// Multi-step tasks are exported as agentic workflows.
    Agentic,
}

/// Export all runs from a store as eval cases.
pub fn export_eval_cases(
    store: &dyn Store,
    mode: &ExportMode,
) -> Result<Vec<TapEvalCase>, MorphError> {
    let run_hashes = store.list(ObjectType::Run)?;
    let mut cases = Vec::new();

    for run_hash in &run_hashes {
        match extract_task(store, run_hash) {
            Ok(task) => {
                let task_cases = task_to_eval_cases(&task, mode);
                cases.extend(task_cases);
            }
            Err(_) => continue,
        }
    }

    Ok(cases)
}

/// Compute detailed statistics for a single trace (for debugging).
pub fn trace_stats(store: &dyn Store, trace_hash: &Hash) -> Result<TapTraceStats, MorphError> {
    let obj = store.get(trace_hash)?;
    let trace = match obj {
        MorphObject::Trace(t) => t,
        _ => return Err(MorphError::Serialization(format!("object {} is not a Trace", trace_hash))),
    };

    let mut event_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut normalized_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut prompt_lengths = Vec::new();
    let mut response_lengths = Vec::new();
    let mut payload_keys: BTreeMap<String, usize> = BTreeMap::new();
    let mut first_ts: Option<String> = None;
    let mut last_ts: Option<String> = None;

    for event in &trace.events {
        *event_kinds.entry(event.kind.clone()).or_insert(0) += 1;
        *normalized_kinds.entry(normalize_kind(&event.kind).to_string()).or_insert(0) += 1;

        for key in event.payload.keys() {
            *payload_keys.entry(key.clone()).or_insert(0) += 1;
        }

        if is_prompt(&event.kind) {
            prompt_lengths.push(event_text(event).unwrap_or("").len());
        }
        if is_response(&event.kind) {
            response_lengths.push(event_text(event).unwrap_or("").len());
        }

        if first_ts.is_none() {
            first_ts = Some(event.ts.clone());
        }
        last_ts = Some(event.ts.clone());
    }

    let has_structured_events = trace.events.iter().any(|e| {
        is_tool_call(&e.kind) || is_tool_result(&e.kind)
            || is_file_read(&e.kind) || is_file_edit(&e.kind)
    });

    let timestamp_range = first_ts.zip(last_ts);

    Ok(TapTraceStats {
        trace_hash: trace_hash.to_string(),
        event_count: trace.events.len(),
        event_kinds,
        normalized_kinds,
        has_structured_events,
        prompt_lengths,
        response_lengths,
        timestamp_range,
        payload_keys,
    })
}

/// List run hashes that match the given filter criteria.
pub fn filter_runs(
    store: &dyn Store,
    filter: &TapFilter,
) -> Result<Vec<Hash>, MorphError> {
    let all_runs = store.list(ObjectType::Run)?;
    let mut matched = Vec::new();

    for run_hash in &all_runs {
        match load_run_and_trace(store, run_hash) {
            Ok((run, trace, _)) => {
                if let Some(ref model) = filter.model {
                    if !run.environment.model.contains(model.as_str()) {
                        continue;
                    }
                }
                if let Some(ref agent) = filter.agent {
                    if !run.agent.id.contains(agent.as_str()) {
                        continue;
                    }
                }
                if let Some(min) = filter.min_steps {
                    let steps = group_into_steps(&trace.events);
                    if steps.len() < min {
                        continue;
                    }
                }
                if let Some(want_tools) = filter.has_tool_calls {
                    let has_tools = trace.events.iter().any(|e| is_tool_call(&e.kind));
                    if has_tools != want_tools {
                        continue;
                    }
                }
                matched.push(*run_hash);
            }
            Err(_) => continue,
        }
    }

    Ok(matched)
}

/// Convert a single task into eval cases (public API for filtered export).
pub fn task_to_eval_cases(task: &TapTask, mode: &ExportMode) -> Vec<TapEvalCase> {
    let mut cases = Vec::new();
    let mut accumulated_context = Vec::new();

    for (i, step) in task.steps.iter().enumerate() {
        if step.prompt.is_empty() && step.response.is_empty() {
            continue;
        }

        let context = match mode {
            ExportMode::PromptOnly => None,
            ExportMode::WithContext | ExportMode::Agentic => {
                if accumulated_context.is_empty() {
                    None
                } else {
                    Some(accumulated_context.join("\n\n---\n\n"))
                }
            }
        };

        let tool_calls = match mode {
            ExportMode::Agentic => step.tool_calls.clone(),
            _ => Vec::new(),
        };
        let file_reads = match mode {
            ExportMode::Agentic | ExportMode::WithContext => step.file_reads.clone(),
            _ => Vec::new(),
        };
        let file_edits = match mode {
            ExportMode::Agentic | ExportMode::WithContext => step.file_edits.clone(),
            _ => Vec::new(),
        };

        cases.push(TapEvalCase {
            run_hash: task.run_hash.clone(),
            model: task.model.clone(),
            agent: task.agent.clone(),
            prompt: step.prompt.clone(),
            context,
            expected_response: step.response.clone(),
            step_index: i,
            total_steps: task.steps.len(),
            tool_calls,
            file_reads,
            file_edits,
        });

        // Accumulate context for subsequent steps, including file contents
        if !step.response.is_empty() {
            accumulated_context.push(format!(
                "[Step {}] User: {}\nAssistant: {}",
                i + 1,
                truncate(&step.prompt, 200),
                truncate(&step.response, 500),
            ));
        }
        for fr in &step.file_reads {
            match (&fr.path, &fr.content) {
                (Some(path), Some(content)) => {
                    accumulated_context.push(format!(
                        "[File read: {}]\n{}",
                        path,
                        truncate(content, 1000),
                    ));
                }
                (Some(path), None) => {
                    accumulated_context.push(format!("[File read: {}]", path));
                }
                _ => {}
            }
        }
        for fe in &step.file_edits {
            match (&fe.path, &fe.content) {
                (Some(path), Some(content)) => {
                    accumulated_context.push(format!(
                        "[File edit: {}]\n{}",
                        path,
                        truncate(content, 1000),
                    ));
                }
                (Some(path), None) => {
                    accumulated_context.push(format!("[File edit: {}]", path));
                }
                _ => {}
            }
        }
        for tc in &step.tool_calls {
            let name = tc.name.as_deref().unwrap_or("tool");
            if let Some(ref output) = tc.output {
                accumulated_context.push(format!(
                    "[Tool: {}] {}",
                    name,
                    truncate(output, 500),
                ));
            }
        }
    }

    cases
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let end = s.floor_char_boundary(max);
        &s[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::*;

    fn setup_store() -> (tempfile::TempDir, Box<dyn Store>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _ = crate::repo::init_repo(root).unwrap();
        let morph_dir = root.join(".morph");
        let store = crate::open_store(&morph_dir).unwrap();
        (dir, store)
    }

    fn make_trace_events(specs: &[(&str, &str)]) -> Vec<TraceEvent> {
        specs.iter().enumerate().map(|(i, (kind, text))| {
            let mut payload = BTreeMap::new();
            payload.insert("text".into(), serde_json::Value::String(text.to_string()));
            TraceEvent {
                id: format!("evt_{}", i),
                seq: i as u64,
                ts: "2026-01-01T00:00:00Z".into(),
                kind: kind.to_string(),
                payload,
            }
        }).collect()
    }

    fn store_run_with_events(
        store: &dyn Store,
        events: Vec<TraceEvent>,
        model: &str,
        agent: &str,
    ) -> Hash {
        let trace = MorphObject::Trace(Trace { events });
        let trace_hash = store.put(&trace).unwrap();

        let identity = crate::identity::identity_pipeline();
        let pipeline_hash = store.put(&identity).unwrap();

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
                id: agent.into(),
                version: "1.0".into(),
                policy: None,
                instance_id: None,
            },
            contributors: None,
            morph_version: None,
        });
        store.put(&run).unwrap()
    }

    #[test]
    fn group_simple_prompt_response() {
        let events = make_trace_events(&[
            ("prompt", "What is 2+2?"),
            ("response", "4"),
        ]);
        let steps = group_into_steps(&events);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].prompt, "What is 2+2?");
        assert_eq!(steps[0].response, "4");
    }

    #[test]
    fn group_user_assistant_aliases() {
        let events = make_trace_events(&[
            ("user", "Hello"),
            ("assistant", "Hi there"),
        ]);
        let steps = group_into_steps(&events);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].prompt, "Hello");
        assert_eq!(steps[0].response, "Hi there");
    }

    #[test]
    fn group_multi_step() {
        let events = make_trace_events(&[
            ("prompt", "Step 1"),
            ("response", "Reply 1"),
            ("prompt", "Step 2"),
            ("response", "Reply 2"),
        ]);
        let steps = group_into_steps(&events);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].prompt, "Step 1");
        assert_eq!(steps[1].prompt, "Step 2");
        assert_eq!(steps[1].response, "Reply 2");
    }

    #[test]
    fn group_with_tool_calls() {
        let mut events = make_trace_events(&[
            ("prompt", "Build a server"),
            ("assistant", "Creating files"),
        ]);
        let mut tc_payload = BTreeMap::new();
        tc_payload.insert("name".into(), serde_json::json!("write_file"));
        tc_payload.insert("input".into(), serde_json::json!("server.py"));
        events.push(TraceEvent {
            id: "evt_tc".into(),
            seq: 2,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "tool_call".into(),
            payload: tc_payload,
        });
        let mut tr_payload = BTreeMap::new();
        tr_payload.insert("output".into(), serde_json::json!("file written"));
        events.push(TraceEvent {
            id: "evt_tr".into(),
            seq: 3,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "tool_result".into(),
            payload: tr_payload,
        });
        events.push(TraceEvent {
            id: "evt_r2".into(),
            seq: 4,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "assistant".into(),
            payload: {
                let mut p = BTreeMap::new();
                p.insert("text".into(), serde_json::json!("Done!"));
                p
            },
        });

        let steps = group_into_steps(&events);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].tool_calls.len(), 1);
        assert_eq!(steps[0].tool_calls[0].name.as_deref(), Some("write_file"));
        assert_eq!(steps[0].tool_calls[0].output.as_deref(), Some("file written"));
        assert!(steps[0].response.contains("Done!"));
    }

    #[test]
    fn group_with_file_events() {
        let mut events = make_trace_events(&[("prompt", "Read the config")]);
        let mut fr_payload = BTreeMap::new();
        fr_payload.insert("path".into(), serde_json::json!("config.toml"));
        fr_payload.insert("content".into(), serde_json::json!("[server]\nport=8080"));
        events.push(TraceEvent {
            id: "evt_fr".into(),
            seq: 1,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "file_read".into(),
            payload: fr_payload,
        });
        events.push(TraceEvent {
            id: "evt_r".into(),
            seq: 2,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "response".into(),
            payload: {
                let mut p = BTreeMap::new();
                p.insert("text".into(), serde_json::json!("Config says port 8080"));
                p
            },
        });

        let steps = group_into_steps(&events);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].file_reads.len(), 1);
        assert_eq!(steps[0].file_reads[0].path.as_deref(), Some("config.toml"));
    }

    #[test]
    fn group_empty_trace() {
        let steps = group_into_steps(&[]);
        assert!(steps.is_empty());
    }

    #[test]
    fn group_multiple_prompts_single_response() {
        let events = make_trace_events(&[
            ("prompt", "First question"),
            ("prompt", "Follow-up"),
            ("response", "Combined answer"),
        ]);
        let steps = group_into_steps(&events);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].prompt, "First question");
        assert!(steps[0].response.is_empty());
        assert_eq!(steps[1].prompt, "Follow-up");
        assert_eq!(steps[1].response, "Combined answer");
    }

    #[test]
    fn extract_task_from_store() {
        let (_dir, store) = setup_store();
        let events = make_trace_events(&[
            ("prompt", "Explain recursion"),
            ("response", "Recursion is when a function calls itself."),
        ]);
        let run_hash = store_run_with_events(store.as_ref(), events, "gpt-4o", "cursor");
        let task = extract_task(store.as_ref(), &run_hash).unwrap();

        assert_eq!(task.model, "gpt-4o");
        assert_eq!(task.agent, "cursor");
        assert_eq!(task.steps.len(), 1);
        assert_eq!(task.steps[0].prompt, "Explain recursion");
        assert_eq!(task.event_count, 2);
    }

    #[test]
    fn diagnose_shallow_trace() {
        let (_dir, store) = setup_store();
        let events = make_trace_events(&[
            ("prompt", "Do something"),
            ("response", "Done"),
        ]);
        let run_hash = store_run_with_events(store.as_ref(), events, "cursor", "cursor");
        let diag = diagnose_run(store.as_ref(), &run_hash).unwrap();

        assert!(diag.has_prompt);
        assert!(diag.has_response);
        assert!(!diag.has_tool_calls);
        assert!(!diag.has_file_reads);
        assert!(diag.issues.iter().any(|i| i.contains("shallow")));
        assert!(diag.issues.iter().any(|i| i.contains("model name")));
    }

    #[test]
    fn diagnose_empty_response() {
        let (_dir, store) = setup_store();
        let events = make_trace_events(&[
            ("prompt", "Do something"),
            ("response", ""),
        ]);
        let run_hash = store_run_with_events(store.as_ref(), events, "gpt-4o", "cursor");
        let diag = diagnose_run(store.as_ref(), &run_hash).unwrap();

        assert!(diag.response_empty);
        assert!(diag.issues.iter().any(|i| i.contains("empty")));
    }

    #[test]
    fn summarize_repo_counts() {
        let (_dir, store) = setup_store();
        let events1 = make_trace_events(&[
            ("prompt", "Task 1"),
            ("response", "Done 1"),
        ]);
        store_run_with_events(store.as_ref(), events1, "gpt-4o", "cursor");

        let events2 = make_trace_events(&[
            ("prompt", "Task 2a"),
            ("response", "Part 1"),
            ("prompt", "Task 2b"),
            ("response", "Part 2"),
        ]);
        store_run_with_events(store.as_ref(), events2, "claude-4", "opencode");

        let summary = summarize_repo(store.as_ref()).unwrap();
        assert_eq!(summary.total_runs, 2);
        assert_eq!(summary.total_events, 6);
        assert_eq!(summary.multi_step_runs, 1);
        assert_eq!(summary.model_counts.get("gpt-4o"), Some(&1));
        assert_eq!(summary.model_counts.get("claude-4"), Some(&1));
    }

    #[test]
    fn export_prompt_only() {
        let (_dir, store) = setup_store();
        let events = make_trace_events(&[
            ("prompt", "Fix the bug"),
            ("response", "Bug fixed"),
        ]);
        store_run_with_events(store.as_ref(), events, "gpt-4o", "test");

        let cases = export_eval_cases(store.as_ref(), &ExportMode::PromptOnly).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].prompt, "Fix the bug");
        assert_eq!(cases[0].expected_response, "Bug fixed");
        assert!(cases[0].context.is_none());
    }

    #[test]
    fn export_agentic_multi_step() {
        let (_dir, store) = setup_store();
        let events = make_trace_events(&[
            ("prompt", "Build feature"),
            ("response", "Starting build"),
            ("prompt", "Now test it"),
            ("response", "Tests pass"),
        ]);
        store_run_with_events(store.as_ref(), events, "gpt-4o", "test");

        let cases = export_eval_cases(store.as_ref(), &ExportMode::Agentic).unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].step_index, 0);
        assert!(cases[0].context.is_none());
        assert_eq!(cases[1].step_index, 1);
        assert!(cases[1].context.is_some());
    }

    #[test]
    fn normalize_kind_handles_aliases() {
        assert_eq!(normalize_kind("user"), "prompt");
        assert_eq!(normalize_kind("prompt"), "prompt");
        assert_eq!(normalize_kind("assistant"), "response");
        assert_eq!(normalize_kind("response"), "response");
        assert_eq!(normalize_kind("tool_call"), "tool_call");
    }

    #[test]
    fn extract_task_fails_on_non_run() {
        let (_dir, store) = setup_store();
        let blob = MorphObject::Blob(Blob {
            kind: "x".into(),
            content: serde_json::json!({}),
        });
        let hash = store.put(&blob).unwrap();
        assert!(extract_task(store.as_ref(), &hash).is_err());
    }

    #[test]
    fn trace_stats_reports_event_details() {
        let (_dir, store) = setup_store();
        let events = make_trace_events(&[
            ("user", "Hello world"),
            ("assistant", "Hi"),
            ("user", "Another question here"),
            ("assistant", "Short"),
        ]);
        let trace = MorphObject::Trace(Trace { events });
        let trace_hash = store.put(&trace).unwrap();

        let stats = trace_stats(store.as_ref(), &trace_hash).unwrap();
        assert_eq!(stats.event_count, 4);
        assert_eq!(stats.event_kinds.get("user"), Some(&2));
        assert_eq!(stats.event_kinds.get("assistant"), Some(&2));
        assert_eq!(stats.normalized_kinds.get("prompt"), Some(&2));
        assert_eq!(stats.normalized_kinds.get("response"), Some(&2));
        assert!(!stats.has_structured_events);
        assert_eq!(stats.prompt_lengths.len(), 2);
        assert_eq!(stats.response_lengths.len(), 2);
        assert!(stats.payload_keys.contains_key("text"));
    }

    #[test]
    fn trace_stats_detects_structured_events() {
        let (_dir, store) = setup_store();
        let mut events = make_trace_events(&[("prompt", "Do it")]);
        let mut tc_payload = BTreeMap::new();
        tc_payload.insert("name".into(), serde_json::json!("read_file"));
        events.push(TraceEvent {
            id: "evt_tc".into(), seq: 1,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "tool_call".into(), payload: tc_payload,
        });
        let trace = MorphObject::Trace(Trace { events });
        let trace_hash = store.put(&trace).unwrap();

        let stats = trace_stats(store.as_ref(), &trace_hash).unwrap();
        assert!(stats.has_structured_events);
    }

    #[test]
    fn filter_runs_by_model() {
        let (_dir, store) = setup_store();
        let events1 = make_trace_events(&[("prompt", "A"), ("response", "B")]);
        store_run_with_events(store.as_ref(), events1, "gpt-4o", "cursor");

        let events2 = make_trace_events(&[("prompt", "C"), ("response", "D")]);
        store_run_with_events(store.as_ref(), events2, "claude-4", "opencode");

        let filter = TapFilter { model: Some("gpt".into()), ..Default::default() };
        let matched = filter_runs(store.as_ref(), &filter).unwrap();
        assert_eq!(matched.len(), 1);

        let filter_all = TapFilter::default();
        let matched_all = filter_runs(store.as_ref(), &filter_all).unwrap();
        assert_eq!(matched_all.len(), 2);
    }

    #[test]
    fn filter_runs_by_min_steps() {
        let (_dir, store) = setup_store();
        let events1 = make_trace_events(&[("prompt", "A"), ("response", "B")]);
        store_run_with_events(store.as_ref(), events1, "gpt-4o", "cursor");

        let events2 = make_trace_events(&[
            ("prompt", "C"), ("response", "D"),
            ("prompt", "E"), ("response", "F"),
        ]);
        store_run_with_events(store.as_ref(), events2, "gpt-4o", "cursor");

        let filter = TapFilter { min_steps: Some(2), ..Default::default() };
        let matched = filter_runs(store.as_ref(), &filter).unwrap();
        assert_eq!(matched.len(), 1);
    }

    #[test]
    fn diagnose_includes_prompt_lengths_and_model() {
        let (_dir, store) = setup_store();
        let events = make_trace_events(&[
            ("prompt", "Hi"),
            ("response", "Hello there, nice to meet you!"),
        ]);
        let run_hash = store_run_with_events(store.as_ref(), events, "gpt-4o", "cursor");
        let diag = diagnose_run(store.as_ref(), &run_hash).unwrap();

        assert_eq!(diag.model, "gpt-4o");
        assert_eq!(diag.agent, "cursor");
        assert_eq!(diag.prompt_lengths, vec![2]);
        assert_eq!(diag.response_lengths, vec![30]);
        assert!(diag.issues.iter().any(|i| i.contains("very short prompt")));
    }

    #[test]
    fn placeholder_response_detected() {
        assert!(is_placeholder_response("(task completed; response not captured by hook)"));
        assert!(is_placeholder_response("(task completed)"));
        assert!(is_placeholder_response("(no response)"));
        assert!(!is_placeholder_response("This is a real response"));
    }

    #[test]
    fn tool_result_pairs_with_last_unpaired() {
        let mut events = make_trace_events(&[("prompt", "Do work")]);

        let mut tc1 = BTreeMap::new();
        tc1.insert("name".into(), serde_json::json!("read_file"));
        tc1.insert("input".into(), serde_json::json!("a.txt"));
        events.push(TraceEvent {
            id: "tc1".into(), seq: 1,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "tool_call".into(), payload: tc1,
        });

        let mut tc2 = BTreeMap::new();
        tc2.insert("name".into(), serde_json::json!("write_file"));
        tc2.insert("input".into(), serde_json::json!("b.txt"));
        events.push(TraceEvent {
            id: "tc2".into(), seq: 2,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "tool_call".into(), payload: tc2,
        });

        let mut tr1 = BTreeMap::new();
        tr1.insert("output".into(), serde_json::json!("contents of a"));
        events.push(TraceEvent {
            id: "tr1".into(), seq: 3,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "tool_result".into(), payload: tr1,
        });

        let mut tr2 = BTreeMap::new();
        tr2.insert("output".into(), serde_json::json!("written"));
        events.push(TraceEvent {
            id: "tr2".into(), seq: 4,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "tool_result".into(), payload: tr2,
        });

        let steps = group_into_steps(&events);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].tool_calls.len(), 2);
        // Last unpaired gets first result, then second result gets remaining
        assert!(steps[0].tool_calls[1].output.is_some());
        assert!(steps[0].tool_calls[0].output.is_some());
    }

    #[test]
    fn token_usage_extracted_from_run_parameters() {
        let (_dir, store) = setup_store();
        let events = make_trace_events(&[("user", "Hello"), ("assistant", "Hi")]);
        let trace = MorphObject::Trace(Trace { events });
        let trace_hash = store.put(&trace).unwrap();

        let identity = crate::identity::identity_pipeline();
        let pipeline_hash = store.put(&identity).unwrap();

        let mut params = BTreeMap::new();
        params.insert("input_tokens".into(), serde_json::json!(1500));
        params.insert("output_tokens".into(), serde_json::json!(500));
        params.insert("cache_read_tokens".into(), serde_json::json!(1000));

        let run = MorphObject::Run(Run {
            pipeline: pipeline_hash.to_string(),
            commit: None,
            environment: RunEnvironment {
                model: "claude-opus-4".into(),
                version: "1.0".into(),
                parameters: params,
                toolchain: BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: BTreeMap::new(),
            trace: trace_hash.to_string(),
            agent: AgentInfo { id: "cursor".into(), version: "1.0".into(), policy: None, instance_id: None },
            contributors: None,
            morph_version: None,
        });
        let run_hash = store.put(&run).unwrap();

        let task = extract_task(store.as_ref(), &run_hash).unwrap();
        assert_eq!(task.token_usage.input_tokens, Some(1500));
        assert_eq!(task.token_usage.output_tokens, Some(500));
        assert_eq!(task.token_usage.cache_read_tokens, Some(1000));
        assert_eq!(task.token_usage.total_tokens, Some(2000));
        assert!(!task.token_usage.is_empty());

        let diag = diagnose_run(store.as_ref(), &run_hash).unwrap();
        assert_eq!(diag.token_usage.input_tokens, Some(1500));
    }

    #[test]
    fn export_context_includes_file_content() {
        let (_dir, store) = setup_store();
        let mut events = make_trace_events(&[("prompt", "Read the config")]);

        let mut fr_payload = BTreeMap::new();
        fr_payload.insert("path".into(), serde_json::json!("config.toml"));
        fr_payload.insert("content".into(), serde_json::json!("[server]\nport=8080"));
        events.push(TraceEvent {
            id: "evt_fr".into(), seq: 1,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "file_read".into(), payload: fr_payload,
        });
        events.push(TraceEvent {
            id: "evt_r".into(), seq: 2,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "response".into(),
            payload: {
                let mut p = BTreeMap::new();
                p.insert("text".into(), serde_json::json!("Config says port 8080"));
                p
            },
        });
        events.push(TraceEvent {
            id: "evt_p2".into(), seq: 3,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "prompt".into(),
            payload: {
                let mut p = BTreeMap::new();
                p.insert("text".into(), serde_json::json!("Change the port"));
                p
            },
        });
        events.push(TraceEvent {
            id: "evt_r2".into(), seq: 4,
            ts: "2026-01-01T00:00:00Z".into(),
            kind: "response".into(),
            payload: {
                let mut p = BTreeMap::new();
                p.insert("text".into(), serde_json::json!("Done"));
                p
            },
        });

        let run_hash = store_run_with_events(store.as_ref(), events, "gpt-4o", "test");
        let task = extract_task(store.as_ref(), &run_hash).unwrap();
        let cases = task_to_eval_cases(&task, &ExportMode::WithContext);

        assert_eq!(cases.len(), 2);
        let ctx = cases[1].context.as_ref().expect("step 2 should have context");
        assert!(ctx.contains("config.toml"), "context should mention file path");
        assert!(ctx.contains("port=8080"), "context should include file content");
    }
}
