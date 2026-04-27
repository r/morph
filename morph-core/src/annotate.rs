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

/// Phase 6b: parse a comma-separated argument like
/// `"login:alpha, login:beta"` into the canonical `cases` list. Trims
/// whitespace and drops empties so callers don't have to worry about
/// trailing commas. Returns an empty vec when the arg has no real
/// values (signal to skip recording an annotation entirely).
pub fn parse_introduces_cases_arg(arg: &str) -> Vec<String> {
    arg.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Phase 6b: build (but don't store) an `introduces_cases` annotation
/// for `commit`. Pair with `store.put(&...)` at the call site. Returns
/// `None` when `cases` is empty so callers can compose without a
/// guard. `author` defaults to `"morph"` when `None` — the CLI passes
/// the current branch name as a provenance hint.
pub fn build_introduces_cases_annotation(
    commit: &Hash,
    cases: &[String],
    author: Option<String>,
) -> Option<MorphObject> {
    if cases.is_empty() {
        return None;
    }
    let mut data = BTreeMap::new();
    data.insert(
        "cases".to_string(),
        serde_json::Value::Array(
            cases.iter().map(|c| serde_json::Value::String(c.clone())).collect(),
        ),
    );
    Some(create_annotation(
        commit,
        None,
        "introduces_cases".into(),
        data,
        author,
    ))
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
    fn parse_introduces_cases_arg_handles_whitespace_and_empties() {
        assert!(parse_introduces_cases_arg("").is_empty());
        assert!(parse_introduces_cases_arg(" , , ").is_empty());
        assert_eq!(
            parse_introduces_cases_arg("login:alpha, login:beta ,, login:gamma"),
            vec![
                "login:alpha".to_string(),
                "login:beta".to_string(),
                "login:gamma".to_string(),
            ],
        );
    }

    #[test]
    fn build_introduces_cases_annotation_skips_empty_lists() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        std::fs::create_dir_all(store.objects_dir()).unwrap();
        let blob = MorphObject::Blob(Blob { kind: "x".into(), content: serde_json::json!({}) });
        let target = store.put(&blob).unwrap();
        assert!(build_introduces_cases_annotation(&target, &[], None).is_none());

        let cases = vec!["login:alpha".to_string(), "login:beta".to_string()];
        let ann = build_introduces_cases_annotation(&target, &cases, None).expect("non-empty cases yield annotation");
        if let MorphObject::Annotation(a) = ann {
            assert_eq!(a.kind, "introduces_cases");
            let arr = a.data.get("cases").and_then(|v| v.as_array()).unwrap();
            assert_eq!(arr.len(), 2);
        } else {
            panic!("expected annotation");
        }
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
