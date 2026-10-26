use super::*;

#[test]
fn explicit_snapshot_does_not_connect_or_change_claude_files() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    let meta_path = fixture.library.join("_meta.json");
    let before = fs::read(&meta_path).unwrap();
    assert!(create_snapshot(&fixture.library, &fixture.recovery, " Before Claude ").unwrap());
    assert!(!check_restore(&fixture.library, &fixture.recovery).unwrap());
    let status = inspect(&fixture.library, &fixture.recovery).unwrap();
    assert_eq!(status.snapshot_name.as_deref(), Some("Before Claude"));
    assert!(status.created_at_ms.unwrap() > 0);
    assert!(status.has_recovery);
    assert!(!status.configured);
    assert!(!fixture.profile().exists());
    assert_eq!(fs::read(&meta_path).unwrap(), before);
    let record = fs::read(&fixture.recovery).unwrap();
    assert!(!create_snapshot(&fixture.library, &fixture.recovery, "Do not replace").unwrap());
    assert_eq!(fs::read(&fixture.recovery).unwrap(), record);
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    assert_eq!(fs::read(&meta_path).unwrap(), before);
    assert!(!fixture.recovery.exists());
}

#[test]
fn attach_and_refresh_preserve_named_snapshot_and_restore_the_prior_selection() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    create_snapshot(&fixture.library, &fixture.recovery, "Before connection").unwrap();
    let created = inspect(&fixture.library, &fixture.recovery)
        .unwrap()
        .created_at_ms;
    for select in [true, false] {
        apply(
            &fixture.library,
            &fixture.recovery,
            &config("pool/model"),
            select,
        )
        .unwrap();
        let status = inspect(&fixture.library, &fixture.recovery).unwrap();
        assert!(status.configured);
        assert_eq!(status.snapshot_name.as_deref(), Some("Before connection"));
        assert_eq!(status.created_at_ms, created);
    }
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
    let meta = metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap();
    assert_eq!(meta["appliedId"], "other");
    assert!(!fixture.profile().exists());
}

#[test]
fn invalid_snapshot_names_and_unowned_profiles_cannot_create_a_snapshot() {
    let fixture = Fixture::new();
    for name in ["".to_owned(), "bad\nname".to_owned(), "x".repeat(81)] {
        assert!(create_snapshot(&fixture.library, &fixture.recovery, &name).is_err());
        assert!(!fixture.recovery.exists());
    }
    fs::write(fixture.profile(), b"foreign profile").unwrap();
    assert!(create_snapshot(&fixture.library, &fixture.recovery, "Before Claude").is_err());
    assert!(!fixture.recovery.exists());
    assert_eq!(fs::read(fixture.profile()).unwrap(), b"foreign profile");
}

#[test]
fn automatic_recovery_is_not_replaced_by_an_explicit_snapshot() {
    let fixture = Fixture::new();
    apply(
        &fixture.library,
        &fixture.recovery,
        &config("pool/model"),
        true,
    )
    .unwrap();
    let before = fs::read(&fixture.recovery).unwrap();
    assert!(!create_snapshot(&fixture.library, &fixture.recovery, "Later").unwrap());
    assert_eq!(fs::read(&fixture.recovery).unwrap(), before);
}
