//! Identity pipeline (v0-spec §5). Well-known Pipeline with deterministic hash.

use crate::objects::{MorphObject, Pipeline, PipelineGraph, PipelineNode};

/// Identity pipeline: single identity node, no edges. I ∘ P = P ∘ I = P.
pub fn identity_pipeline() -> MorphObject {
    MorphObject::Pipeline(Pipeline {
        graph: PipelineGraph {
            nodes: vec![PipelineNode {
                id: "passthrough".to_string(),
                kind: "identity".to_string(),
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::content_hash;

    #[test]
    fn identity_pipeline_hash_stable() {
        let p = identity_pipeline();
        let h1 = content_hash(&p).unwrap();
        let h2 = content_hash(&p).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn identity_roundtrip_serialization() {
        let p = identity_pipeline();
        let json = crate::canonical_json(&p).unwrap();
        let parsed: MorphObject = serde_json::from_str(&json).unwrap();
        assert_eq!(content_hash(&p).unwrap(), content_hash(&parsed).unwrap());
    }
}
