use super::*;

fn attached_fixture() -> Fixture {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    apply(
        &fixture.library,
        &fixture.recovery,
        &config("pool/original"),
        true,
    )
    .unwrap();
    fixture
}

fn compact_metadata_at_limit(fixture: &Fixture) -> Vec<u8> {
    let path = fixture.library.join("_meta.json");
    let mut meta = metadata(Some(&fs::read(&path).unwrap())).unwrap();
    meta["foreign_values"] = json!(vec![0; 128]);
    meta["foreign_padding"] = json!("");
    let overhead = serde_json::to_vec(&meta).unwrap().len();
    meta["foreign_padding"] = json!("x".repeat(MAX_CONFIG_BYTES as usize - overhead));
    let bytes = serde_json::to_vec(&meta).unwrap();
    assert_eq!(bytes.len() as u64, MAX_CONFIG_BYTES);
    fs::write(path, &bytes).unwrap();
    bytes
}

#[test]
fn attach_rejects_metadata_expansion_before_creating_profile_or_recovery() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    let original_meta = compact_metadata_at_limit(&fixture);

    let error = apply(
        &fixture.library,
        &fixture.recovery,
        &config("pool/model"),
        true,
    )
    .unwrap_err();

    assert_eq!(error.code, ErrorCode::ProfileRestoreBlocked);
    assert!(fs::read(fixture.library.join("_meta.json")).unwrap() == original_meta);
    assert!(!fixture.profile().exists());
    assert!(!fixture.recovery.exists());
}

#[test]
fn refresh_rejects_metadata_expansion_without_changing_owned_files() {
    let fixture = attached_fixture();
    let original_meta = compact_metadata_at_limit(&fixture);
    let original_profile = fs::read(fixture.profile()).unwrap();
    let original_recovery = fs::read(&fixture.recovery).unwrap();

    let error = apply(
        &fixture.library,
        &fixture.recovery,
        &config("pool/new"),
        true,
    )
    .unwrap_err();

    assert_eq!(error.code, ErrorCode::ProfileRestoreBlocked);
    assert!(fs::read(fixture.library.join("_meta.json")).unwrap() == original_meta);
    assert_eq!(fs::read(fixture.profile()).unwrap(), original_profile);
    assert_eq!(fs::read(&fixture.recovery).unwrap(), original_recovery);
}

#[test]
fn restore_rejects_metadata_expansion_before_deleting_owned_files() {
    let fixture = attached_fixture();
    let original_meta = compact_metadata_at_limit(&fixture);
    let original_profile = fs::read(fixture.profile()).unwrap();
    let original_recovery = fs::read(&fixture.recovery).unwrap();

    let error = restore(&fixture.library, &fixture.recovery).unwrap_err();

    assert_eq!(error.code, ErrorCode::ProfileRestoreBlocked);
    assert!(fs::read(fixture.library.join("_meta.json")).unwrap() == original_meta);
    assert_eq!(fs::read(fixture.profile()).unwrap(), original_profile);
    assert_eq!(fs::read(&fixture.recovery).unwrap(), original_recovery);
}

#[test]
fn attach_and_restore_preflight_reject_metadata_expansion() {
    let fixture = attached_fixture();
    compact_metadata_at_limit(&fixture);

    let attach =
        check_attach(&fixture.library, &fixture.recovery, &config("pool/new")).unwrap_err();
    let restore = check_restore(&fixture.library, &fixture.recovery).unwrap_err();

    assert_eq!(attach.code, ErrorCode::ProfileRestoreBlocked);
    assert_eq!(restore.code, ErrorCode::ProfileRestoreBlocked);
}

#[test]
fn unchanged_profile_does_not_require_rewriting_large_foreign_metadata() {
    let fixture = attached_fixture();
    let original_meta = compact_metadata_at_limit(&fixture);
    let original_recovery = fs::read(&fixture.recovery).unwrap();

    check_attach(
        &fixture.library,
        &fixture.recovery,
        &config("pool/original"),
    )
    .unwrap();
    assert!(
        !apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/original"),
            true,
        )
        .unwrap()
    );
    assert!(fs::read(fixture.library.join("_meta.json")).unwrap() == original_meta);
    assert_eq!(fs::read(&fixture.recovery).unwrap(), original_recovery);
}

#[test]
fn bounded_reads_keep_optional_files_and_accept_the_exact_limit() {
    let fixture = Fixture::new();
    let path = fixture.root.join("read-limit.json");
    assert!(read(&path).unwrap().is_none());

    let mut bytes = vec![b' '; MAX_CONFIG_BYTES as usize];
    fs::write(&path, &bytes).unwrap();
    assert!(read(&path).unwrap().unwrap() == bytes);

    bytes.push(b' ');
    fs::write(&path, bytes).unwrap();
    assert_eq!(
        read(&path).unwrap_err().code,
        ErrorCode::ProfileRestoreBlocked
    );
    assert_eq!(
        read(&fixture.library).unwrap_err().code,
        ErrorCode::ProfileRestoreBlocked
    );
}
