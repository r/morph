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
pub fn canonical_json(obj: &crate::objects::MorphObject) -> Result<String, crate::store::MorphError> {
    let value = serde_json::to_value(obj).map_err(|e| crate::store::MorphError::Serialization(e.to_string()))?;
    serde_json::to_string(&value).map_err(|e| crate::store::MorphError::Serialization(e.to_string()))
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
