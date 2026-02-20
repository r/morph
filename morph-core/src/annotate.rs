//! Annotations: attach metadata to any object or sub-target (v0-spec §4.10).

use crate::objects::{Annotation, MorphObject};
use crate::store::{MorphError, ObjectType, Store};
use crate::Hash;
use chrono::Utc;
use std::collections::BTreeMap;

/// Create an Annotation object (caller puts it). Returns the object so store.put can be used.
pub fn create_annotation(
    target: &Hash,
    target_sub: Option<String>,
    kind: String,
    data: BTreeMap<String, serde_json::Value>,
    author: Option<String>,
) -> MorphObject {
    let timestamp = Utc::now().to_rfc3339();
    let author = author.unwrap_or_else(|| "morph".to_string());
    MorphObject::Annotation(Annotation {
        target: target.to_string(),
        target_sub,
        kind,
        data,
        author,
        timestamp,
    })
}

/// List all annotations targeting the given object (and optionally target_sub).
pub fn list_annotations(
    store: &dyn Store,
    target: &Hash,
    target_sub: Option<&str>,
) -> Result<Vec<(Hash, Annotation)>, MorphError> {
    let target_str = target.to_string();
    let hashes = store.list(ObjectType::Annotation)?;
    let mut out = Vec::new();
    for h in hashes {
        let obj = store.get(&h)?;
        if let MorphObject::Annotation(a) = obj {
            if a.target != target_str {
                continue;
            }
            if let Some(sub) = target_sub {
                if a.target_sub.as_deref() != Some(sub) {
                    continue;
                }
            } else if a.target_sub.is_some() {
                continue;
            }
            out.push((h, a));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::Blob;
    use crate::store::FsStore;

    #[test]
    fn create_and_list_annotation() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        std::fs::create_dir_all(store.objects_dir()).unwrap();

        let blob = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({}) });
        let target_hash = store.put(&blob).unwrap();

        let mut data = BTreeMap::new();
        data.insert("rating".to_string(), serde_json::json!("good"));
        let ann = create_annotation(&target_hash, None, "feedback".into(), data, None);
        let ann_hash = store.put(&ann).unwrap();

        let list = list_annotations(&store, &target_hash, None).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, ann_hash);
        assert_eq!(list[0].1.kind, "feedback");
    }

    #[test]
    fn list_filters_by_target_sub() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        std::fs::create_dir_all(store.objects_dir()).unwrap();

        let blob = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({}) });
        let target_hash = store.put(&blob).unwrap();

        let ann1 = create_annotation(&target_hash, Some("evt_1".into()), "bookmark".into(), BTreeMap::new(), None);
        let ann2 = create_annotation(&target_hash, Some("evt_2".into()), "note".into(), BTreeMap::new(), None);
        store.put(&ann1).unwrap();
        store.put(&ann2).unwrap();

        let list_all = list_annotations(&store, &target_hash, None).unwrap();
        assert_eq!(list_all.len(), 0);

        let list_evt1 = list_annotations(&store, &target_hash, Some("evt_1")).unwrap();
        assert_eq!(list_evt1.len(), 1);
        assert_eq!(list_evt1[0].1.kind, "bookmark");
    }
}
