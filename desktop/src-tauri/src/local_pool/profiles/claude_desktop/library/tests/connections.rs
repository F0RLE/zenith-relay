use super::*;

fn set_connection(fixture: &Fixture, endpoint: Option<&str>) {
    let before = fs::read(fixture.profile()).unwrap();
    let mut value = parse(&before).unwrap();
    let fields = value.as_object_mut().unwrap();
    fields.remove("egressProxyUrl");
    if let Some(endpoint) = endpoint {
        fields.insert("egressProxyUrl".into(), json!(endpoint));
    }
    let after = serialize(&value).unwrap();
    record_native_connection_update(
        &fixture.library,
        &fixture.recovery,
        PROFILE_ID,
        &before,
        &after,
    )
    .unwrap();
    fs::write(fixture.profile(), after).unwrap();
}

#[test]
fn native_connection_preserves_pool_ownership_catalog_refresh_and_restore() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    apply(&fixture.library, &fixture.recovery, &config("old"), true).unwrap();
    set_connection(&fixture, Some("http://proxy.example.test:8080/"));
    assert!(
        inspect(&fixture.library, &fixture.recovery)
            .unwrap()
            .configured
    );

    apply(&fixture.library, &fixture.recovery, &config("new"), false).unwrap();
    let value = parse(&fs::read(fixture.profile()).unwrap()).unwrap();
    assert_eq!(value["inferenceModels"][0]["name"], "new");
    assert_eq!(value["egressProxyUrl"], "http://proxy.example.test:8080/");
    assert!(
        inspect(&fixture.library, &fixture.recovery)
            .unwrap()
            .configured
    );
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    assert_eq!(parse(&fs::read(fixture.profile()).unwrap()).unwrap(), value);
    let meta = parse(&fs::read(fixture.library.join("_meta.json")).unwrap()).unwrap();
    assert_eq!(meta["appliedId"], "other");
}

#[test]
fn resetting_native_connection_keeps_normal_owned_profile_cleanup() {
    let fixture = Fixture::new();
    apply(&fixture.library, &fixture.recovery, &config("model"), true).unwrap();
    set_connection(&fixture, Some("http://proxy.example.test:8080/"));
    apply(&fixture.library, &fixture.recovery, &config("new"), false).unwrap();
    set_connection(&fixture, None);
    assert!(
        inspect(&fixture.library, &fixture.recovery)
            .unwrap()
            .configured
    );
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    assert!(!fixture.profile().exists());
}

#[test]
fn connection_journal_preserves_ownership_before_native_write_finishes() {
    let fixture = Fixture::new();
    apply(&fixture.library, &fixture.recovery, &config("model"), true).unwrap();
    let before = fs::read(fixture.profile()).unwrap();
    let mut value = parse(&before).unwrap();
    value["egressProxyUrl"] = json!("http://proxy.example.test:8080/");
    record_native_connection_update(
        &fixture.library,
        &fixture.recovery,
        PROFILE_ID,
        &before,
        &serialize(&value).unwrap(),
    )
    .unwrap();
    assert!(
        inspect(&fixture.library, &fixture.recovery)
            .unwrap()
            .configured
    );
    assert_eq!(fs::read(fixture.profile()).unwrap(), before);
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    assert!(!fixture.profile().exists());
}

#[test]
fn connection_update_cannot_adopt_other_fields_or_external_profile_edits() {
    let fixture = Fixture::new();
    apply(&fixture.library, &fixture.recovery, &config("model"), true).unwrap();
    let before = fs::read(fixture.profile()).unwrap();
    let original_record = fs::read(&fixture.recovery).unwrap();
    assert!(
        record_native_connection_update(
            &fixture.library,
            &fixture.recovery,
            PROFILE_ID,
            &before,
            &serialize(&config("foreign")).unwrap(),
        )
        .is_err()
    );
    assert_eq!(fs::read(&fixture.recovery).unwrap(), original_record);

    let mut value = config("foreign");
    let external = serialize(&value).unwrap();
    fs::write(fixture.profile(), &external).unwrap();
    value["egressProxyUrl"] = json!("http://proxy.example.test:8080/");
    let after = serialize(&value).unwrap();
    record_native_connection_update(
        &fixture.library,
        &fixture.recovery,
        PROFILE_ID,
        &external,
        &after,
    )
    .unwrap();
    fs::write(fixture.profile(), after).unwrap();
    assert_eq!(fs::read(&fixture.recovery).unwrap(), original_record);
    assert!(
        !inspect(&fixture.library, &fixture.recovery)
            .unwrap()
            .configured
    );
}
