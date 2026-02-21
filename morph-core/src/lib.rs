//! Morph core: content-addressed object model, storage, and repository operations.
//!
//! Morph is a pure VCS for transformation programs. It does not execute programs;
//! it stores, versions, and gates on behavioral contracts.

pub mod hash;
pub mod objects;
pub mod store;
pub mod working;
pub mod commit;
pub mod metrics;
pub mod record;
pub mod annotate;

pub use hash::{canonical_json, content_hash, Hash};
pub use objects::MorphObject;
pub use store::{FsStore, MorphError, ObjectType, Store};
pub use repo::init_repo;
pub use identity::identity_program;
pub use working::{find_repo, blob_from_prompt_file, blob_from_file, materialize_blob, program_from_file, eval_suite_from_file, status, add_paths, StatusEntry};
pub use commit::{create_commit, create_merge_commit, rollup, resolve_head, current_branch, set_head_branch, set_head_detached, log_from};
pub use metrics::{aggregate, check_thresholds, check_dominance, aggregate_suite};
pub use record::{record_run, record_eval_metrics, record_session};
pub use annotate::{create_annotation, list_annotations};

mod repo;
mod identity;
