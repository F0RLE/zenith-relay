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
        assert!(check_restore(&fixture.library, &fixture.recovery).is_err());
        assert!(restore(&fixture.library, &fixture.recovery).is_err());
        assert!(apply(&fixture.library, &fixture.recovery, &desired, false).is_err());
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
fn stale_recovery_never_authorizes_foreign_settings_or_an_unapplied_snapshot() {
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
        assert!(
            check_attach(&fixture.library, &fixture.recovery, &desired).is_err(),
            "{field}"
        );
        assert!(
            apply(&fixture.library, &fixture.recovery, &desired, true).is_err(),
            "{field}"
        );
        assert_eq!(fs::read(fixture.profile()).unwrap(), bytes);
        assert_eq!(fs::read(&fixture.recovery).unwrap(), before);
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
    assert!(check_attach(&fixture.library, &fixture.recovery, &desired).is_err());
    assert!(apply(&fixture.library, &fixture.recovery, &desired, true).is_err());
}
