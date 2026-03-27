//! Service layer: derives view models from the Morph object store.
//!
//! Each method opens the store, reads objects, and transforms them into
//! stable view-model types. All reads are stateless — the store is the
//! single source of truth.

use crate::org_policy::OrgPolicy;
use crate::views::*;
use morph_core::objects::MorphObject;
use morph_core::policy::{self as core_policy, RepoPolicy};
use morph_core::store::{MorphError, ObjectType, Store};
use morph_core::{log_from, open_store, Hash};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Context for one Morph repository.
#[derive(Clone)]
pub struct RepoContext {
    pub name: String,
    pub morph_dir: PathBuf,
}

impl RepoContext {
    pub fn open_store(&self) -> Result<Box<dyn Store>, MorphError> {
        open_store(&self.morph_dir)
    }

    // ── Repository summary ──────────────────────────────────────────

    pub fn summary(&self) -> Result<RepoSummary, MorphError> {
        let store = self.open_store()?;
        let head = morph_core::resolve_head(store.as_ref()).ok().flatten();
        let current_branch = morph_core::current_branch(store.as_ref()).unwrap_or(None);
        let branches = list_branch_names(&store)?;
        let commit_count = match head {
            Some(_) => log_from(store.as_ref(), "HEAD").map(|v| v.len()).unwrap_or(0),
            None => 0,
        };
        let run_count = store.list(ObjectType::Run).map(|v| v.len()).unwrap_or(0);

        Ok(RepoSummary {
            name: self.name.clone(),
            head: head.map(|h| h.to_string()),
            current_branch,
            branch_count: branches.len(),
            commit_count,
            run_count,
        })
    }

    // ── Branches ────────────────────────────────────────────────────

    pub fn list_branches(&self) -> Result<BranchListResponse, MorphError> {
        let store = self.open_store()?;
        let current = morph_core::current_branch(store.as_ref())?;
        let names = list_branch_names(&store)?;
        let mut branches = Vec::with_capacity(names.len());
        for name in names {
            let head = store.ref_read(&format!("heads/{}", name))?;
            branches.push(BranchInfo {
                name,
                head: head.map(|h| h.to_string()),
            });
        }
        Ok(BranchListResponse { branches, current })
    }

    // ── Commits ─────────────────────────────────────────────────────

    pub fn list_commits(&self, ref_name: &str) -> Result<CommitListResponse, MorphError> {
        let store = self.open_store()?;
        let hashes = log_from(store.as_ref(), ref_name)?;
        let mut commits = Vec::with_capacity(hashes.len());
        for h in &hashes {
            let obj = store.get(h)?;
            if let MorphObject::Commit(c) = &obj {
                let certified = has_certification(store.as_ref(), h);
                commits.push(CommitSummary {
                    hash: h.to_string(),
                    message: c.message.clone(),
                    author: c.author.clone(),
                    timestamp: c.timestamp.clone(),
                    parents: c.parents.clone(),
                    has_tree: c.tree.is_some(),
                    morph_version: c.morph_version.clone(),
                    metric_count: c.eval_contract.observed_metrics.len(),
                    is_merge: c.parents.len() > 1,
                    certified,
                });
            }
        }
        Ok(CommitListResponse { commits })
    }

    pub fn commit_detail(&self, hash_str: &str) -> Result<CommitDetailResponse, MorphError> {
        let hash = parse_hash(hash_str)?;
        let store = self.open_store()?;
        let obj = store.get(&hash)?;
        let commit = match obj {
            MorphObject::Commit(c) => c,
            _ => return Err(MorphError::Serialization(format!("{} is not a commit", hash_str))),
        };

        let behavioral_status = derive_behavioral_status(
            store.as_ref(),
            &self.morph_dir,
            &hash,
            &commit,
        );

        let contributors = commit.contributors.as_ref().map(|cs| {
            cs.iter()
                .map(|c| ContributorView {
                    id: c.id.clone(),
                    role: c.role.clone(),
                })
                .collect()
        });

        Ok(CommitDetailResponse {
            hash: hash.to_string(),
            message: commit.message,
            author: commit.author,
            timestamp: commit.timestamp,
            parents: commit.parents,
            pipeline: commit.pipeline,
            tree: commit.tree,
            morph_version: commit.morph_version,
            eval_contract: EvalContractView {
                suite: commit.eval_contract.suite,
                observed_metrics: commit.eval_contract.observed_metrics,
            },
            contributors,
            evidence_refs: commit.evidence_refs,
            env_constraints: commit.env_constraints,
            behavioral_status,
        })
    }

    // ── Runs ────────────────────────────────────────────────────────

    pub fn list_runs(&self) -> Result<RunListResponse, MorphError> {
        let store = self.open_store()?;
        let hashes = store.list(ObjectType::Run)?;
        let mut runs = Vec::with_capacity(hashes.len());
        for h in hashes {
            let obj = store.get(&h)?;
            if let MorphObject::Run(r) = &obj {
                runs.push(RunSummary {
                    hash: h.to_string(),
                    trace: r.trace.clone(),
                    pipeline: r.pipeline.clone(),
                    agent: format!("{} {}", r.agent.id, r.agent.version),
                    metric_count: r.metrics.len(),
                });
            }
        }
        Ok(RunListResponse { runs })
    }

    pub fn run_detail(&self, hash_str: &str) -> Result<RunDetailResponse, MorphError> {
        let hash = parse_hash(hash_str)?;
        let store = self.open_store()?;
        let obj = store.get(&hash)?;
        let run = match obj {
            MorphObject::Run(r) => r,
            _ => return Err(MorphError::Serialization(format!("{} is not a run", hash_str))),
        };
        let contributors = run.contributors.as_ref().map(|cs| {
            cs.iter()
                .map(|c| RunContributorView {
                    id: c.id.clone(),
                    version: c.version.clone(),
                    role: c.role.clone(),
                })
                .collect()
        });
        Ok(RunDetailResponse {
            hash: hash.to_string(),
            pipeline: run.pipeline,
            commit: run.commit,
            trace: run.trace,
            agent: AgentView {
                id: run.agent.id,
                version: run.agent.version,
                instance_id: run.agent.instance_id,
                policy: run.agent.policy,
            },
            environment: EnvironmentView {
                model: run.environment.model,
                version: run.environment.version,
                parameters: run.environment.parameters,
                toolchain: run.environment.toolchain,
            },
            metrics: run.metrics,
            output_artifacts: run.output_artifacts,
            contributors,
        })
    }

    // ── Traces ──────────────────────────────────────────────────────

    pub fn trace_detail(&self, hash_str: &str) -> Result<TraceDetailResponse, MorphError> {
        let hash = parse_hash(hash_str)?;
        let store = self.open_store()?;
        let obj = store.get(&hash)?;
        let trace = match obj {
            MorphObject::Trace(t) => t,
            _ => return Err(MorphError::Serialization(format!("{} is not a trace", hash_str))),
        };
        let event_count = trace.events.len();
        let events = trace
            .events
            .into_iter()
            .map(|e| TraceEventView {
                id: e.id,
                seq: e.seq,
                ts: e.ts,
                kind: e.kind,
                payload: e.payload,
            })
            .collect();
        Ok(TraceDetailResponse {
            hash: hash.to_string(),
            events,
            event_count,
        })
    }

    // ── Pipelines ───────────────────────────────────────────────────

    pub fn pipeline_detail(&self, hash_str: &str) -> Result<PipelineDetailResponse, MorphError> {
        let hash = parse_hash(hash_str)?;
        let store = self.open_store()?;
        let obj = store.get(&hash)?;
        let pipeline = match obj {
            MorphObject::Pipeline(p) => p,
            _ => {
                return Err(MorphError::Serialization(format!(
                    "{} is not a pipeline",
                    hash_str
                )))
            }
        };

        let attribution = pipeline.attribution.as_ref().map(|a| {
            a.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        serde_json::to_value(v).unwrap_or_default(),
                    )
                })
                .collect()
        });

        let provenance = pipeline.provenance.as_ref().map(|p| ProvenanceView {
            derived_from_run: p.derived_from_run.clone(),
            derived_from_trace: p.derived_from_trace.clone(),
            derived_from_event: p.derived_from_event.clone(),
            method: p.method.clone(),
        });

        let node_count = pipeline.graph.nodes.len();
        let edge_count = pipeline.graph.edges.len();

        let nodes = pipeline
            .graph
            .nodes
            .into_iter()
            .map(|n| PipelineNodeView {
                id: n.id,
                kind: n.kind,
                ref_: n.ref_,
                params: n.params,
                env: n.env,
            })
            .collect();

        let edges = pipeline
            .graph
            .edges
            .into_iter()
            .map(|e| PipelineEdgeView {
                from: e.from,
                to: e.to,
                kind: e.kind,
            })
            .collect();

        Ok(PipelineDetailResponse {
            hash: hash.to_string(),
            node_count,
            edge_count,
            prompts: pipeline.prompts,
            eval_suite: pipeline.eval_suite,
            attribution,
            provenance,
            graph: PipelineGraphView { nodes, edges },
        })
    }

    // ── Object (raw) ────────────────────────────────────────────────

    pub fn raw_object(&self, hash_str: &str) -> Result<serde_json::Value, MorphError> {
        let hash = parse_hash(hash_str)?;
        let store = self.open_store()?;
        let obj = store.get(&hash)?;
        serde_json::to_value(&obj).map_err(|e| MorphError::Serialization(e.to_string()))
    }

    // ── Annotations ─────────────────────────────────────────────────

    pub fn annotations(&self, hash_str: &str) -> Result<AnnotationsResponse, MorphError> {
        let hash = parse_hash(hash_str)?;
        let store = self.open_store()?;
        let all = morph_core::list_annotations(store.as_ref(), &hash, None)?;
        let with_sub = list_all_annotations_for_target(store.as_ref(), &hash)?;
        let mut annotations: Vec<AnnotationView> = Vec::new();
        for (h, a) in all.into_iter().chain(with_sub.into_iter()) {
            annotations.push(AnnotationView {
                hash: h.to_string(),
                kind: a.kind,
                data: a.data,
                author: a.author,
                timestamp: a.timestamp,
                target_sub: a.target_sub,
            });
        }
        Ok(AnnotationsResponse {
            target: hash_str.to_string(),
            annotations,
        })
    }

    // ── Policy ──────────────────────────────────────────────────────

    pub fn policy(
        &self,
        org: Option<&OrgPolicy>,
    ) -> Result<PolicyResponse, MorphError> {
        let repo_policy = core_policy::read_policy(&self.morph_dir)?;
        let repo_view = repo_policy_to_view(&repo_policy);

        let org_view = org.map(|o| OrgPolicyView {
            required_metrics: o.required_metrics.clone(),
            thresholds: o.thresholds.clone(),
            directions: o.directions.clone(),
            presets: o
                .presets
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        PolicyPresetView {
                            required_metrics: v.required_metrics.clone(),
                            thresholds: v.thresholds.clone(),
                        },
                    )
                })
                .collect(),
        });

        let effective_required = crate::org_policy::effective_required_metrics(
            org,
            &repo_policy.required_metrics,
        );
        let effective_thresh = crate::org_policy::effective_thresholds(
            org,
            &repo_policy.thresholds,
        );

        Ok(PolicyResponse {
            repo_policy: repo_view,
            org_policy: org_view,
            effective_required_metrics: effective_required,
            effective_thresholds: effective_thresh,
        })
    }

    // ── Gate ────────────────────────────────────────────────────────

    pub fn gate_status(&self, hash_str: &str) -> Result<GateStatusResponse, MorphError> {
        let hash = parse_hash(hash_str)?;
        let store = self.open_store()?;
        let obj = store.get(&hash)?;
        let commit = match &obj {
            MorphObject::Commit(c) => c,
            _ => return Err(MorphError::Serialization(format!("{} is not a commit", hash_str))),
        };

        let result = core_policy::gate_check(store.as_ref(), &self.morph_dir, &hash)?;
        let repo_policy = core_policy::read_policy(&self.morph_dir)?;

        Ok(GateStatusResponse {
            passed: result.passed,
            commit: hash.to_string(),
            reasons: result.reasons,
            metrics: commit.eval_contract.observed_metrics.clone(),
            policy: repo_policy_to_view(&repo_policy),
        })
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn parse_hash(s: &str) -> Result<Hash, MorphError> {
    Hash::from_hex(s).map_err(|_| MorphError::InvalidHash(s.to_string()))
}

fn list_branch_names(store: &dyn Store) -> Result<Vec<String>, MorphError> {
    let heads_dir = store.refs_dir().join("heads");
    if !heads_dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&heads_dir)? {
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

fn has_certification(store: &dyn Store, commit_hash: &Hash) -> bool {
    let annotations = morph_core::list_annotations(store, commit_hash, None).unwrap_or_default();
    annotations.iter().any(|(_, a)| {
        a.kind == "certification"
            && a.data
                .get("passed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
    })
}

fn find_certification_detail(
    store: &dyn Store,
    commit_hash: &Hash,
) -> Option<CertificationView> {
    let annotations = morph_core::list_annotations(store, commit_hash, None).unwrap_or_default();
    for (_, a) in annotations.iter().rev() {
        if a.kind == "certification" {
            let passed = a.data.get("passed").and_then(|v| v.as_bool()).unwrap_or(false);
            let runner = a
                .data
                .get("runner")
                .and_then(|v| v.as_str())
                .map(String::from);
            let eval_suite = a
                .data
                .get("eval_suite")
                .and_then(|v| v.as_str())
                .map(String::from);
            let metrics = a
                .data
                .get("metrics")
                .and_then(|v| serde_json::from_value::<BTreeMap<String, f64>>(v.clone()).ok())
                .unwrap_or_default();
            let failures = a
                .data
                .get("result")
                .and_then(|v| v.get("failures"))
                .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
                .unwrap_or_default();
            return Some(CertificationView {
                passed,
                runner,
                eval_suite,
                metrics,
                failures,
            });
        }
    }
    None
}

/// List ALL annotations targeting a hash (including those with target_sub set).
fn list_all_annotations_for_target(
    store: &dyn Store,
    target: &Hash,
) -> Result<Vec<(Hash, morph_core::objects::Annotation)>, MorphError> {
    let target_str = target.to_string();
    let hashes = store.list(ObjectType::Annotation)?;
    let mut out = Vec::new();
    for h in hashes {
        let obj = store.get(&h)?;
        if let MorphObject::Annotation(a) = obj {
            if a.target == target_str && a.target_sub.is_some() {
                out.push((h, a));
            }
        }
    }
    Ok(out)
}

fn derive_behavioral_status(
    store: &dyn Store,
    morph_dir: &Path,
    hash: &Hash,
    commit: &morph_core::objects::Commit,
) -> BehavioralStatus {
    let certified = has_certification(store, hash);
    let certification = find_certification_detail(store, hash);

    let gate_result = core_policy::gate_check(store, morph_dir, hash).ok();
    let gate_passed = gate_result.as_ref().map(|g| g.passed);
    let gate_reasons = gate_result
        .as_ref()
        .map(|g| g.reasons.clone())
        .unwrap_or_default();

    let is_merge = commit.parents.len() > 1;
    let merge_status = if is_merge && commit.parents.len() >= 2 {
        derive_merge_status(store, commit)
    } else {
        None
    };

    BehavioralStatus {
        certified,
        certification,
        gate_passed,
        gate_reasons,
        is_merge,
        merge_status,
    }
}

fn derive_merge_status(
    store: &dyn Store,
    commit: &morph_core::objects::Commit,
) -> Option<MergeStatusView> {
    let parent_a = &commit.parents[0];
    let parent_b = &commit.parents[1];

    let pa_metrics = load_commit_metrics(store, parent_a);
    let pb_metrics = load_commit_metrics(store, parent_b);
    let merged_metrics = commit.eval_contract.observed_metrics.clone();

    let dominates_a = if !pa_metrics.is_empty() && !merged_metrics.is_empty() {
        Some(
            pa_metrics
                .iter()
                .all(|(k, v)| merged_metrics.get(k).map(|mv| mv >= v).unwrap_or(false)),
        )
    } else {
        None
    };

    let dominates_b = if !pb_metrics.is_empty() && !merged_metrics.is_empty() {
        Some(
            pb_metrics
                .iter()
                .all(|(k, v)| merged_metrics.get(k).map(|mv| mv >= v).unwrap_or(false)),
        )
    } else {
        None
    };

    Some(MergeStatusView {
        parent_a: parent_a.clone(),
        parent_b: parent_b.clone(),
        merged_metrics,
        parent_a_metrics: pa_metrics,
        parent_b_metrics: pb_metrics,
        dominates_a,
        dominates_b,
    })
}

fn load_commit_metrics(store: &dyn Store, hash_str: &str) -> BTreeMap<String, f64> {
    let hash = match Hash::from_hex(hash_str) {
        Ok(h) => h,
        Err(_) => return BTreeMap::new(),
    };
    match store.get(&hash) {
        Ok(MorphObject::Commit(c)) => c.eval_contract.observed_metrics,
        _ => BTreeMap::new(),
    }
}

fn repo_policy_to_view(p: &RepoPolicy) -> RepoPolicyView {
    RepoPolicyView {
        required_metrics: p.required_metrics.clone(),
        thresholds: p.thresholds.clone(),
        directions: p.directions.clone(),
        default_eval_suite: p.default_eval_suite.clone(),
        merge_policy: p.merge_policy.clone(),
        ci_defaults: p.ci_defaults.clone(),
    }
}
