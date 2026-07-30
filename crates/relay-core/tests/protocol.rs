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

    current.features.remove(Feature::ClientKeyBudgets.as_str());
    current
        .features
        .remove(Feature::ProfileKeyRotation.as_str());
    assert!(negotiate(ClientProtocolRange::default(), &current).is_ok());
    assert!(!current.supports(Feature::ClientKeyBudgets));
    assert!(current.supports(Feature::ClientAccess));
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
