use super::*;

fn identity_config() -> Value {
    let mut config = config("unused");
    config["inferenceModels"] = json!([{
        "name": zenith_relay_core::claude_desktop_catalog_model_alias("gateway-model"),
        "behavesAs": "claude-opus-4-6",
        "maxEffort": "max"
    }]);
    config
}

#[test]
fn unknown_client_version_keeps_existing_identity_profile_and_recovery() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    apply(
        &fixture.library,
        &fixture.recovery,
        &identity_config(),
        true,
    )
    .unwrap();
    let before = fs::read(fixture.profile()).unwrap();
    let recovery = fs::read(&fixture.recovery).unwrap();
    assert_eq!(
        model_identity_support(&fixture.library, None)
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(fs::read(fixture.profile()).unwrap(), before);
    assert_eq!(fs::read(&fixture.recovery).unwrap(), recovery);
    assert!(model_identity_support(&fixture.library, Some(true)).unwrap());
    assert!(!model_identity_support(&fixture.library, Some(false)).unwrap());
}

#[test]
fn unknown_client_version_uses_baseline_for_profiles_without_identity() {
    let fixture = Fixture::new();
    assert!(!model_identity_support(&fixture.library, None).unwrap());
    fixture.write_meta(None);
    apply(
        &fixture.library,
        &fixture.recovery,
        &config("pool/model"),
        true,
    )
    .unwrap();
    assert!(!model_identity_support(&fixture.library, None).unwrap());
}

#[test]
fn retired_recovery_identity_is_rejected_without_overwriting_its_data() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    let config = identity_config();
    apply(&fixture.library, &fixture.recovery, &config, true).unwrap();
    let mut record = parse(&fs::read(&fixture.recovery).unwrap()).unwrap();
    record["profileId"] = json!("zenith-relay");
    let bytes = serialize(&record).unwrap();
    fs::write(&fixture.recovery, &bytes).unwrap();
    let profile = fs::read(fixture.profile()).unwrap();
    assert!(inspect(&fixture.library, &fixture.recovery).is_err());
    assert!(apply(&fixture.library, &fixture.recovery, &config, true).is_err());
    assert!(restore(&fixture.library, &fixture.recovery).is_err());
    assert_eq!(fs::read(&fixture.recovery).unwrap(), bytes);
    assert_eq!(fs::read(fixture.profile()).unwrap(), profile);
}

#[test]
fn authoritative_empty_refresh_clears_identity_models_and_remains_restorable() {
    let fixture = Fixture::new();
    fixture.write_meta(Some("other"));
    let mut config = identity_config();
    apply(&fixture.library, &fixture.recovery, &config, true).unwrap();
    config["inferenceModels"] = json!([]);
    apply(&fixture.library, &fixture.recovery, &config, false).unwrap();
    let status = inspect(&fixture.library, &fixture.recovery).unwrap();
    assert!(status.configured);
    assert_eq!(status.model_count, 0);
    assert!(!model_identity_support(&fixture.library, None).unwrap());
    assert_eq!(
        parse(&fs::read(fixture.profile()).unwrap()).unwrap(),
        config
    );
    restore(&fixture.library, &fixture.recovery).unwrap();
    assert_eq!(
        metadata(Some(&fs::read(fixture.library.join("_meta.json")).unwrap())).unwrap()["appliedId"],
        "other"
    );
}

#[test]
fn reclaim_requires_exact_current_catalog_identity_for_the_same_route() {
    for change in [
        "exact",
        "different-identity",
        "different-name",
        "missing",
        "empty",
        "null",
        "foreign",
    ] {
        let fixture = Fixture::new();
        fixture.write_meta(Some("other"));
        let mut config = identity_config();
        let expected = config.clone();
        match change {
            "different-identity" => {
                config["inferenceModels"][0]["behavesAs"] = json!("claude-sonnet-4-6")
            }
            "different-name" => config["inferenceModels"][0]["name"] = json!("unknown-model"),
            "missing" => {
                config["inferenceModels"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("behavesAs");
            }
            "empty" => config["inferenceModels"][0]["behavesAs"] = json!(""),
            "null" => config["inferenceModels"][0]["behavesAs"] = Value::Null,
            "foreign" => config["inferenceModels"][0]["supports1m"] = json!(true),
            _ => {}
        }
        apply(&fixture.library, &fixture.recovery, &config, true).unwrap();
        fs::remove_file(&fixture.recovery).unwrap();
        let before = fs::read(fixture.profile()).unwrap();
        let result = apply(&fixture.library, &fixture.recovery, &expected, true);
        if change == "exact" {
            assert!(result.unwrap());
            assert!(restore(&fixture.library, &fixture.recovery).unwrap());
        } else {
            assert_eq!(
                result.unwrap_err().code,
                ErrorCode::ProfileRestoreBlocked,
                "{change}"
            );
            assert_eq!(fs::read(fixture.profile()).unwrap(), before, "{change}");
            assert!(!fixture.recovery.exists(), "{change}");
        }
    }
}
