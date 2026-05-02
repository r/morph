//! Morph core: content-addressed object model, storage, and repository operations.
//!
//! Morph is a pure VCS for transformation pipelines. It does not execute pipelines;
//! it stores, versions, and gates on behavioral contracts.

pub mod annotate;
pub mod commit;
pub mod commit_config;
pub mod diff;
pub mod eval_parsers;
pub mod eval_suite;
pub mod extract;
pub mod forget;
pub mod hash;
pub mod index;
pub mod language;
pub mod merge;
pub mod merge_flow;
pub mod merge_state;
pub mod metrics;
pub mod objects;
pub mod objmerge;
pub mod pipemerge;
pub mod policy;
pub mod record;
pub mod reference;
pub mod revert;
pub mod run_breadcrumb;
pub mod ssh_proto;
pub mod ssh_store;
pub mod stash;
pub mod store;
pub mod structured;
pub mod sync;
pub mod tag;
pub mod tap;
pub mod text3way;
pub mod time;
pub mod tree;
pub mod treemerge;
pub mod workdir;
pub mod working;

pub use agent::{ensure_instance_id, generate_instance_id, read_instance_id, write_instance_id};
pub use annotate::{
    auto_detect_introduces_cases, build_introduces_cases_annotation, create_annotation,
    list_annotations, parse_introduces_cases_arg,
};
pub use author::{
    read_identity_config, resolve_author, resolve_author_for_repo, write_identity_config,
};
pub use commit::{
    checkout_tree, compute_human_edits, create_commit, create_merge_commit,
    create_merge_commit_full, create_merge_commit_with_retirement, create_tree_commit,
    create_tree_commit_with_provenance, current_branch, fold_human_author_into_contributors,
    log_from, resolve_head, resolve_provenance_from_run, rollup, set_head_branch,
    set_head_detached, CommitProvenance, DEFAULT_BRANCH,
};
pub use commit_config::{read_commit_test_command, write_commit_test_command};
pub use diff::{diff_commits, diff_file_maps, diff_trees, DiffEntry, DiffStatus};
pub use eval_parsers::{
    parse_auto, parse_cargo_test, parse_go_test, parse_jest, parse_pytest, parse_vitest,
    parse_with_runner,
};
pub use eval_suite::{
    add_cases_from_cucumber, add_cases_from_paths, add_cases_from_yaml, build_or_extend_suite,
    compute_eval_gaps, diff_suite_case_ids,
};
pub use extract::extract_pipeline_from_run;
pub use forget::{
    apply_tombstone, commits_referencing, forget_local, kind_is_forgettable, ForgetReport,
    RETROACTIVE_NOTE,
};
pub use gc::{gc, GcResult};
pub use hash::{canonical_json, content_hash, content_hash_git, short_hash_str, Hash};
pub use identity::identity_pipeline;
pub use index::{clear_index, read_index, update_index, write_index, StagingIndex};
pub use language::{
    adapter_for_filename, builtin_adapters, LanguageAdapter, PythonLanguageAdapter, Symbol,
};
pub use merge::{
    ensure_review_node_for_retirement, execute_merge, prepare_merge, DominanceResult,
    DominanceViolation, MergePlan,
};
pub use merge_flow::{
    abort_merge, continue_merge, merge_progress_summary, resolve_node, start_merge,
    ContinueMergeOpts, ContinueMergeOutcome, MergeProgress, StartMergeOpts, StartMergeOutcome,
};
pub use metrics::{
    aggregate, aggregate_suite, check_dominance, check_dominance_with_suite, check_thresholds,
    retire_metrics, union_suites,
};
pub use migrate::{
    migrate_0_0_to_0_2, migrate_0_2_to_0_3, migrate_0_3_to_0_4, migrate_0_4_to_0_5,
    migrate_to_latest, MigrateReport, MigrationStep,
};
pub use objects::{
    CommitContributor, Eval, EvalContract, EvalItem, MorphObject, Session, SessionEvent,
    SessionTrace, Tombstone,
};
pub use objmerge::{
    merge_base, merge_commits, MergeOutcome, ObjConflict, StructuralKind, TrivialOutcome,
};
pub use pipemerge::{merge_pipelines, ConflictAxis, NodeConflict, PipelineMergeOutcome};
pub use policy::{
    branch_matches_any, branch_matches_pattern, certify_commit, effective_metrics,
    effective_metrics_for_commit, enforce_push_gate, gate_check, missing_required_metrics,
    read_policy, write_policy, CertificationResult, GateResult, RepoPolicy,
};
pub use record::{
    record_conversation, record_eval_metrics, record_eval_run, record_run, record_session,
    run_test_command, ConversationMessage, EvalRunOutcome,
};
pub use reference::{
    backfill_from_init, clear_merge_breadcrumb, current_git_branch, drift_summary,
    ensure_branch_synced, ensure_morph_in_git_info_exclude, git_head_sha, git_log_range,
    git_parents, handle_post_checkout, handle_post_rewrite, handle_pre_merge_commit,
    install_post_commit_hook, install_reference_hooks, is_git_working_tree,
    list_stale_certifications, list_unmerged_paths, lookup_branch_git_sha,
    lookup_morph_for_git_sha, merge_ref_path, pending_certifications, read_git_commit,
    read_merge_breadcrumb, rebuild_merge_commit, reference_mode_hooks,
    run_git_commit_with_morph_internal, run_git_merge_abort_with_morph_internal,
    run_git_merge_with_morph_internal, sync_to_head, sync_to_head_with_origin,
    write_merge_breadcrumb, BranchSyncOutcome, CheckoutOutcome, DriftSummary, GitCommitInfo,
    GitMergeOutcome, HookInstallReport, MergeRebuildOpts, PreMergeOutcome,
    ReferenceMergeBreadcrumb, RewriteOutcome, SyncOutcome, POST_CHECKOUT_HOOK_SCRIPT,
    POST_COMMIT_HOOK_SCRIPT, POST_MERGE_HOOK_SCRIPT, POST_REWRITE_HOOK_SCRIPT,
    PRE_MERGE_COMMIT_HOOK_SCRIPT,
};
pub use repo::{
    drop_legacy_repo_mode, init_bare, init_repo, is_bare, is_legacy_standalone, open_store,
    read_init_at_git_sha, read_repo_submode, read_repo_version, require_store_version,
    resolve_morph_dir, write_init_at_git_sha, write_repo_submode, write_repo_version, RepoSubmode,
    STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_0_4, STORE_VERSION_0_5, STORE_VERSION_INIT,
    STORE_VERSION_LATEST, SUPPORTED_REPO_VERSIONS,
};
pub use revert::revert_commit;
pub use run_breadcrumb::{
    clear_last_run, fingerprint_index, read_last_run, record_last_run, resolve_fresh_last_run,
    write_last_run, LastRun, StaleReason,
};
pub use stash::{stash_list, stash_pop, stash_save, StashEntry};
#[allow(deprecated)]
pub use store::GixStore;
pub use store::ObjectLayout;
pub use store::{resolve_hash_prefix, resolve_revision, FsStore, MorphError, ObjectType, Store};
pub use structured::{
    change_semantics, final_artifact, find_run_by_trace, recent_trace_summaries,
    resolve_run_or_trace_hash, summarize_trace, target_context, task_structure, verification_steps,
    ArtifactType, ChangeSemantics, FinalArtifact, TargetContext, TaskPhase, TaskScope,
    TaskStructure, TraceSummary, VerificationAction, VerificationSteps,
};
pub use sync::{
    add_remote, clone_repo, collect_reachable_objects, fetch_remote, get_branch_upstream,
    is_ancestor, list_refs, open_remote_store, pull_branch, push_branch, read_branch_upstreams,
    read_remotes, set_branch_upstream, verify_closure, write_remotes, BranchUpstream, CloneOpts,
    CloneOutcome, RemoteSpec,
};
pub use tag::{create_tag, delete_tag, list_tags};
pub use tap::{
    diagnose_run, export_eval_cases, extract_task, filter_runs, summarize_repo, task_to_eval_cases,
    trace_stats, ExportMode, TapDiagnostic, TapEvalCase, TapEvent, TapFileEvent, TapFilter,
    TapStep, TapSummary, TapTask, TapTokenUsage, TapToolCall, TapTraceStats,
};
pub use text3way::{merge_text, TextMergeLabels, TextMergeResult};
pub use time::now_rfc3339_utc;
pub use tree::{build_tree, empty_tree_hash, flatten_tree, restore_tree};
pub use treemerge::{apply_workdir_ops, merge_trees, TreeMergeOutcome, WorkdirOp};
pub use working::{
    activity_summary, add_paths, blob_from_file, blob_from_prompt_file, build_status_json,
    eval_suite_from_file, find_repo, materialize_blob, pipeline_from_file, status, working_status,
    ActivitySummary, StatusEntry,
};

pub mod agent;
pub mod author;
pub mod gc;
mod identity;
mod migrate;
mod morphignore;
mod repo;
