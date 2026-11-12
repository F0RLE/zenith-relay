use super::*;

#[test]
fn explicit_reconnect_repairs_stale_ownership_without_losing_the_original_snapshot() {
    for already_current in [false, true] {
        let fixture = Fixture::new();
        fixture.write_meta(Some("other"));
        let original_meta = fs::read(fixture.library.join("_meta.json")).unwrap();
        create_snapshot(&fixture.library, &fixture.recovery, "Original profile").unwrap();
        apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/old"),
            true,
        )
        .unwrap();
        let before = recovery(&fixture.recovery, &fixture.library)
            .unwrap()
            .unwrap();
        let desired = config("pool/current");
        let changed = if already_current {
            desired.clone()
        } else {
            config("pool/intermediate")
        };
        fs::write(fixture.profile(), serialize(&changed).unwrap()).unwrap();
        let stale_record = fs::read(&fixture.recovery).unwrap();
        let changed_bytes = fs::read(fixture.profile()).unwrap();

        assert!(
            !inspect(&fixture.library, &fixture.recovery)
                .unwrap()
                .configured
        );
        assert!(check_restore(&fixture.library, &fixture.recovery).unwrap());
        assert!(!apply(&fixture.library, &fixture.recovery, &desired, false).unwrap());
        assert_eq!(fs::read(&fixture.recovery).unwrap(), stale_record);
        assert_eq!(fs::read(fixture.profile()).unwrap(), changed_bytes);

        check_attach(&fixture.library, &fixture.recovery, &desired).unwrap();
        assert!(!apply(&fixture.library, &fixture.recovery, &desired, true).unwrap());
        assert!(
            inspect(&fixture.library, &fixture.recovery)
                .unwrap()
                .configured
        );
        assert_eq!(
            fs::read(fixture.profile()).unwrap(),
            serialize(&desired).unwrap()
        );
        let repaired = recovery(&fixture.recovery, &fixture.library)
            .unwrap()
            .unwrap();
        assert_eq!(repaired.previous_applied_id, before.previous_applied_id);
        assert_eq!(repaired.snapshot_name, before.snapshot_name);
        assert_eq!(repaired.created_at_ms, before.created_at_ms);
        assert!(owns_profile(&repaired, Some(&changed_bytes)));
        assert!(owns_profile(
            &repaired,
            Some(&fs::read(fixture.profile()).unwrap())
        ));
        assert!(restore(&fixture.library, &fixture.recovery).unwrap());
        assert_eq!(
            parse(&fs::read(fixture.library.join("_meta.json")).unwrap()).unwrap(),
            parse(&original_meta).unwrap()
        );
    }
}

#[test]
fn explicit_reconnect_preserves_external_preferences_and_the_original_snapshot() {
    let desired = config("pool/current");
    let changes = [
        ("inferenceGatewayApiKey", json!("synthetic-foreign-key")),
        ("inferenceGatewayBaseUrl", json!("https://example.invalid")),
        ("toolSearchEnabled", json!(false)),
        ("autoModeEnabled", json!(true)),
        ("custom", json!(true)),
        (
            "inferenceModels",
            json!([{"name":"pool/model","custom":true}]),
        ),
    ];
    for (field, value) in changes {
        let fixture = Fixture::new();
        fixture.write_meta(Some("other"));
        create_snapshot(&fixture.library, &fixture.recovery, "Original settings").unwrap();
        apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/old"),
            true,
        )
        .unwrap();
        let mut changed = desired.clone();
        changed[field] = value;
        let bytes = serialize(&changed).unwrap();
        fs::write(fixture.profile(), &bytes).unwrap();
        let before = fs::read(&fixture.recovery).unwrap();
        assert!(!apply(&fixture.library, &fixture.recovery, &desired, false).unwrap());
        assert_eq!(fs::read(fixture.profile()).unwrap(), bytes);
        assert_eq!(fs::read(&fixture.recovery).unwrap(), before);
        assert!(
            check_attach(&fixture.library, &fixture.recovery, &desired).is_ok(),
            "{field}"
        );
        assert!(
            !apply(&fixture.library, &fixture.recovery, &desired, true).unwrap(),
            "{field}"
        );
        let attached = parse(&fs::read(fixture.profile()).unwrap()).unwrap();
        assert_eq!(
            attached["inferenceGatewayApiKey"],
            desired["inferenceGatewayApiKey"]
        );
        assert_eq!(
            attached["inferenceGatewayBaseUrl"],
            desired["inferenceGatewayBaseUrl"]
        );
        assert_eq!(attached["inferenceModels"], desired["inferenceModels"]);
        let preference = !field.starts_with("inference");
        if preference {
            assert_eq!(attached[field], changed[field]);
        }
        let after = recovery(&fixture.recovery, &fixture.library)
            .unwrap()
            .unwrap();
        let before: Recovery = serde_json::from_slice(&before).unwrap();
        assert_eq!(after.snapshot_name, before.snapshot_name);
        assert_eq!(after.created_at_ms, before.created_at_ms);
        assert_eq!(after.previous_applied_id, before.previous_applied_id);
        assert_eq!(after.preserve_profile_on_restore, preference);
        assert!(
            !String::from_utf8_lossy(&fs::read(&fixture.recovery).unwrap())
                .contains("synthetic-foreign-key")
        );
        assert!(restore(&fixture.library, &fixture.recovery).unwrap());
        let meta = metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap();
        assert_eq!(meta["appliedId"], "other");
        assert_eq!(fixture.profile().exists(), preference);
        if preference {
            assert_eq!(
                fs::read(fixture.profile()).unwrap(),
                serialize(&attached).unwrap()
            );
        }
    }
    let fixture = Fixture::new();
    create_snapshot(&fixture.library, &fixture.recovery, "Pending connection").unwrap();
    fs::write(fixture.profile(), serialize(&desired).unwrap()).unwrap();
    fs::write(
        fixture.library.join("_meta.json"),
        serialize(&json!({
            "entries":[{"id":PROFILE_ID,"name":"Zenith Relay"}],"appliedId":PROFILE_ID
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(check_attach(&fixture.library, &fixture.recovery, &desired).is_ok());
    assert!(!apply(&fixture.library, &fixture.recovery, &desired, true).unwrap());
}

#[test]
fn restoring_external_profile_bytes_preserves_them_even_when_their_json_is_invalid() {
    for bytes in [br"{}".as_slice(), br"{unfinished".as_slice()] {
        let fixture = Fixture::new();
        fixture.write_meta(Some("other"));
        apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/model"),
            true,
        )
        .unwrap();
        fs::write(fixture.profile(), bytes).unwrap();
        assert!(check_restore(&fixture.library, &fixture.recovery).unwrap());
        assert!(restore(&fixture.library, &fixture.recovery).unwrap());
        assert_eq!(fs::read(fixture.profile()).unwrap(), bytes);
        let meta = metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap();
        assert_eq!(meta["appliedId"], "other");
        assert!(has_entry(&meta, PROFILE_ID));
        assert!(!fixture.recovery.exists());
    }
}

#[test]
fn unapplied_snapshot_does_not_restrict_later_external_configuration() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    create_snapshot(
        &fixture.library,
        &fixture.recovery,
        "Before external changes",
    )
    .unwrap();
    let metadata_bytes = b"{user-is-editing";
    let profile_bytes = b"{external-profile";
    fs::write(fixture.library.join("_meta.json"), metadata_bytes).unwrap();
    fs::write(fixture.profile(), profile_bytes).unwrap();
    assert!(!check_restore(&fixture.library, &fixture.recovery).unwrap());
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    assert_eq!(
        fs::read(fixture.library.join("_meta.json")).unwrap(),
        metadata_bytes
    );
    assert_eq!(fs::read(fixture.profile()).unwrap(), profile_bytes);
    assert!(!fixture.recovery.exists());
}

#[test]
fn preserved_preferences_survive_refresh_restore_and_a_later_reconnect() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    apply(
        &fixture.library,
        &fixture.recovery,
        &config("pool/old"),
        true,
    )
    .unwrap();
    let mut external = config("pool/old");
    external["custom"] = json!({"theme":"synthetic-dark"});
    fs::write(fixture.profile(), serialize(&external).unwrap()).unwrap();
    for (model, select) in [("pool/current", true), ("pool/refreshed", false)] {
        apply(&fixture.library, &fixture.recovery, &config(model), select).unwrap();
        let profile = parse(&fs::read(fixture.profile()).unwrap()).unwrap();
        assert_eq!(profile["custom"], external["custom"]);
    }
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    assert!(fixture.profile().exists());
    check_attach(&fixture.library, &fixture.recovery, &config("pool/later")).unwrap();
    assert!(
        apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/later"),
            true
        )
        .unwrap()
    );
    assert_eq!(
        parse(&fs::read(fixture.profile()).unwrap()).unwrap()["custom"],
        external["custom"]
    );
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
}

#[test]
fn renamed_profile_reconnects_without_duplicate_entries_or_lost_settings() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    let desired = config("pool/model");
    apply(&fixture.library, &fixture.recovery, &desired, true).unwrap();
    let meta_path = fixture.library.join("_meta.json");
    let mut meta = metadata(Some(&fs::read(&meta_path).unwrap())).unwrap();
    meta["entries"][1]["name"] = json!("My desktop gateway");
    fs::write(&meta_path, serialize(&meta).unwrap()).unwrap();
    check_attach(&fixture.library, &fixture.recovery, &desired).unwrap();
    assert!(!apply(&fixture.library, &fixture.recovery, &desired, true).unwrap());
    assert_eq!(
        metadata(Some(&fs::read(&meta_path).unwrap())).unwrap(),
        meta
    );
    assert!(
        inspect(&fixture.library, &fixture.recovery)
            .unwrap()
            .configured
    );
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    let restored = metadata(Some(&fs::read(&meta_path).unwrap())).unwrap();
    assert_eq!(restored["entries"], meta["entries"]);
    assert_eq!(restored["appliedId"], "other");
    assert!(fixture.profile().exists());
    check_attach(&fixture.library, &fixture.recovery, &desired).unwrap();
    assert!(apply(&fixture.library, &fixture.recovery, &desired, true).unwrap());
}

#[test]
fn recovery_without_the_preservation_flag_remains_restorable() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    let desired = config("pool/model");
    apply(&fixture.library, &fixture.recovery, &desired, true).unwrap();
    let mut record = parse(&fs::read(&fixture.recovery).unwrap()).unwrap();
    record
        .as_object_mut()
        .unwrap()
        .remove("preserveProfileOnRestore");
    fs::write(&fixture.recovery, serialize(&record).unwrap()).unwrap();
    assert!(check_restore(&fixture.library, &fixture.recovery).unwrap());
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    assert!(!fixture.profile().exists());
    assert!(!fixture.recovery.exists());
    let meta = metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap();
    assert_eq!(meta["appliedId"], "other");
}

#[test]
fn external_selection_before_reconnect_does_not_replace_a_named_recovery_point() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    create_snapshot(&fixture.library, &fixture.recovery, "Original selection").unwrap();
    let meta_path = fixture.library.join("_meta.json");
    let mut meta = metadata(Some(&fs::read(&meta_path).unwrap())).unwrap();
    meta["entries"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"newer","name":"Newer manual profile"}));
    meta["appliedId"] = json!("newer");
    fs::write(&meta_path, serialize(&meta).unwrap()).unwrap();
    let desired = config("pool/model");
    apply(&fixture.library, &fixture.recovery, &desired, true).unwrap();
    let point = recovery(&fixture.recovery, &fixture.library)
        .unwrap()
        .unwrap();
    assert_eq!(point.previous_applied_id.as_deref(), Some("other"));
    assert_eq!(point.snapshot_name.as_deref(), Some("Original selection"));
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    let restored = metadata(Some(&fs::read(&meta_path).unwrap())).unwrap();
    assert_eq!(restored["appliedId"], "other");
    assert_eq!(restored["entries"], meta["entries"]);
}
