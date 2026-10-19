use zenith_relay_core::protocol::{
    negotiate, Capabilities, ClientProtocolRange, Feature, ProtocolError, RevealedAccountIdentity,
    CURRENT_PROTOCOL_VERSION,
};

#[test]
fn protocol_negotiation_accepts_current_server_and_features_are_explicit() {
    let capabilities = Capabilities::personal_server("server-1", "fingerprint-1");
    let negotiated = negotiate(ClientProtocolRange::default(), &capabilities).unwrap();

    assert_eq!(negotiated.version, CURRENT_PROTOCOL_VERSION);
    assert!(capabilities.supports(Feature::Accounts));
    assert!(capabilities.supports(Feature::AccountImportToPool));
    assert!(capabilities.supports(Feature::AccountIdentityReveal));
    assert!(capabilities.supports(Feature::Sources));
    assert!(capabilities.supports(Feature::ModelPricing));
    assert!(capabilities.supports(Feature::ProfileAttach));
    // New saved strategy variants require explicit capability negotiation.
    assert!(capabilities.supports(Feature::Rotation));
    assert!(capabilities.features.contains("rotation_v3"));
    assert!(!capabilities.features.contains("rotation_v2"));
}

#[test]
fn old_clients_cannot_read_new_strategies_but_new_clients_can_use_old_servers() {
    let current = Capabilities::personal_server("server", "fingerprint");
    assert!(current.supports_rotation_policy(3));
    assert!(!current.supports_rotation_policy(2));
    assert!(!current.supports_rotation_policy(255));
    assert!(negotiate(ClientProtocolRange { min: 2, max: 2 }, &current).is_err());
    let mut old_server = current;
    old_server.protocol_version = 2;
    old_server.compatibility_min_client = 2;
    old_server.features.remove("rotation_v3");
    old_server.features.insert("rotation_v2".into());
    assert!(old_server.supports_rotation_policy(2));
    assert!(!old_server.supports_rotation_policy(3));
    assert_eq!(
        negotiate(ClientProtocolRange::default(), &old_server)
            .unwrap()
            .version,
        2
    );
}

#[test]
fn protocol_overlap_and_missing_optional_capabilities_remain_compatible() {
    let mut current = Capabilities::personal_server("server-1", "fingerprint-1");
    current.compatibility_min_client = CURRENT_PROTOCOL_VERSION - 1;
    let negotiated = negotiate(
        ClientProtocolRange {
            min: CURRENT_PROTOCOL_VERSION - 1,
            max: CURRENT_PROTOCOL_VERSION,
        },
        &current,
    )
    .unwrap();
    assert_eq!(negotiated.version, CURRENT_PROTOCOL_VERSION);

    current
        .features
        .remove(Feature::ProfileKeyRotation.as_str());
    assert!(negotiate(ClientProtocolRange::default(), &current).is_ok());
    assert!(!current.supports(Feature::ProfileKeyRotation));
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

#[test]
fn protocol_negotiation_rejects_non_overlapping_versions_and_empty_identity() {
    let capabilities = Capabilities::personal_server("server-1", "fingerprint-1");
    assert!(matches!(
        negotiate(
            ClientProtocolRange {
                min: CURRENT_PROTOCOL_VERSION + 1,
                max: CURRENT_PROTOCOL_VERSION + 2,
            },
            &capabilities,
        ),
        Err(ProtocolError::Incompatible { .. })
    ));

    let invalid = Capabilities::personal_server("", "fingerprint-1");
    assert_eq!(
        negotiate(ClientProtocolRange::default(), &invalid),
        Err(ProtocolError::InvalidServerIdentity)
    );
}
