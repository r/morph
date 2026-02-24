//! Identity program (v0-spec §5). Well-known Program with deterministic hash.

use crate::objects::{MorphObject, Program, ProgramGraph, ProgramNode};

/// Identity program: single identity node, no edges. I ∘ P = P ∘ I = P.
pub fn identity_program() -> MorphObject {
    MorphObject::Program(Program {
        graph: ProgramGraph {
            nodes: vec![ProgramNode {
                id: "passthrough".to_string(),
                kind: "identity".to_string(),
                ref_: None,
                params: std::collections::BTreeMap::new(),
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
    fn identity_program_hash_stable() {
        let p = identity_program();
        let h1 = content_hash(&p).unwrap();
        let h2 = content_hash(&p).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn identity_roundtrip_serialization() {
        let p = identity_program();
        let json = crate::canonical_json(&p).unwrap();
        let parsed: MorphObject = serde_json::from_str(&json).unwrap();
        assert_eq!(content_hash(&p).unwrap(), content_hash(&parsed).unwrap());
    }
}
