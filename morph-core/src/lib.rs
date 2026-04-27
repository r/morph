//! Morph core: content-addressed object model, storage, and repository operations.
//!
//! Morph is a pure VCS for transformation pipelines. It does not execute pipelines;
//! it stores, versions, and gates on behavioral contracts.

pub mod hash;
pub mod objects;
pub mod store;
pub mod working;
pub mod commit;
pub mod metrics;
pub mod merge;
pub mod record;
pub mod annotate;
pub mod index;
pub mod tree;
pub mod extract;
pub mod tap;
pub mod language;
pub mod structured;
pub mod sync;
pub mod objmerge;
pub mod pipemerge;
pub mod text3way;
pub mod treemerge;
pub mod merge_state;
pub mod workdir;
pub mod merge_flow;
pub mod policy;
pub mod diff;
pub mod tag;
pub mod stash;
pub mod revert;
pub mod ssh_proto;
pub mod ssh_store;

pub use hash::{canonical_json, content_hash, content_hash_git, Hash};
pub use objects::MorphObject;
#[allow(deprecated)]
pub use store::GixStore;
pub use store::{resolve_hash_prefix, FsStore, MorphError, ObjectType, Store};
pub use repo::{
    init_repo, init_bare, is_bare, open_store, read_repo_version, require_store_version,
    resolve_morph_dir,
    STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_0_4, STORE_VERSION_0_5, STORE_VERSION_INIT,
};
pub use identity::identity_pipeline;
pub use author::{
    resolve_author, resolve_author_for_repo, read_identity_config, write_identity_config,
};
pub use agent::{
    generate_instance_id, read_instance_id, ensure_instance_id, write_instance_id,
};
pub use working::{find_repo, blob_from_prompt_file, blob_from_file, materialize_blob, pipeline_from_file, eval_suite_from_file, status, add_paths, StatusEntry, working_status, activity_summary, ActivitySummary};
pub use commit::{create_commit, create_tree_commit, create_tree_commit_with_provenance, create_merge_commit, create_merge_commit_full, create_merge_commit_with_retirement, rollup, resolve_head, current_branch, set_head_branch, set_head_detached, checkout_tree, log_from, CommitProvenance, resolve_provenance_from_run};
pub use metrics::{aggregate, check_thresholds, check_dominance, check_dominance_with_suite, aggregate_suite, union_suites, retire_metrics};
pub use merge::{MergePlan, DominanceResult, DominanceViolation, prepare_merge, execute_merge};
pub use objmerge::{
    merge_base, merge_commits, MergeOutcome, ObjConflict, StructuralKind, TrivialOutcome,
};
pub use pipemerge::{
    merge_pipelines, ConflictAxis, NodeConflict, PipelineMergeOutcome,
};
pub use text3way::{merge_text, TextMergeLabels, TextMergeResult};
pub use treemerge::{apply_workdir_ops, merge_trees, TreeMergeOutcome, WorkdirOp};
pub use merge_flow::{
    abort_merge, continue_merge, merge_progress_summary, resolve_node,
    start_merge, ContinueMergeOpts, ContinueMergeOutcome, MergeProgress,
    StartMergeOpts, StartMergeOutcome,
};
pub use record::{record_run, record_eval_metrics, record_session, record_conversation, ConversationMessage};
pub use extract::extract_pipeline_from_run;
pub use tap::{
    extract_task, diagnose_run, summarize_repo, export_eval_cases,
    trace_stats, filter_runs, task_to_eval_cases,
    TapTask, TapStep, TapEvent, TapToolCall, TapFileEvent,
    TapDiagnostic, TapSummary, TapEvalCase, ExportMode,
    TapTraceStats, TapFilter, TapTokenUsage,
};
pub use language::{
    LanguageAdapter, PythonLanguageAdapter, Symbol,
    adapter_for_filename, builtin_adapters,
};
pub use structured::{
    recent_trace_summaries, summarize_trace,
    task_structure, target_context, final_artifact,
    change_semantics, verification_steps, find_run_by_trace,
    TaskPhase, TaskScope, ArtifactType,
    TraceSummary, TaskStructure, TargetContext, FinalArtifact,
    ChangeSemantics, VerificationSteps, VerificationAction,
};
pub use annotate::{create_annotation, list_annotations};
pub use index::{read_index, write_index, clear_index, update_index, StagingIndex};
pub use tree::{build_tree, flatten_tree, restore_tree, empty_tree_hash};
pub use migrate::{migrate_0_0_to_0_2, migrate_0_2_to_0_3, migrate_0_3_to_0_4, migrate_0_4_to_0_5};
pub use sync::{
    RemoteSpec, read_remotes, write_remotes, add_remote,
    BranchUpstream, read_branch_upstreams, get_branch_upstream, set_branch_upstream,
    collect_reachable_objects, is_ancestor, verify_closure,
    push_branch, fetch_remote, pull_branch,
    clone_repo, CloneOpts, CloneOutcome,
    open_remote_store, list_refs,
};
pub use policy::{
    RepoPolicy, CertificationResult, GateResult,
    read_policy, write_policy, certify_commit, gate_check, enforce_push_gate,
    branch_matches_pattern, branch_matches_any,
};
pub use diff::{diff_trees, diff_commits, diff_file_maps, DiffEntry, DiffStatus};
pub use tag::{create_tag, list_tags, delete_tag};
pub use stash::{stash_save, stash_list, stash_pop, StashEntry};
pub use revert::revert_commit;
pub use gc::{gc, GcResult};
pub use store::ObjectLayout;

pub mod gc;
mod morphignore;
mod migrate;
mod repo;
mod identity;
pub mod author;
pub mod agent;
