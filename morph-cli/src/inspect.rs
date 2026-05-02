//! Handlers for the `morph inspect` namespace — the user-facing
//! interface for browsing recorded sessions (Run + Trace pairs) and
//! exporting them as evaluation cases. Introduced in Phase 3 (v0.45)
//! as a consolidation of the older `morph trace`, `morph tap`, and
//! `morph traces` commands; the predecessors were removed in
//! Phase 4.2 (v0.47). The Phase 4.1 (v0.46) `morph run` and
//! flat-`morph eval` aliases this module used to also back were
//! removed in Phase 4.3 (v0.48), along with the deprecation-notice
//! helper that pointed at them.

use crate::cli::InspectCmd;
use crate::resolve_obj_hash;
use anyhow::Result;
use morph_core::hex_prefix;
use morph_core::store::{ObjectType, Store};
use morph_core::MorphObject;

/// Top-level dispatch for `morph inspect <subcommand>`.
pub(crate) fn run_inspect(verbose: bool, sub: InspectCmd) -> Result<()> {
    match sub {
        InspectCmd::Summary { json } => run_summary(verbose, json),
        InspectCmd::Recent { limit, json } => run_recent(verbose, limit, json),
        InspectCmd::Show { hash } => run_show(verbose, &hash),
        InspectCmd::Diagnose { run_hash } => run_diagnose(verbose, &run_hash),
        InspectCmd::Export {
            mode,
            output,
            model,
            agent,
            min_steps,
        } => run_export(verbose, &mode, output.as_deref(), model, agent, min_steps),
        InspectCmd::Stats { trace_hash } => run_stats(verbose, &trace_hash),
        InspectCmd::Preview { run_hash, mode } => run_preview(verbose, &run_hash, &mode),
        InspectCmd::Task { hash } => run_task(verbose, &hash),
        InspectCmd::Target { hash } => run_target(verbose, &hash),
        InspectCmd::Artifact { hash } => run_artifact(verbose, &hash),
        InspectCmd::Semantics { hash } => run_semantics(verbose, &hash),
        InspectCmd::Verification { hash } => run_verification(verbose, &hash),
    }
}

pub(crate) fn run_summary(verbose: bool, json: bool) -> Result<()> {
    let (_repo_root, store) = crate::get_store(verbose)?;
    let summary = morph_core::summarize_repo(store.as_ref())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }
    // Heading still says "Tap" so users running CI scripts that grep
    // for the marker line don't have to change anything during the
    // alias window. The new heading lands in v0.47 alongside removal
    // of the deprecated commands.
    println!("=== Tap Repository Summary ===\n");
    println!("Runs:           {}", summary.total_runs);
    println!("Traces:         {}", summary.total_traces);
    println!("Total events:   {}", summary.total_events);
    println!("Multi-step:     {}", summary.multi_step_runs);
    println!("Empty response: {}", summary.empty_response_runs);
    println!("With metrics:   {}", summary.runs_with_metrics);
    println!("\nEvent kinds:");
    for (kind, count) in &summary.event_kind_counts {
        println!("  {:<16} {}", kind, count);
    }
    println!("\nModels:");
    for (model, count) in &summary.model_counts {
        println!("  {:<30} {}", model, count);
    }
    println!("\nAgents:");
    for (agent, count) in &summary.agent_counts {
        println!("  {:<20} {}", agent, count);
    }
    if !summary.issues.is_empty() {
        println!("\nIssues:");
        for issue in &summary.issues {
            println!("  ⚠ {}", issue);
        }
    }
    Ok(())
}

pub(crate) fn run_recent(verbose: bool, limit: usize, json: bool) -> Result<()> {
    let (_repo_root, store) = crate::get_store(verbose)?;
    let summaries = morph_core::recent_trace_summaries(store.as_ref(), limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&summaries)?);
        return Ok(());
    }
    println!("=== Recent Traces ({} shown) ===\n", summaries.len());
    for s in &summaries {
        let short = &s.run_hash[..12.min(s.run_hash.len())];
        let phase = serde_json::to_string(&s.task_phase).unwrap_or_default();
        let scope = serde_json::to_string(&s.task_scope).unwrap_or_default();
        println!(
            "{} {}  phase={}  scope={}",
            short,
            s.timestamp,
            phase.trim_matches('"'),
            scope.trim_matches('"')
        );
        if !s.target_files.is_empty() {
            println!("  files:   {}", s.target_files.join(", "));
        }
        if !s.target_symbols.is_empty() {
            println!("  symbols: {}", s.target_symbols.join(", "));
        }
        println!("  prompt:  {}\n", s.prompt_preview);
    }
    Ok(())
}

/// `inspect show <hash>` auto-detects what the user gave us:
///
/// - `all` → loop over every Run and print the extracted task.
/// - `<trace_hash>` → print the raw trace events.
/// - `<run_hash>` → print the extracted task structure (model,
///   agent, steps).
///
/// This collapses the two old commands `morph trace show` (events
/// only, errors on a Run) and `morph tap inspect` (task structure,
/// errors on a Trace) into one user-facing command.
pub(crate) fn run_show(verbose: bool, hash: &str) -> Result<()> {
    let (_repo_root, store) = crate::get_store(verbose)?;
    if hash == "all" {
        let hashes = store.list(ObjectType::Run)?;
        for h in &hashes {
            match morph_core::extract_task(store.as_ref(), h) {
                Ok(task) => print_tap_task(&task),
                Err(e) => eprintln!("run {}: {}", h, e),
            }
        }
        return Ok(());
    }
    let h = resolve_obj_hash(store.as_ref(), hash)?;
    match store.get(&h)? {
        MorphObject::Trace(t) => print_trace_events(&t),
        MorphObject::Run(_) => {
            let task = morph_core::extract_task(store.as_ref(), &h)?;
            print_tap_task(&task);
        }
        other => anyhow::bail!(
            "object {} is a {:?}; `morph inspect show` accepts trace or run hashes (or `all`)",
            hash,
            std::mem::discriminant(&other)
        ),
    }
    Ok(())
}

pub(crate) fn run_diagnose(verbose: bool, run_hash: &str) -> Result<()> {
    let (_repo_root, store) = crate::get_store(verbose)?;
    if run_hash == "all" {
        let hashes = store.list(ObjectType::Run)?;
        let mut total_issues = 0;
        let mut issue_counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for h in &hashes {
            match morph_core::diagnose_run(store.as_ref(), h) {
                Ok(diag) => {
                    for issue in &diag.issues {
                        total_issues += 1;
                        let key = issue.split(" — ").next().unwrap_or(issue).to_string();
                        *issue_counts.entry(key).or_insert(0) += 1;
                    }
                }
                Err(e) => eprintln!("run {}: {}", h, e),
            }
        }
        println!("=== Tap Diagnostic Summary ({} runs) ===\n", hashes.len());
        println!("Total issues: {}\n", total_issues);
        for (issue, count) in &issue_counts {
            println!("  [{:>3}x] {}", count, issue);
        }
    } else {
        let h = resolve_obj_hash(store.as_ref(), run_hash)?;
        let diag = morph_core::diagnose_run(store.as_ref(), &h)?;
        println!("{}", serde_json::to_string_pretty(&diag)?);
    }
    Ok(())
}

pub(crate) fn run_export(
    verbose: bool,
    mode: &str,
    output: Option<&std::path::Path>,
    model: Option<String>,
    agent: Option<String>,
    min_steps: Option<usize>,
) -> Result<()> {
    let (_repo_root, store) = crate::get_store(verbose)?;
    let export_mode = parse_export_mode(mode)?;

    let cases = if model.is_some() || agent.is_some() || min_steps.is_some() {
        let filter = morph_core::TapFilter {
            model,
            agent,
            min_steps,
            has_tool_calls: None,
        };
        let run_hashes = morph_core::filter_runs(store.as_ref(), &filter)?;
        let mut all_cases = Vec::new();
        for run_hash in &run_hashes {
            if let Ok(task) = morph_core::extract_task(store.as_ref(), run_hash) {
                let task_cases = morph_core::task_to_eval_cases(&task, &export_mode);
                all_cases.extend(task_cases);
            }
        }
        all_cases
    } else {
        morph_core::export_eval_cases(store.as_ref(), &export_mode)?
    };

    let json = serde_json::to_string_pretty(&cases)?;
    if let Some(path) = output {
        std::fs::write(path, &json)?;
        println!("Exported {} eval cases to {}", cases.len(), path.display());
    } else {
        println!("{}", json);
    }
    Ok(())
}

pub(crate) fn run_stats(verbose: bool, trace_hash: &str) -> Result<()> {
    let (_repo_root, store) = crate::get_store(verbose)?;
    let h = resolve_obj_hash(store.as_ref(), trace_hash)?;
    let stats = morph_core::trace_stats(store.as_ref(), &h)?;
    println!(
        "=== Trace {} ===\n",
        &trace_hash[..12.min(trace_hash.len())]
    );
    println!("Events:             {}", stats.event_count);
    println!(
        "Structured events:  {}",
        if stats.has_structured_events {
            "yes"
        } else {
            "no"
        }
    );
    if let Some((first, last)) = &stats.timestamp_range {
        println!("Time range:         {} .. {}", first, last);
    }
    println!("\nEvent kinds (raw):");
    for (kind, count) in &stats.event_kinds {
        println!("  {:<20} {}", kind, count);
    }
    println!("\nEvent kinds (normalized):");
    for (kind, count) in &stats.normalized_kinds {
        println!("  {:<20} {}", kind, count);
    }
    println!("\nPayload keys:");
    for (key, count) in &stats.payload_keys {
        println!("  {:<20} {}", key, count);
    }
    if !stats.prompt_lengths.is_empty() {
        let avg: f64 =
            stats.prompt_lengths.iter().sum::<usize>() as f64 / stats.prompt_lengths.len() as f64;
        println!(
            "\nPrompt lengths:     {} prompts, avg {:.0} chars",
            stats.prompt_lengths.len(),
            avg
        );
    }
    if !stats.response_lengths.is_empty() {
        let avg: f64 = stats.response_lengths.iter().sum::<usize>() as f64
            / stats.response_lengths.len() as f64;
        println!(
            "Response lengths:   {} responses, avg {:.0} chars",
            stats.response_lengths.len(),
            avg
        );
    }
    Ok(())
}

pub(crate) fn run_preview(verbose: bool, run_hash: &str, mode: &str) -> Result<()> {
    let (_repo_root, store) = crate::get_store(verbose)?;
    let h = resolve_obj_hash(store.as_ref(), run_hash)?;
    let task = morph_core::extract_task(store.as_ref(), &h)?;
    let export_mode = parse_export_mode(mode)?;
    let cases = morph_core::task_to_eval_cases(&task, &export_mode);

    println!(
        "=== Preview: {} ({} steps, mode: {}) ===\n",
        &run_hash[..12.min(run_hash.len())],
        task.step_count,
        mode
    );
    println!("Model: {}  Agent: {}", task.model, task.agent);
    println!();

    for case in &cases {
        println!("--- Step {}/{} ---", case.step_index + 1, case.total_steps);
        println!("[PROMPT] ({} chars)", case.prompt.len());
        let prompt_preview = if case.prompt.len() > 300 {
            format!(
                "{}...",
                &case.prompt[..case.prompt.floor_char_boundary(300)]
            )
        } else {
            case.prompt.clone()
        };
        println!("{}", prompt_preview);

        if let Some(ref ctx) = case.context {
            println!("\n[CONTEXT] ({} chars)", ctx.len());
            let ctx_preview = if ctx.len() > 500 {
                format!("{}...", &ctx[..ctx.floor_char_boundary(500)])
            } else {
                ctx.clone()
            };
            println!("{}", ctx_preview);
        }

        if !case.file_reads.is_empty() {
            println!("\n[FILE READS] {}", case.file_reads.len());
            for fr in &case.file_reads {
                let has_content = fr.content.is_some();
                println!(
                    "  {} {}",
                    fr.path.as_deref().unwrap_or("?"),
                    if has_content {
                        "(has content)"
                    } else {
                        "(path only)"
                    }
                );
            }
        }
        if !case.file_edits.is_empty() {
            println!("\n[FILE EDITS] {}", case.file_edits.len());
            for fe in &case.file_edits {
                let has_content = fe.content.is_some();
                println!(
                    "  {} {}",
                    fe.path.as_deref().unwrap_or("?"),
                    if has_content {
                        "(has content)"
                    } else {
                        "(path only)"
                    }
                );
            }
        }
        if !case.tool_calls.is_empty() {
            println!("\n[TOOL CALLS] {}", case.tool_calls.len());
            for tc in &case.tool_calls {
                println!(
                    "  {} {}{}",
                    tc.name.as_deref().unwrap_or("(unnamed)"),
                    if tc.output.is_some() {
                        "[has output]"
                    } else {
                        ""
                    },
                    if tc.error.is_some() {
                        " [has error]"
                    } else {
                        ""
                    }
                );
            }
        }

        println!(
            "\n[EXPECTED RESPONSE] ({} chars)",
            case.expected_response.len()
        );
        let resp_preview = if case.expected_response.len() > 300 {
            format!(
                "{}...",
                &case.expected_response[..case.expected_response.floor_char_boundary(300)]
            )
        } else {
            case.expected_response.clone()
        };
        println!("{}\n", resp_preview);
    }
    Ok(())
}

pub(crate) fn run_task(verbose: bool, hash: &str) -> Result<()> {
    let (_repo_root, store) = crate::get_store(verbose)?;
    let run_hash = crate::resolve_run_hash(store.as_ref(), hash)?;
    let out = morph_core::task_structure(store.as_ref(), &run_hash)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

pub(crate) fn run_target(verbose: bool, hash: &str) -> Result<()> {
    let (_repo_root, store) = crate::get_store(verbose)?;
    let run_hash = crate::resolve_run_hash(store.as_ref(), hash)?;
    let out = morph_core::target_context(store.as_ref(), &run_hash)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

pub(crate) fn run_artifact(verbose: bool, hash: &str) -> Result<()> {
    let (_repo_root, store) = crate::get_store(verbose)?;
    let run_hash = crate::resolve_run_hash(store.as_ref(), hash)?;
    let out = morph_core::final_artifact(store.as_ref(), &run_hash)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

pub(crate) fn run_semantics(verbose: bool, hash: &str) -> Result<()> {
    let (_repo_root, store) = crate::get_store(verbose)?;
    let run_hash = crate::resolve_run_hash(store.as_ref(), hash)?;
    let out = morph_core::change_semantics(store.as_ref(), &run_hash)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

pub(crate) fn run_verification(verbose: bool, hash: &str) -> Result<()> {
    let (_repo_root, store) = crate::get_store(verbose)?;
    let run_hash = crate::resolve_run_hash(store.as_ref(), hash)?;
    let out = morph_core::verification_steps(store.as_ref(), &run_hash)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn parse_export_mode(mode: &str) -> Result<morph_core::ExportMode> {
    match mode {
        "prompt-only" => Ok(morph_core::ExportMode::PromptOnly),
        "with-context" => Ok(morph_core::ExportMode::WithContext),
        "agentic" => Ok(morph_core::ExportMode::Agentic),
        other => anyhow::bail!(
            "unknown export mode '{}' (use: prompt-only, with-context, agentic)",
            other
        ),
    }
}

pub(crate) fn print_tap_task(task: &morph_core::TapTask) {
    println!("=== Run {} ===", hex_prefix(&task.run_hash, 12));
    println!(
        "  model: {}  agent: {}  events: {}  steps: {}",
        task.model, task.agent, task.event_count, task.step_count
    );
    for (i, step) in task.steps.iter().enumerate() {
        println!("\n  --- Step {} ---", i + 1);
        let prompt_preview = if step.prompt.len() > 120 {
            format!(
                "{}...",
                &step.prompt[..step.prompt.floor_char_boundary(120)]
            )
        } else {
            step.prompt.clone()
        };
        println!("  Prompt: {}", prompt_preview);
        if !step.tool_calls.is_empty() {
            println!("  Tool calls: {}", step.tool_calls.len());
            for tc in &step.tool_calls {
                println!(
                    "    - {}{}",
                    tc.name.as_deref().unwrap_or("(unnamed)"),
                    if tc.output.is_some() {
                        " [has output]"
                    } else {
                        ""
                    }
                );
            }
        }
        if !step.file_reads.is_empty() {
            println!("  File reads: {}", step.file_reads.len());
        }
        if !step.file_edits.is_empty() {
            println!("  File edits: {}", step.file_edits.len());
        }
        let resp_preview = if step.response.len() > 200 {
            format!(
                "{}...",
                &step.response[..step.response.floor_char_boundary(200)]
            )
        } else {
            step.response.clone()
        };
        if resp_preview.is_empty() {
            println!("  Response: (empty)");
        } else {
            println!("  Response: {}", resp_preview);
        }
    }
    println!();
}

pub(crate) fn print_trace_events(trace: &morph_core::objects::Trace) {
    for ev in &trace.events {
        let text = ev
            .payload
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        match ev.kind.as_str() {
            "prompt" | "user" => println!("--- prompt ---\n{}", text),
            "response" | "assistant" => println!("--- response ---\n{}", text),
            "tool_call" | "tool_use" | "function_call" => {
                let name = ev
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unnamed)");
                println!("--- tool_call: {} ---\n{}", name, text);
            }
            "tool_result" | "tool_output" | "function_result" => {
                let output = ev
                    .payload
                    .get("output")
                    .and_then(|v| v.as_str())
                    .unwrap_or(text);
                let err = ev.payload.get("error").and_then(|v| v.as_str());
                println!("--- tool_result ---\n{}", output);
                if let Some(e) = err {
                    println!("  error: {}", e);
                }
            }
            "file_read" | "read_file" => {
                let path = ev
                    .payload
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                println!("--- file_read: {} ---", path);
            }
            "file_edit" | "edit_file" | "write_file" | "file_write" => {
                let path = ev
                    .payload
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                println!("--- file_edit: {} ---", path);
            }
            _ => println!("--- {} ---\n{}", ev.kind, text),
        }
    }
}
