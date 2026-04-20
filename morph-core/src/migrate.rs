//! One-time migration from store 0.0 (or 0.1) to 0.2 (Git-format hashes).
//!
//! Loads all objects from the old store, rewrites hash references in dependency order,
//! writes to FsStore (Git-format hashing), updates refs and repo_version.

use crate::objects::*;
use crate::store::{FsStore, MorphError, Store};
use crate::Hash;
use std::collections::HashMap;
use std::path::Path;

/// Migrate a 0.0 repo at `morph_dir` to 0.2. Objects are rewritten with new hashes; refs updated.
pub fn migrate_0_0_to_0_2(morph_dir: &Path) -> Result<(), MorphError> {
    let objects_dir = morph_dir.join("objects");
    if !objects_dir.is_dir() {
        return Ok(());
    }

    // Load (old_hash, object) from 0.0 layout
    let mut old_objects: Vec<(Hash, MorphObject)> = Vec::new();
    for entry in std::fs::read_dir(&objects_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if name.len() != 64 {
            continue;
        }
        let old_hash = Hash::from_hex(name).map_err(|_| MorphError::InvalidHash(name.into()))?;
        let bytes = std::fs::read(&path)?;
        let obj: MorphObject =
            serde_json::from_slice(&bytes).map_err(|e| MorphError::Serialization(e.to_string()))?;
        old_objects.push((old_hash, obj));
    }

    if old_objects.is_empty() {
        set_repo_version(morph_dir, "0.2")?;
        return Ok(());
    }

    let mut map: HashMap<String, Hash> = HashMap::new();
    let git_store = FsStore::new_git(morph_dir);
    std::fs::create_dir_all(git_store.objects_dir())?;
    std::fs::create_dir_all(git_store.refs_dir())?;

    // Dependency order: no-refs first, then Tree, Pipeline, Commit, Run, TraceRollup, Annotation
    let order = dependency_order();
    for type_ord in order {
        for (old_hash, obj) in &old_objects {
            if object_type_ord(obj) != type_ord {
                continue;
            }
            let rewritten = rewrite_object(obj, &map)?;
            let new_hash = git_store.put(&rewritten)?;
            map.insert(old_hash.to_string(), new_hash);
        }
    }

    // Update refs: HEAD (symbolic stays), heads/* with new commit hashes
    let refs_heads = morph_dir.join("refs").join("heads");
    if refs_heads.is_dir() {
        for entry in std::fs::read_dir(&refs_heads)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = refs_heads.join(&name);
            let content = std::fs::read_to_string(&path)?.trim().to_string();
            if content.len() == 64
                && Hash::from_hex(&content).is_ok() {
                    if let Some(&new_h) = map.get(&content) {
                        git_store.ref_write(&format!("heads/{}", name), &new_h)?;
                    }
                }
        }
    }

    set_repo_version(morph_dir, "0.2")?;
    Ok(())
}

/// Migrate a 0.2 repo to 0.3. Only bumps the config version; no object rewriting.
pub fn migrate_0_2_to_0_3(morph_dir: &Path) -> Result<(), MorphError> {
    set_repo_version(morph_dir, "0.3")
}

/// Migrate a 0.3 repo to 0.4: move objects from flat layout to fan-out (2-char prefix dirs)
/// and create `.morph/.gitignore` if missing.
pub fn migrate_0_3_to_0_4(morph_dir: &Path) -> Result<(), MorphError> {
    let objects_dir = morph_dir.join("objects");
    if objects_dir.is_dir() {
        let mut moved = 0u64;
        let entries: Vec<_> = std::fs::read_dir(&objects_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().extension().and_then(|x| x.to_str()) == Some("json")
                    && e.path().is_file()
            })
            .collect();

        for entry in entries {
            let path = entry.path();
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) if s.len() == 64 => s.to_string(),
                _ => continue,
            };
            let (prefix, rest) = stem.split_at(2);
            let dest_dir = objects_dir.join(prefix);
            std::fs::create_dir_all(&dest_dir)?;
            let dest = dest_dir.join(format!("{}.json", rest));
            std::fs::rename(&path, &dest)?;
            moved += 1;
        }

        if moved > 0 {
            eprintln!("Moved {} objects to fan-out layout.", moved);
        }
    }

    let gitignore = morph_dir.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(&gitignore, "/objects/\n")?;
    }

    set_repo_version(morph_dir, "0.4")
}

fn set_repo_version(morph_dir: &Path, version: &str) -> Result<(), MorphError> {
    let config_path = morph_dir.join("config.json");
    let config = if config_path.exists() {
        let data = std::fs::read_to_string(&config_path)?;
        let mut v: serde_json::Value =
            serde_json::from_str(&data).map_err(|e| MorphError::Serialization(e.to_string()))?;
        v["repo_version"] = serde_json::Value::String(version.to_string());
        v
    } else {
        serde_json::json!({ "repo_version": version })
    };
    std::fs::write(
        config_path,
        serde_json::to_string_pretty(&config).map_err(|e| MorphError::Serialization(e.to_string()))?,
    )?;
    Ok(())
}

fn object_type_ord(obj: &MorphObject) -> u8 {
    match obj {
        MorphObject::Blob(_) => 0,
        MorphObject::EvalSuite(_) => 0,
        MorphObject::Trace(_) => 0,
        MorphObject::Artifact(_) => 0,
        MorphObject::Tree(_) => 1,
        MorphObject::Pipeline(_) => 2,
        MorphObject::Commit(_) => 3,
        MorphObject::Run(_) => 4,
        MorphObject::TraceRollup(_) => 5,
        MorphObject::Annotation(_) => 6,
    }
}

fn dependency_order() -> [u8; 7] {
    [0, 1, 2, 3, 4, 5, 6]
}

fn subst(map: &HashMap<String, Hash>, old: &str) -> String {
    map.get(old).map(|h| h.to_string()).unwrap_or_else(|| old.to_string())
}

fn rewrite_object(obj: &MorphObject, map: &HashMap<String, Hash>) -> Result<MorphObject, MorphError> {
    Ok(match obj {
        MorphObject::Blob(b) => MorphObject::Blob(b.clone()),
        MorphObject::EvalSuite(e) => MorphObject::EvalSuite(e.clone()),
        MorphObject::Trace(t) => MorphObject::Trace(t.clone()),
        MorphObject::Artifact(a) => MorphObject::Artifact(a.clone()),
        MorphObject::Tree(t) => MorphObject::Tree(Tree {
            entries: t
                .entries
                .iter()
                .map(|e| TreeEntry {
                    name: e.name.clone(),
                    hash: subst(map, &e.hash),
                    entry_type: e.entry_type.clone(),
                })
                .collect(),
        }),
        MorphObject::Pipeline(p) => MorphObject::Pipeline(Pipeline {
            graph: PipelineGraph {
                nodes: p
                    .graph
                    .nodes
                    .iter()
                    .map(|n| PipelineNode {
                        id: n.id.clone(),
                        kind: n.kind.clone(),
                        ref_: n.ref_.as_ref().map(|r| subst(map, r)),
                        params: n.params.clone(),
                        env: n.env.clone(),
                    })
                    .collect(),
                edges: p.graph.edges.clone(),
            },
            prompts: p.prompts.iter().map(|s| subst(map, s)).collect(),
            eval_suite: p.eval_suite.as_ref().map(|s| subst(map, s)),
            attribution: p.attribution.clone(),
            provenance: p.provenance.as_ref().map(|pr| Provenance {
                derived_from_run: pr.derived_from_run.as_ref().map(|s| subst(map, s)),
                derived_from_trace: pr.derived_from_trace.as_ref().map(|s| subst(map, s)),
                derived_from_event: pr.derived_from_event.clone(),
                method: pr.method.clone(),
            }),
        }),
        MorphObject::Commit(c) => MorphObject::Commit(Commit {
            tree: c.tree.as_ref().map(|s| subst(map, s)),
            pipeline: subst(map, &c.pipeline),
            parents: c.parents.iter().map(|s| subst(map, s)).collect(),
            message: c.message.clone(),
            timestamp: c.timestamp.clone(),
            author: c.author.clone(),
            contributors: c.contributors.clone(),
            eval_contract: EvalContract {
                suite: subst(map, &c.eval_contract.suite),
                observed_metrics: c.eval_contract.observed_metrics.clone(),
            },
            env_constraints: c.env_constraints.clone(),
            evidence_refs: c.evidence_refs.as_ref().map(|refs| refs.iter().map(|s| subst(map, s)).collect()),
            morph_version: c.morph_version.clone(),
        }),
        MorphObject::Run(r) => MorphObject::Run(Run {
            pipeline: subst(map, &r.pipeline),
            commit: r.commit.as_ref().map(|s| subst(map, s)),
            environment: r.environment.clone(),
            input_state_hash: r.input_state_hash.clone(),
            output_artifacts: r.output_artifacts.iter().map(|s| subst(map, s)).collect(),
            metrics: r.metrics.clone(),
            trace: subst(map, &r.trace),
            agent: AgentInfo {
                id: r.agent.id.clone(),
                version: r.agent.version.clone(),
                policy: r.agent.policy.as_ref().map(|s| subst(map, s)),
                instance_id: r.agent.instance_id.clone(),
            },
            contributors: r.contributors.clone(),
            morph_version: r.morph_version.clone(),
        }),
        MorphObject::TraceRollup(tr) => MorphObject::TraceRollup(TraceRollup {
            trace: subst(map, &tr.trace),
            summary: tr.summary.clone(),
            key_events: tr.key_events.clone(),
        }),
        MorphObject::Annotation(a) => {
            let mut data = a.data.clone();
            if a.kind == "link" {
                if let Some(serde_json::Value::String(t)) = data.get("target") {
                    data.insert("target".to_string(), serde_json::Value::String(subst(map, t)));
                }
            }
            MorphObject::Annotation(Annotation {
                target: subst(map, &a.target),
                target_sub: a.target_sub.clone(),
                kind: a.kind.clone(),
                data,
                author: a.author.clone(),
                timestamp: a.timestamp.clone(),
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{Blob, Commit, EvalContract, EvalSuite, MorphObject};
    use crate::repo::init_repo;
    use std::collections::BTreeMap;

    #[test]
    fn migrate_0_0_to_0_2_rewrites_hashes_and_sets_version() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let store = crate::store::FsStore::new(dir.path().join(".morph"));

        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"x": 1}),
        });
        let blob_hash = store.put(&blob).unwrap();
        let suite = MorphObject::EvalSuite(EvalSuite {
            cases: vec![],
            metrics: vec![],
        });
        let suite_hash = store.put(&suite).unwrap();
        let commit = MorphObject::Commit(Commit {
            tree: None,
            pipeline: blob_hash.to_string(),
            parents: vec![],
            message: "first".into(),
            timestamp: "2020-01-01T00:00:00Z".into(),
            author: "test".into(),
            contributors: None,
            eval_contract: EvalContract {
                suite: suite_hash.to_string(),
                observed_metrics: BTreeMap::new(),
            },
            env_constraints: None,
            evidence_refs: None,
            morph_version: None,
        });
        let commit_hash = store.put(&commit).unwrap();
        store.ref_write_raw("HEAD", "ref: heads/main").unwrap();
        store.ref_write("heads/main", &commit_hash).unwrap();

        let morph_dir = dir.path().join(".morph");
        migrate_0_0_to_0_2(&morph_dir).unwrap();

        assert_eq!(
            crate::repo::read_repo_version(&morph_dir).unwrap(),
            "0.2"
        );
        let git_store = FsStore::new_git(&morph_dir);
        let head_raw = git_store.ref_read_raw("HEAD").unwrap();
        assert!(head_raw.as_deref().map(|s| s.contains("heads/main")).unwrap_or(false));
        let head_hash = crate::commit::resolve_head(&git_store).unwrap();
        assert!(head_hash.is_some());
        let head = head_hash.unwrap();
        let obj = git_store.get(&head).unwrap();
        assert!(matches!(obj, MorphObject::Commit(_)));
        // New hashes differ from old
        assert_ne!(head, commit_hash);
    }

    #[test]
    fn migrate_0_2_to_0_3_bumps_version() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        set_repo_version(&morph_dir, "0.2").unwrap();
        assert_eq!(crate::repo::read_repo_version(&morph_dir).unwrap(), "0.2");

        migrate_0_2_to_0_3(&morph_dir).unwrap();
        assert_eq!(crate::repo::read_repo_version(&morph_dir).unwrap(), "0.3");
    }

    #[test]
    fn migrate_0_2_to_0_3_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        set_repo_version(&morph_dir, "0.2").unwrap();

        migrate_0_2_to_0_3(&morph_dir).unwrap();
        migrate_0_2_to_0_3(&morph_dir).unwrap();
        assert_eq!(crate::repo::read_repo_version(&morph_dir).unwrap(), "0.3");
    }

    #[test]
    fn migrate_0_0_to_0_2_empty_objects_dir() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");

        migrate_0_0_to_0_2(&morph_dir).unwrap();
        assert_eq!(crate::repo::read_repo_version(&morph_dir).unwrap(), "0.2");
    }

    #[test]
    fn migrate_0_0_to_0_2_no_objects_dir() {
        let dir = tempfile::tempdir().unwrap();
        let morph_dir = dir.path().join(".morph");
        std::fs::create_dir_all(&morph_dir).unwrap();
        std::fs::write(
            morph_dir.join("config.json"),
            r#"{"repo_version":"0.0"}"#,
        ).unwrap();

        migrate_0_0_to_0_2(&morph_dir).unwrap();
    }

    #[test]
    fn migrate_0_3_to_0_4_moves_objects_to_fanout() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        set_repo_version(&morph_dir, "0.3").unwrap();

        let store = FsStore::new_git(&morph_dir);
        let blob = MorphObject::Blob(Blob {
            kind: "test".into(),
            content: serde_json::json!({"data": 42}),
        });
        let hash = store.put(&blob).unwrap();
        let hex = hash.to_string();
        let flat_path = morph_dir.join("objects").join(format!("{}.json", hex));
        assert!(flat_path.exists(), "flat object should exist before migration");

        migrate_0_3_to_0_4(&morph_dir).unwrap();

        assert_eq!(crate::repo::read_repo_version(&morph_dir).unwrap(), "0.4");
        assert!(!flat_path.exists(), "flat object should be gone after migration");

        let (prefix, rest) = hex.split_at(2);
        let fanout_path = morph_dir.join("objects").join(prefix).join(format!("{}.json", rest));
        assert!(fanout_path.exists(), "fan-out object should exist after migration");

        let fanout_store = FsStore::new_git_fanout(&morph_dir);
        let got = fanout_store.get(&hash).unwrap();
        assert!(matches!(got, MorphObject::Blob(_)));
    }

    #[test]
    fn migrate_0_3_to_0_4_creates_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        set_repo_version(&morph_dir, "0.3").unwrap();

        let gitignore = morph_dir.join(".gitignore");
        if gitignore.exists() {
            std::fs::remove_file(&gitignore).unwrap();
        }

        migrate_0_3_to_0_4(&morph_dir).unwrap();
        assert!(gitignore.exists(), ".gitignore should be created");
        let content = std::fs::read_to_string(&gitignore).unwrap();
        assert!(content.contains("objects"), ".gitignore should ignore objects/");
    }

    #[test]
    fn migrate_0_3_to_0_4_empty_objects_dir() {
        let dir = tempfile::tempdir().unwrap();
        let _ = init_repo(dir.path()).unwrap();
        let morph_dir = dir.path().join(".morph");
        set_repo_version(&morph_dir, "0.3").unwrap();

        migrate_0_3_to_0_4(&morph_dir).unwrap();
        assert_eq!(crate::repo::read_repo_version(&morph_dir).unwrap(), "0.4");
    }
}
