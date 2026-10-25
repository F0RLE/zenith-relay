use zenith_relay_core::protocol::{DesktopCapabilities, RevealedAccountIdentity};

#[test]
fn desktop_capabilities_expose_local_features_without_server_negotiation() {
    let capabilities = DesktopCapabilities::current();
    for feature in [
        "accounts",
        "account_connectors_v1",
        "gemini_interactions_native_v1",
        "account_import_to_pool",
        "account_identity_reveal",
        "sources",
        "model_pricing",
        "profile_attach",
        "rotation_v3",
        "route_recovery_v1",
        "chatgpt_retry_until_available",
    ] {
        assert!(capabilities.features.contains(feature));
    }
    assert!(capabilities.features.contains("rotation_v3"));
    assert!(!capabilities.features.contains("rotation_v2"));
    assert!(!capabilities.features.contains("profile_key_rotation"));
    assert!(!capabilities.features.contains("backups"));
    let value = serde_json::to_value(capabilities).unwrap();
    assert_eq!(
        value.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["features", "accountConnectors"]
    );
    let connectors = value["accountConnectors"].as_array().unwrap();
    assert_eq!(connectors.len(), 3);
    assert_eq!(
        connectors
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["openai.codex", "openai.siwc", "openai.excel_bps"]
    );
    assert!(connectors
        .iter()
        .all(|item| item["providerFamily"] == "openai"));
    let legacy: DesktopCapabilities =
        serde_json::from_value(serde_json::json!({"features":["accounts"]})).unwrap();
    assert!(legacy.account_connectors.is_empty());
    assert!(legacy.features.contains("accounts"));
}

#[test]
fn revealed_account_identity_debug_output_is_redacted() {
    let identity = RevealedAccountIdentity {
        account_id: "account-1".into(),
        identity: "private@example.test".into(),
    };
    let debug = format!("{identity:?}");
    assert!(debug.contains("[redacted]"));
    assert!(!debug.contains("private@example.test"));
}
