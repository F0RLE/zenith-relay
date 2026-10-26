use super::*;
use std::fs;
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
    library: PathBuf,
    recovery: PathBuf,
}

impl Fixture {
    fn profile(&self) -> PathBuf {
        self.library.join(format!("{PROFILE_ID}.json"))
    }

    fn new() -> Self {
        // Resolve platform-owned temp aliases before creating the isolated fixture.
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("zenith-relay-claude-{}", uuid::Uuid::new_v4()));
        let library = root.join("configLibrary");
        fs::create_dir_all(&library).unwrap();
        Self {
            recovery: root.join("recovery.json"),
            root,
            library,
        }
    }

    fn write_meta(&self, applied: Option<&str>) {
        let mut meta = json!({
            "entries": [{ "id": "other", "name": "Other", "custom": { "keep": true } }],
            "custom": { "theme": "dark" }
        });
        if let Some(applied) = applied {
            meta["appliedId"] = json!(applied);
        }
        fs::write(
            self.library.join("_meta.json"),
            serde_json::to_vec(&meta).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn config(model: &str) -> Value {
    super::super::models::gateway_config(
        "http://127.0.0.1:14998",
        "synthetic-key",
        vec![json!({ "name": model })],
    )
}

#[test]
fn attach_preserves_foreign_metadata_and_restore_removes_only_owned_profile() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    let original_meta = fs::read(fixture.library.join("_meta.json")).unwrap();
    let foreign_path = fixture.library.join("other.json");
    let foreign_bytes = br#"{ "inferenceProvider": "bedrock", "custom": true }"#;
    fs::write(&foreign_path, foreign_bytes).unwrap();
    assert!(
        apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/model"),
            true
        )
        .unwrap()
    );
    let metadata = fs::read(fixture.library.join("_meta.json")).unwrap();
    let metadata_value: Value = serde_json::from_slice(&metadata).unwrap();
    assert_eq!(metadata_value["custom"], json!({ "theme": "dark" }));
    assert_eq!(
        metadata_value["entries"][0]["custom"],
        json!({ "keep": true })
    );
    assert_eq!(metadata_value["appliedId"], PROFILE_ID);
    assert!(
        metadata_value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == "other")
    );
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    let restored: Value =
        serde_json::from_slice(&fs::read(fixture.library.join("_meta.json")).unwrap()).unwrap();
    let original: Value = serde_json::from_slice(&original_meta).unwrap();
    assert_eq!(restored, original);
    assert_eq!(fs::read(foreign_path).unwrap(), foreign_bytes);
    assert!(!fixture.profile().exists());
    assert!(!fixture.recovery.exists());
}

#[test]
fn inactive_refresh_does_not_overwrite_a_newer_manual_selection() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    assert!(
        apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/model"),
            true
        )
        .unwrap()
    );
    let mut meta: Value =
        serde_json::from_slice(&fs::read(fixture.library.join("_meta.json")).unwrap()).unwrap();
    meta["appliedId"] = json!("other");
    fs::write(
        fixture.library.join("_meta.json"),
        serde_json::to_vec(&meta).unwrap(),
    )
    .unwrap();
    let before = fs::read(fixture.profile()).unwrap();
    assert!(
        !apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/new-model"),
            false
        )
        .unwrap()
    );
    assert_eq!(fs::read(fixture.profile()).unwrap(), before);
}

#[test]
fn tampered_owned_profile_blocks_restore() {
    let fixture = Fixture::new();
    fixture.write_meta(None);
    assert!(
        apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/model"),
            true
        )
        .unwrap()
    );
    fs::write(fixture.profile(), br"{}").unwrap();
    assert!(check_restore(&fixture.library, &fixture.recovery).is_err());
    assert_eq!(
        restore(&fixture.library, &fixture.recovery)
            .unwrap_err()
            .code,
        ErrorCode::ProfileRestoreBlocked
    );
}

#[test]
fn fresh_attach_is_idempotent_and_recovery_contains_no_request_key() {
    let fixture = Fixture::new();
    let mut profile = config("pool/model");
    profile["inferenceGatewayApiKey"] = json!("synthetic-request-key");
    assert!(apply(&fixture.library, &fixture.recovery, &profile, true).unwrap());
    let before = fs::read(&fixture.recovery).unwrap();
    assert!(!String::from_utf8_lossy(&before).contains("synthetic-request-key"));
    assert!(!apply(&fixture.library, &fixture.recovery, &profile, true).unwrap());
    assert_eq!(fs::read(&fixture.recovery).unwrap(), before);
    assert!(
        inspect(&fixture.library, &fixture.recovery)
            .unwrap()
            .configured
    );
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    let meta = metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap();
    assert_eq!(meta, json!({ "entries": [] }));
    assert!(!restore(&fixture.library, &fixture.recovery).unwrap());
}

#[test]
fn active_refresh_keeps_exact_model_order_and_original_selection() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    apply(
        &fixture.library,
        &fixture.recovery,
        &config("pool/old"),
        true,
    )
    .unwrap();
    let updated = json!({ "inferenceModels": [{ "name": "pool/z" }, { "name": "pool/a" }] });
    assert!(!apply(&fixture.library, &fixture.recovery, &updated, false).unwrap());
    let status = inspect(&fixture.library, &fixture.recovery).unwrap();
    assert!(status.configured);
    assert_eq!(status.model_count, 2);
    assert_eq!(
        parse(&fs::read(fixture.profile()).unwrap()).unwrap(),
        updated
    );
    restore(&fixture.library, &fixture.recovery).unwrap();
    let meta = metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap();
    assert_eq!(meta["appliedId"], "other");
}

#[test]
fn refreshing_effort_variants_keeps_three_models_and_restorable_ownership() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    let entries: Vec<_> = ["first", "second", "third"]
        .into_iter()
        .map(|model| json!({"name": zenith_relay_core::claude_desktop_model_alias(model)}))
        .collect();
    let mut expanded = config("unused");
    expanded["inferenceModels"] = json!(
        entries
            .iter()
            .flat_map(|entry| {
                let name = entry["name"].as_str().unwrap();
                [
                    entry.clone(),
                    json!({"name":format!("{name}-effort-low"),"maxEffort":"low"}),
                    json!({"name":format!("{name}-effort-high"),"maxEffort":"high"}),
                ]
            })
            .collect::<Vec<_>>()
    );
    assert!(apply(&fixture.library, &fixture.recovery, &expanded, true).unwrap());
    let mut updated = config("unused");
    updated["inferenceModels"] = json!(entries);
    apply(&fixture.library, &fixture.recovery, &updated, false).unwrap();
    assert_eq!(
        inspect(&fixture.library, &fixture.recovery)
            .unwrap()
            .model_count,
        3
    );
    assert_eq!(
        parse(&fs::read(fixture.profile()).unwrap()).unwrap(),
        updated
    );
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    let meta = metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap();
    assert_eq!(meta["appliedId"], "other");
}

#[test]
fn restore_preserves_a_newer_foreign_selection_and_metadata() {
    let fixture = Fixture::new();
    apply(
        &fixture.library,
        &fixture.recovery,
        &config("pool/model"),
        true,
    )
    .unwrap();
    let meta_path = fixture.library.join("_meta.json");
    let mut meta = metadata(Some(&fs::read(&meta_path).unwrap())).unwrap();
    meta["entries"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "new-login", "name": "New login" }));
    meta["appliedId"] = json!("new-login");
    meta["custom"] = json!({ "newer": true });
    fs::write(&meta_path, serialize(&meta).unwrap()).unwrap();
    restore(&fixture.library, &fixture.recovery).unwrap();
    let restored = metadata(Some(&fs::read(&meta_path).unwrap())).unwrap();
    assert_eq!(restored["appliedId"], "new-login");
    assert_eq!(restored["custom"], json!({ "newer": true }));
    assert_eq!(
        restored["entries"],
        json!([{ "id": "new-login", "name": "New login" }])
    );
}

#[test]
fn unowned_id_collisions_never_overwrite_configuration() {
    for meta in [
        json!({ "entries": [], "appliedId": PROFILE_ID }),
        json!({ "entries": [{ "id": PROFILE_ID, "name": "Foreign" }] }),
    ] {
        let fixture = Fixture::new();
        let meta_path = fixture.library.join("_meta.json");
        let bytes = serialize(&meta).unwrap();
        fs::write(&meta_path, &bytes).unwrap();
        assert!(
            apply(
                &fixture.library,
                &fixture.recovery,
                &config("pool/model"),
                true
            )
            .is_err()
        );
        assert_eq!(fs::read(&meta_path).unwrap(), bytes);
        assert!(!fixture.recovery.exists());
    }
    let fixture = Fixture::new();
    let profile_path = fixture.profile();
    fs::write(&profile_path, b"foreign profile bytes").unwrap();
    assert!(
        apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/model"),
            true
        )
        .is_err()
    );
    assert_eq!(fs::read(profile_path).unwrap(), b"foreign profile bytes");
    assert!(!fixture.recovery.exists());
}

#[test]
fn renamed_owned_entry_or_removed_previous_selection_blocks_restore() {
    for rename in [true, false] {
        let fixture = Fixture::new();
        fixture.write_meta(Some("other"));
        apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/model"),
            true,
        )
        .unwrap();
        let meta_path = fixture.library.join("_meta.json");
        let mut meta = metadata(Some(&fs::read(&meta_path).unwrap())).unwrap();
        if rename {
            meta["entries"][1]["name"] = json!("Renamed by user");
        } else {
            meta["entries"].as_array_mut().unwrap().remove(0);
        }
        let bytes = serialize(&meta).unwrap();
        fs::write(&meta_path, &bytes).unwrap();
        assert!(restore(&fixture.library, &fixture.recovery).is_err());
        assert_eq!(fs::read(meta_path).unwrap(), bytes);
        assert!(fixture.recovery.exists());
        assert!(fixture.profile().exists());
    }
}

#[test]
fn interrupted_attach_can_be_retried_or_restored() {
    for write_profile in [false, true] {
        let fixture = Fixture::new();
        fixture.write_meta(Some("other"));
        let bytes = serialize(&config("pool/model")).unwrap();
        let record = Recovery {
            library: fixture.library.clone(),
            profile_id: PROFILE_ID.to_owned(),
            previous_applied_id: Some("other".into()),
            profile_hashes: vec![hash(&bytes)],
            snapshot_name: None,
            created_at_ms: None,
            pending: false,
        };
        fs::write(&fixture.recovery, serialize(&record).unwrap()).unwrap();
        if write_profile {
            fs::write(fixture.profile(), &bytes).unwrap();
        }
        assert!(
            !inspect(&fixture.library, &fixture.recovery)
                .unwrap()
                .configured
        );
        if write_profile {
            assert!(
                !apply(
                    &fixture.library,
                    &fixture.recovery,
                    &config("pool/model"),
                    true
                )
                .unwrap()
            );
            assert!(
                inspect(&fixture.library, &fixture.recovery)
                    .unwrap()
                    .configured
            );
        }
        assert!(restore(&fixture.library, &fixture.recovery).unwrap());
        assert!(!fixture.recovery.exists());
    }
}

#[test]
fn malformed_recovery_is_not_trusted() {
    for (previous, profile_hashes) in [
        (Some(PROFILE_ID), vec![hash(b"synthetic")]),
        (Some("../foreign"), vec![hash(b"synthetic")]),
        (None, vec!["invalid-fingerprint".into()]),
        (None, Vec::new()),
        (None, vec![hash(b"synthetic"); 3]),
    ] {
        let fixture = Fixture::new();
        let record = Recovery {
            library: fixture.library.clone(),
            profile_id: PROFILE_ID.to_owned(),
            previous_applied_id: previous.map(str::to_owned),
            profile_hashes,
            snapshot_name: None,
            created_at_ms: None,
            pending: false,
        };
        fs::write(&fixture.recovery, serialize(&record).unwrap()).unwrap();
        assert!(inspect(&fixture.library, &fixture.recovery).is_err());
        assert!(restore(&fixture.library, &fixture.recovery).is_err());
        assert!(fixture.recovery.exists());
    }
}

#[cfg(unix)]
#[test]
fn symlinked_library_is_never_modified() {
    let fixture = Fixture::new();
    let link = fixture.root.join("linked-library");
    std::os::unix::fs::symlink(&fixture.library, &link).unwrap();
    assert!(apply(&link, &fixture.recovery, &config("pool/model"), true).is_err());
    assert!(!fixture.profile().exists());
}

#[test]
fn metadata_rejects_duplicate_or_unsafe_ids() {
    let fixture = Fixture::new();
    fs::write(
        fixture.library.join("_meta.json"),
        br#"{"entries":[{"id":"a","name":"A"},{"id":"a","name":"B"}]}"#,
    )
    .unwrap();
    assert_eq!(
        inspect(&fixture.library, &fixture.recovery)
            .unwrap_err()
            .code,
        ErrorCode::ProfileRestoreBlocked
    );
    fs::write(
        fixture.library.join("_meta.json"),
        br#"{"entries":[{"id":"../secret","name":"A"}]}"#,
    )
    .unwrap();
    assert_eq!(
        inspect(&fixture.library, &fixture.recovery)
            .unwrap_err()
            .code,
        ErrorCode::ProfileRestoreBlocked
    );
}

#[test]
fn selected_profile_is_readable_by_desktop_uuid_bootstrap() {
    let fixture = Fixture::new();
    apply(
        &fixture.library,
        &fixture.recovery,
        &config("pool/model"),
        true,
    )
    .unwrap();
    let meta = metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap();
    let id = meta["appliedId"].as_str().unwrap();
    assert_eq!(id.len(), 36);
    assert!(
        id.bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
    );
    assert!(uuid::Uuid::parse_str(id).is_ok());
    assert!(fixture.library.join(format!("{id}.json")).is_file());
}

#[test]
fn orphaned_legacy_profile_is_cleaned_up_on_connect() {
    // An older Relay version left a "zenith-relay" entry and profile with no
    // recovery record. Connecting with the current UUID profile must remove the
    // orphaned entry, delete the legacy file, and leave a clean UUID profile.
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    let meta_path = fixture.library.join("_meta.json");
    let mut meta = metadata(Some(&fs::read(&meta_path).unwrap())).unwrap();
    meta["entries"].as_array_mut().unwrap().push(json!({
        "id": "zenith-relay", "name": "Zenith Relay"
    }));
    meta["appliedId"] = json!("other");
    fs::write(&meta_path, serialize(&meta).unwrap()).unwrap();
    let legacy_path = fixture.library.join("zenith-relay.json");
    fs::write(&legacy_path, serialize(&config("pool/old")).unwrap()).unwrap();
    let foreign_path = fixture.library.join("other.json");
    fs::write(&foreign_path, b"synthetic foreign bytes").unwrap();
    assert!(!fixture.recovery.exists());
    // Connect creates the UUID profile and cleans up the orphaned legacy file.
    assert!(
        apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/new"),
            true
        )
        .unwrap()
    );
    assert!(
        inspect(&fixture.library, &fixture.recovery)
            .unwrap()
            .configured
    );
    assert!(!legacy_path.exists(), "orphaned legacy file is deleted");
    let meta = metadata(Some(&fs::read(&meta_path).unwrap())).unwrap();
    assert_eq!(meta["appliedId"], PROFILE_ID);
    assert!(
        !meta["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == "zenith-relay"),
        "legacy entry is removed"
    );
    assert_eq!(meta["custom"], json!({"theme": "dark"}));
    assert_eq!(fs::read(&foreign_path).unwrap(), b"synthetic foreign bytes");
    // Restore returns to the pre-connection selection and removes the UUID profile.
    restore(&fixture.library, &fixture.recovery).unwrap();
    let meta = metadata(Some(&fs::read(&meta_path).unwrap())).unwrap();
    assert_eq!(meta["appliedId"], "other");
    assert!(!fixture.profile().exists());
    assert!(!fixture.recovery.exists());
    assert!(
        !meta["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == "zenith-relay")
    );
}

mod snapshots;

#[test]
fn lost_recovery_with_owned_entry_is_reclaimed_on_reconnect() {
    // The recovery record was lost (data cleared, reinstall) but the profile
    // still matches the current gateway. Reconnecting recreates recovery
    // without trusting the display name alone.
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    // First connect normally.
    assert!(
        apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/model"),
            true
        )
        .unwrap()
    );
    assert!(
        inspect(&fixture.library, &fixture.recovery)
            .unwrap()
            .configured
    );
    // Simulate losing the recovery record.
    fs::remove_file(&fixture.recovery).unwrap();
    assert!(
        !inspect(&fixture.library, &fixture.recovery)
            .unwrap()
            .configured
    );
    // Reconnect must succeed and recreate the recovery record.
    assert!(
        apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/new-model"),
            true
        )
        .unwrap()
    );
    let status = inspect(&fixture.library, &fixture.recovery).unwrap();
    assert!(status.configured);
    assert_eq!(status.model_count, 1);
    assert!(fixture.recovery.exists());
    // Restore still works after reclaim.
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    assert!(!fixture.profile().exists());
    assert!(!fixture.recovery.exists());
}

#[test]
fn identical_profile_recreates_lost_recovery_without_a_self_reference() {
    let fixture = Fixture::new();
    let config = config("pool/model");
    apply(&fixture.library, &fixture.recovery, &config, true).unwrap();
    fs::remove_file(&fixture.recovery).unwrap();
    check_attach(&fixture.library, &fixture.recovery, &config).unwrap();
    assert!(apply(&fixture.library, &fixture.recovery, &config, true).unwrap());
    assert!(
        inspect(&fixture.library, &fixture.recovery)
            .unwrap()
            .configured
    );
    assert!(
        recovery(&fixture.recovery, &fixture.library)
            .unwrap()
            .unwrap()
            .previous_applied_id
            .is_none()
    );
    restore(&fixture.library, &fixture.recovery).unwrap();
    let meta = metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap();
    assert!(meta.get("appliedId").is_none());
}

#[test]
fn lost_recovery_accepts_effort_choices_but_rejects_unknown_model_settings() {
    let fixture = Fixture::new();
    let mut selected = config("pool/model");
    selected["inferenceModels"][0]["maxEffort"] = json!("high");
    apply(&fixture.library, &fixture.recovery, &selected, true).unwrap();
    fs::remove_file(&fixture.recovery).unwrap();
    let updated = config("pool/new-model");
    check_attach(&fixture.library, &fixture.recovery, &updated).unwrap();
    let before = fs::read(fixture.profile()).unwrap();
    for (field, value) in [("maxEffort", json!("unknown")), ("supports1m", json!(true))] {
        let mut foreign = selected.clone();
        foreign["inferenceModels"][0][field] = value;
        let bytes = serialize(&foreign).unwrap();
        fs::write(fixture.profile(), &bytes).unwrap();
        assert!(check_attach(&fixture.library, &fixture.recovery, &updated).is_err());
        assert_eq!(fs::read(fixture.profile()).unwrap(), bytes);
        assert!(!fixture.recovery.exists());
    }
    fs::write(fixture.profile(), &before).unwrap();
    apply(&fixture.library, &fixture.recovery, &updated, true).unwrap();
    restore(&fixture.library, &fixture.recovery).unwrap();
}

#[test]
fn lost_recovery_never_reclaims_a_foreign_gateway_or_custom_settings() {
    for (field, value) in [
        ("inferenceGatewayApiKey", json!("other-synthetic-key")),
        ("inferenceGatewayBaseUrl", json!("https://foreign.example")),
        ("toolSearchEnabled", json!(false)),
        ("autoModeEnabled", json!(true)),
        ("autoModeEnabled", Value::Null),
        ("modelCatalogEnabled", json!(false)),
        (
            "modelCatalogUrl",
            json!("https://foreign.example/catalog.json"),
        ),
        ("custom", json!(true)),
    ] {
        let fixture = Fixture::new();
        let original = config("pool/model");
        apply(&fixture.library, &fixture.recovery, &original, true).unwrap();
        fs::remove_file(&fixture.recovery).unwrap();
        let mut foreign = original.clone();
        foreign[field] = value;
        let bytes = serialize(&foreign).unwrap();
        fs::write(fixture.profile(), &bytes).unwrap();
        assert!(check_attach(&fixture.library, &fixture.recovery, &original).is_err());
        assert!(apply(&fixture.library, &fixture.recovery, &original, true).is_err());
        assert_eq!(fs::read(fixture.profile()).unwrap(), bytes);
        assert!(!fixture.recovery.exists());
    }
}

#[test]
fn old_owned_profile_upgrades_tool_search_without_touching_foreign_settings() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    let mut old = config("pool/model");
    old.as_object_mut().unwrap().remove("toolSearchEnabled");
    apply(&fixture.library, &fixture.recovery, &old, true).unwrap();
    let before = fs::read(fixture.profile()).unwrap();
    let updated = config("pool/model");
    check_attach(&fixture.library, &fixture.recovery, &updated).unwrap();
    assert_eq!(fs::read(fixture.profile()).unwrap(), before);
    apply(&fixture.library, &fixture.recovery, &updated, true).unwrap();
    assert_eq!(
        parse(&fs::read(fixture.profile()).unwrap()).unwrap(),
        updated
    );
    restore(&fixture.library, &fixture.recovery).unwrap();
    let meta = metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap();
    assert_eq!(meta["appliedId"], "other");

    let fixture = Fixture::new();
    apply(&fixture.library, &fixture.recovery, &old, true).unwrap();
    // The same exact static gateway can also reclaim an older owned profile
    // after its recovery record was lost; an explicit false was tested above.
    fs::remove_file(&fixture.recovery).unwrap();
    check_attach(&fixture.library, &fixture.recovery, &updated).unwrap();
    apply(&fixture.library, &fixture.recovery, &updated, true).unwrap();
    assert_eq!(
        parse(&fs::read(fixture.profile()).unwrap()).unwrap(),
        updated
    );
    restore(&fixture.library, &fixture.recovery).unwrap();
    assert!(!fixture.profile().exists());
}

#[test]
fn old_owned_profile_disables_unsupported_auto_mode_with_or_without_recovery() {
    for keep_recovery in [true, false] {
        let fixture = Fixture::new();
        fixture.write_meta(Some("other"));
        let mut old = config("pool/model");
        old.as_object_mut().unwrap().remove("autoModeEnabled");
        apply(&fixture.library, &fixture.recovery, &old, true).unwrap();
        if !keep_recovery {
            fs::remove_file(&fixture.recovery).unwrap();
        }
        let before = fs::read(fixture.profile()).unwrap();
        let updated = config("pool/model");
        check_attach(&fixture.library, &fixture.recovery, &updated).unwrap();
        assert_eq!(fs::read(fixture.profile()).unwrap(), before);
        apply(&fixture.library, &fixture.recovery, &updated, true).unwrap();
        assert_eq!(
            parse(&fs::read(fixture.profile()).unwrap()).unwrap(),
            updated
        );
        restore(&fixture.library, &fixture.recovery).unwrap();
        assert!(!fixture.profile().exists());
        let meta = metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap();
        if keep_recovery {
            assert_eq!(meta["appliedId"], "other");
        }
        assert!(
            meta["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["id"] == "other")
        );
    }
}

#[test]
fn active_legacy_cleanup_does_not_leave_a_deleted_restore_target() {
    let fixture = Fixture::new();
    let legacy = config("pool/old");
    fs::write(
        fixture.library.join("zenith-relay.json"),
        serialize(&legacy).unwrap(),
    )
    .unwrap();
    fs::write(
        fixture.library.join("_meta.json"),
        serialize(&json!({
            "entries": [{"id":"zenith-relay", "name":"Zenith Relay"}],
            "appliedId": "zenith-relay"
        }))
        .unwrap(),
    )
    .unwrap();
    apply(
        &fixture.library,
        &fixture.recovery,
        &config("pool/new"),
        true,
    )
    .unwrap();
    restore(&fixture.library, &fixture.recovery).unwrap();
    let meta = metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap();
    assert!(meta.get("appliedId").is_none());
    assert_eq!(meta["entries"], json!([]));
}

#[test]
fn legacy_name_alone_does_not_authorize_deleting_foreign_configuration() {
    let fixture = Fixture::new();
    let legacy_path = fixture.library.join("zenith-relay.json");
    let bytes = br#"{"inferenceProvider":"bedrock","custom":true}"#;
    fs::write(&legacy_path, bytes).unwrap();
    fs::write(
        fixture.library.join("_meta.json"),
        serialize(&json!({
            "entries": [{"id":"zenith-relay", "name":"Zenith Relay"}],
            "appliedId": "zenith-relay"
        }))
        .unwrap(),
    )
    .unwrap();
    apply(
        &fixture.library,
        &fixture.recovery,
        &config("pool/new"),
        true,
    )
    .unwrap();
    assert_eq!(fs::read(&legacy_path).unwrap(), bytes);
    restore(&fixture.library, &fixture.recovery).unwrap();
    let meta = metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap();
    assert_eq!(meta["appliedId"], "zenith-relay");
    assert_eq!(fs::read(&legacy_path).unwrap(), bytes);
}

#[test]
fn reclaimed_profile_preserves_a_newer_manual_selection_on_restore() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    apply(
        &fixture.library,
        &fixture.recovery,
        &config("pool/model"),
        true,
    )
    .unwrap();
    fs::remove_file(&fixture.recovery).unwrap();
    let meta_path = fixture.library.join("_meta.json");
    let mut meta = metadata(Some(&fs::read(&meta_path).unwrap())).unwrap();
    meta["appliedId"] = json!("other");
    fs::write(&meta_path, serialize(&meta).unwrap()).unwrap();
    apply(
        &fixture.library,
        &fixture.recovery,
        &config("pool/new"),
        true,
    )
    .unwrap();
    assert!(check_restore(&fixture.library, &fixture.recovery).unwrap());
    restore(&fixture.library, &fixture.recovery).unwrap();
    let meta = metadata(Some(&fs::read(&meta_path).unwrap())).unwrap();
    assert_eq!(meta["appliedId"], "other");
}
