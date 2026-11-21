use super::super::{ReplayItem, output_proof::OutputProof};
use super::*;
use serde_json::json;

#[test]
fn only_one_proven_removed_top_level_object_name_reaches_the_local_probe() {
    for kind in ["message", "reasoning", "function_call", "custom_tool_call"] {
        let item = json!({"type":kind,"id":"synthetic","synthetic_object":{"private":"value"},
            "other_object":{},"opaque_string":"private"});
        let proof = OutputProof::new(&ReplayItem::new(&item));
        let mut probe = FieldProbe::default();
        probe.capture(&item);
        let mut replay = item.clone();
        replay.as_object_mut().unwrap().remove("synthetic_object");
        let mismatch = proof.mismatch(0, Some(&replay));
        assert_eq!(
            probe.removed_name(&mismatch, Some(&replay)),
            Some("synthetic_object")
        );
        assert_eq!(probe.removed_name(&mismatch, None), None);
        assert_eq!(
            probe.removed_name(&proof.mismatch(1, Some(&replay)), Some(&replay)),
            None
        );
        let exported = serde_json::to_string(&mismatch).unwrap();
        for hidden in ["synthetic", "private", "other_object", "opaque_string"] {
            assert!(!exported.contains(hidden));
        }
        for field in ["id", "other_object", "opaque_string"] {
            let mut edited = replay.clone();
            edited.as_object_mut().unwrap().remove(field);
            assert_eq!(
                probe.removed_name(&proof.mismatch(0, Some(&edited)), Some(&edited)),
                None
            );
        }
        // The same masking applies when a client edits an object instead of omitting it.
        replay["synthetic_object"] = json!({"changed":true});
        assert_eq!(
            probe.removed_name(&proof.mismatch(0, Some(&replay)), Some(&replay)),
            None
        );
    }
}

#[test]
fn unsafe_names_and_truncated_differences_are_not_displayed() {
    for name in [
        "",
        "9prefix",
        "mail@example.com",
        "x\ninjected",
        "\x1b[31m",
        "a".repeat(65).as_str(),
    ] {
        let item = json!({"type":"reasoning", (name):{}});
        let replay = json!({"type":"reasoning"});
        let mut probe = FieldProbe::default();
        probe.capture(&item);
        let proof = OutputProof::new(&ReplayItem::new(&item));
        assert!(
            probe
                .removed_name(&proof.mismatch(0, Some(&replay)), Some(&replay))
                .is_none()
        );
    }
    let mut item = json!({"type":"reasoning","omitted":{}});
    for i in 0..MAX_UNKNOWN_FIELDS {
        item[format!("ext_{i}")] = json!({});
    }
    let mut probe = FieldProbe::default();
    probe.capture(&item);
    assert_eq!(probe.0.len(), MAX_UNKNOWN_FIELDS);
    let proof = OutputProof::new(&ReplayItem::new(&item));
    item.as_object_mut().unwrap().remove("omitted");
    assert!(
        probe
            .removed_name(&proof.mismatch(0, Some(&item)), Some(&item))
            .is_none()
    );
}
