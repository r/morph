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
pub mod sync;
pub mod policy;
pub mod diff;
pub mod tag;
pub mod stash;
pub mod revert;

pub use hash::{canonical_json, content_hash, content_hash_git, Hash};
pub use objects::MorphObject;
#[allow(deprecated)]
pub use store::GixStore;
pub use store::{FsStore, MorphError, ObjectType, Store};
pub use repo::{
    init_repo, open_store, read_repo_version, require_store_version,
    STORE_VERSION_0_2, STORE_VERSION_0_3, STORE_VERSION_0_4, STORE_VERSION_INIT,
};
pub use identity::identity_pipeline;
pub use working::{find_repo, blob_from_prompt_file, blob_from_file, materialize_blob, pipeline_from_file, eval_suite_from_file, status, add_paths, StatusEntry, working_status, activity_summary, ActivitySummary};
pub use commit::{create_commit, create_tree_commit, create_tree_commit_with_provenance, create_merge_commit, create_merge_commit_full, create_merge_commit_with_retirement, rollup, resolve_head, current_branch, set_head_branch, set_head_detached, checkout_tree, log_from, CommitProvenance, resolve_provenance_from_run};
pub use metrics::{aggregate, check_thresholds, check_dominance, check_dominance_with_suite, aggregate_suite, union_suites, retire_metrics};
pub use merge::{MergePlan, DominanceResult, DominanceViolation, prepare_merge, execute_merge};
pub use record::{record_run, record_eval_metrics, record_session, record_conversation, ConversationMessage};
pub use extract::extract_pipeline_from_run;
pub use annotate::{create_annotation, list_annotations};
pub use index::{read_index, write_index, clear_index, update_index, StagingIndex};
pub use tree::{build_tree, flatten_tree, restore_tree, empty_tree_hash};
pub use migrate::{migrate_0_0_to_0_2, migrate_0_2_to_0_3, migrate_0_3_to_0_4};
pub use sync::{
    RemoteSpec, read_remotes, write_remotes, add_remote,
    collect_reachable_objects, is_ancestor,
    push_branch, fetch_remote, pull_branch,
    open_remote_store, list_refs,
};
pub use policy::{
    RepoPolicy, CertificationResult, GateResult,
    read_policy, write_policy, certify_commit, gate_check,
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
