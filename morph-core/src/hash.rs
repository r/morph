//! Content addressing: canonical JSON and SHA-256 hashing.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// Content-addressed hash (SHA-256, 32 bytes). Display as hex.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Hash(#[serde(with = "hex_serde")] [u8; 32]);

mod hex_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8; 32], s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(d: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        let bytes = hex::decode(s).map_err(serde::de::Error::custom)?;
        bytes.try_into().map_err(|v: Vec<u8>| {
            serde::de::Error::custom(format!("expected 32 bytes, got {}", v.len()))
        })
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", hex::encode(self.0))
    }
}

impl Hash {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Hash(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Parse from hex string (64 chars).
    pub fn from_hex(s: &str) -> Result<Self, crate::store::MorphError> {
        let bytes = hex::decode(s).map_err(|e| crate::store::MorphError::InvalidHash(e.to_string()))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| crate::store::MorphError::InvalidHash("expected 32 bytes".into()))?;
        Ok(Hash(arr))
    }
}

/// Compute SHA-256 hash of canonical JSON bytes for a Morph object (0.0/0.1 format).
pub fn content_hash(obj: &crate::objects::MorphObject) -> Result<Hash, crate::store::MorphError> {
    let json = canonical_json(obj)?;
    let digest = Sha256::digest(json.as_bytes());
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&digest);
    Ok(Hash(arr))
}

/// Compute Git-format content hash for a Morph object (0.2 format).
/// Hash = SHA-256 of "blob " + decimal_len + "\0" + canonical_json.
/// This matches the hash gix produces when writing a blob.
pub fn content_hash_git(obj: &crate::objects::MorphObject) -> Result<Hash, crate::store::MorphError> {
    let json = canonical_json(obj)?;
    let bytes = json.as_bytes();
    let header = format!("blob {}\0", bytes.len());
    let mut hasher = Sha256::new();
    hasher.update(header.as_bytes());
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&digest);
    Ok(Hash(arr))
}

/// Serialize to canonical JSON (deterministic, for hashing). Uses compact form.
/// All map-typed fields in MorphObject use BTreeMap, which iterates in sorted key order,
/// so serde_json::to_string produces deterministic output directly.
pub fn canonical_json(obj: &crate::objects::MorphObject) -> Result<String, crate::store::MorphError> {
    serde_json::to_string(obj).map_err(|e| crate::store::MorphError::Serialization(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::*;

    #[test]
    fn hash_deterministic() {
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"x": 1}),
        });
        let h1 = content_hash(&blob).unwrap();
        let h2 = content_hash(&blob).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn different_object_different_hash() {
        let b1 = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"a": 1}),
        });
        let b2 = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"a": 2}),
        });
        assert_ne!(content_hash(&b1).unwrap(), content_hash(&b2).unwrap());
    }

    #[test]
    fn roundtrip_blob_serialization() {
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"template": "Hello {{name}}"}),
        });
        let json = canonical_json(&blob).unwrap();
        let parsed: MorphObject = serde_json::from_str(&json).unwrap();
        assert_eq!(content_hash(&blob).unwrap(), content_hash(&parsed).unwrap());
    }

    #[test]
    fn roundtrip_all_simple_types() {
        let tree = MorphObject::Tree(Tree {
            entries: vec![TreeEntry {
                name: "a".into(),
                hash: "0".repeat(64),
                entry_type: "blob".into(),
            }],
        });
        let json = canonical_json(&tree).unwrap();
        let parsed: MorphObject = serde_json::from_str(&json).unwrap();
        assert_eq!(content_hash(&tree).unwrap(), content_hash(&parsed).unwrap());
    }

    #[test]
    fn from_hex_invalid_rejected() {
        assert!(Hash::from_hex("").is_err());
        assert!(Hash::from_hex("ab").is_err());
        assert!(Hash::from_hex(&"f".repeat(63)).is_err());
        assert!(Hash::from_hex(&"g".repeat(64)).is_err());
        assert!(Hash::from_hex(&"0".repeat(64)).is_ok());
    }

    // --- instance_id on agent identity types ---

    #[test]
    fn agent_info_with_instance_id_roundtrip() {
        let run = MorphObject::Run(Run {
            pipeline: "0".repeat(64),
            commit: None,
            environment: RunEnvironment {
                model: "test".into(),
                version: "1.0".into(),
                parameters: std::collections::BTreeMap::new(),
                toolchain: std::collections::BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: std::collections::BTreeMap::new(),
            trace: "0".repeat(64),
            agent: AgentInfo {
                id: "cursor".into(),
                version: "1.0".into(),
                instance_id: Some("550e8400-e29b-41d4-a716-446655440000".into()),
                policy: None,
            },
            contributors: None,
            morph_version: None,
        });
        let json = canonical_json(&run).unwrap();
        let parsed: MorphObject = serde_json::from_str(&json).unwrap();
        assert_eq!(content_hash(&run).unwrap(), content_hash(&parsed).unwrap());
        if let MorphObject::Run(r) = &parsed {
            assert_eq!(r.agent.instance_id.as_deref(), Some("550e8400-e29b-41d4-a716-446655440000"));
        } else {
            panic!("expected Run");
        }
    }

    #[test]
    fn agent_info_without_instance_id_defaults_to_none() {
        let json = r#"{"type":"run","program":"aa","environment":{"model":"m","version":"1"},"input_state_hash":"bb","output_artifacts":[],"trace":"cc","agent":{"id":"cursor","version":"1.0"}}"#;
        let parsed: MorphObject = serde_json::from_str(json).unwrap();
        if let MorphObject::Run(r) = &parsed {
            assert_eq!(r.agent.instance_id, None);
        } else {
            panic!("expected Run");
        }
    }

    #[test]
    fn different_instance_id_different_hash() {
        let make_run = |iid: Option<&str>| {
            MorphObject::Run(Run {
                pipeline: "0".repeat(64),
                commit: None,
                environment: RunEnvironment {
                    model: "test".into(),
                    version: "1.0".into(),
                    parameters: std::collections::BTreeMap::new(),
                    toolchain: std::collections::BTreeMap::new(),
                },
                input_state_hash: "0".repeat(64),
                output_artifacts: vec![],
                metrics: std::collections::BTreeMap::new(),
                trace: "0".repeat(64),
                agent: AgentInfo {
                    id: "cursor".into(),
                    version: "1.0".into(),
                    instance_id: iid.map(String::from),
                    policy: None,
                },
                contributors: None,
                morph_version: None,
            })
        };
        let r1 = make_run(Some("aaa"));
        let r2 = make_run(Some("bbb"));
        let r3 = make_run(None);
        assert_ne!(content_hash(&r1).unwrap(), content_hash(&r2).unwrap());
        assert_ne!(content_hash(&r1).unwrap(), content_hash(&r3).unwrap());
    }

    #[test]
    fn contributor_info_instance_id_roundtrip() {
        let run = MorphObject::Run(Run {
            pipeline: "0".repeat(64),
            commit: None,
            environment: RunEnvironment {
                model: "test".into(),
                version: "1.0".into(),
                parameters: std::collections::BTreeMap::new(),
                toolchain: std::collections::BTreeMap::new(),
            },
            input_state_hash: "0".repeat(64),
            output_artifacts: vec![],
            metrics: std::collections::BTreeMap::new(),
            trace: "0".repeat(64),
            agent: AgentInfo {
                id: "cursor".into(),
                version: "1.0".into(),
                instance_id: None,
                policy: None,
            },
            contributors: Some(vec![ContributorInfo {
                id: "agent-a".into(),
                version: "2.0".into(),
                instance_id: Some("inst-abc".into()),
                policy: None,
                role: Some("generation".into()),
            }]),
            morph_version: None,
        });
        let json = canonical_json(&run).unwrap();
        let parsed: MorphObject = serde_json::from_str(&json).unwrap();
        if let MorphObject::Run(r) = &parsed {
            let c = &r.contributors.as_ref().unwrap()[0];
            assert_eq!(c.instance_id.as_deref(), Some("inst-abc"));
        } else {
            panic!("expected Run");
        }
    }

    #[test]
    fn attribution_entry_instance_id_roundtrip() {
        let mut attribution = std::collections::BTreeMap::new();
        attribution.insert("node1".to_string(), AttributionEntry {
            agent_id: "agent-x".into(),
            agent_version: Some("3.0".into()),
            instance_id: Some("inst-xyz".into()),
            actors: None,
        });
        let prog = MorphObject::Pipeline(Pipeline {
            graph: PipelineGraph {
                nodes: vec![PipelineNode {
                    id: "node1".into(),
                    kind: "identity".into(),
                    ref_: None,
                    params: std::collections::BTreeMap::new(),
                    env: None,
                }],
                edges: vec![],
            },
            prompts: vec![],
            eval_suite: None,
            attribution: Some(attribution),
            provenance: None,
        });
        let json = canonical_json(&prog).unwrap();
        let parsed: MorphObject = serde_json::from_str(&json).unwrap();
        if let MorphObject::Pipeline(p) = &parsed {
            let attr = p.attribution.as_ref().unwrap().get("node1").unwrap();
            assert_eq!(attr.instance_id.as_deref(), Some("inst-xyz"));
        } else {
            panic!("expected Pipeline");
        }
    }

    // --- paper-aligned new fields ---

    #[test]
    fn review_node_kind_roundtrip() {
        let prog = MorphObject::Pipeline(Pipeline {
            graph: PipelineGraph {
                nodes: vec![PipelineNode {
                    id: "review-step".into(),
                    kind: "review".into(),
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
        });
        let json = canonical_json(&prog).unwrap();
        let parsed: MorphObject = serde_json::from_str(&json).unwrap();
        if let MorphObject::Pipeline(p) = &parsed {
            assert_eq!(p.graph.nodes[0].kind, "review");
        } else {
            panic!("expected Pipeline");
        }
    }

    #[test]
    fn pipeline_node_env_roundtrip() {
        let mut env = std::collections::BTreeMap::new();
        env.insert("model".to_string(), serde_json::json!("gpt-4o"));
        env.insert("temperature".to_string(), serde_json::json!(0.7));
        let prog = MorphObject::Pipeline(Pipeline {
            graph: PipelineGraph {
                nodes: vec![PipelineNode {
                    id: "n1".into(),
                    kind: "prompt_call".into(),
                    ref_: None,
                    params: std::collections::BTreeMap::new(),
                    env: Some(env),
                }],
                edges: vec![],
            },
            prompts: vec![],
            eval_suite: None,
            attribution: None,
            provenance: None,
        });
        let json = canonical_json(&prog).unwrap();
        let parsed: MorphObject = serde_json::from_str(&json).unwrap();
        if let MorphObject::Pipeline(p) = &parsed {
            let node_env = p.graph.nodes[0].env.as_ref().unwrap();
            assert_eq!(node_env.get("model").unwrap(), &serde_json::json!("gpt-4o"));
        } else {
            panic!("expected Pipeline");
        }
    }

    #[test]
    fn attribution_actors_set_roundtrip() {
        let mut attribution = std::collections::BTreeMap::new();
        attribution.insert("review-node".to_string(), AttributionEntry {
            agent_id: "agent-1".into(),
            agent_version: None,
            instance_id: None,
            actors: Some(vec![
                ActorRef {
                    id: "agent-1".into(),
                    actor_type: "agent".into(),
                    env_config: None,
                },
                ActorRef {
                    id: "human-1".into(),
                    actor_type: "human".into(),
                    env_config: None,
                },
            ]),
        });
        let prog = MorphObject::Pipeline(Pipeline {
            graph: PipelineGraph {
                nodes: vec![PipelineNode {
                    id: "review-node".into(),
                    kind: "review".into(),
                    ref_: None,
                    params: std::collections::BTreeMap::new(),
                    env: None,
                }],
                edges: vec![],
            },
            prompts: vec![],
            eval_suite: None,
            attribution: Some(attribution),
            provenance: None,
        });
        let json = canonical_json(&prog).unwrap();
        let parsed: MorphObject = serde_json::from_str(&json).unwrap();
        if let MorphObject::Pipeline(p) = &parsed {
            let actors = p.attribution.as_ref().unwrap().get("review-node").unwrap()
                .actors.as_ref().unwrap();
            assert_eq!(actors.len(), 2);
            assert_eq!(actors[0].id, "agent-1");
            assert_eq!(actors[1].actor_type, "human");
        } else {
            panic!("expected Pipeline");
        }
    }

    // --- content_hash_git (Option B / 0.2) ---

    #[test]
    fn content_hash_git_deterministic() {
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"x": 1}),
        });
        let h1 = content_hash_git(&blob).unwrap();
        let h2 = content_hash_git(&blob).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn content_hash_git_different_from_content_hash() {
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"x": 1}),
        });
        let h_legacy = content_hash(&blob).unwrap();
        let h_git = content_hash_git(&blob).unwrap();
        assert_ne!(h_legacy, h_git, "Git format includes blob header so hashes differ");
    }

    #[test]
    fn content_hash_git_roundtrip_same_hash() {
        let blob = MorphObject::Blob(Blob {
            kind: "prompt".into(),
            content: serde_json::json!({"template": "Hello {{name}}"}),
        });
        let json = canonical_json(&blob).unwrap();
        let parsed: MorphObject = serde_json::from_str(&json).unwrap();
        assert_eq!(content_hash_git(&blob).unwrap(), content_hash_git(&parsed).unwrap());
    }
}
