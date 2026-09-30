use super::*;
use crate::local_pool::models::{LocalGatewayKeyRecord, ProviderSourceRecord};
use std::cell::RefCell;
use std::collections::BTreeMap;
use zenith_relay_core::{MessagesReasoningMode, SourceAdapter, SourceProtocolBinding, WireApi};

fn source_record(id: &str) -> ProviderSourceRecord {
    ProviderSourceRecord {
        id: id.into(),
        name: "Provider".into(),
        enabled: true,
        in_pool: false,
        draining: false,
        base_url: "https://provider.test/v1".into(),
        secret_ref: "source:test".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_config: Default::default(),
        protocol_bindings: Vec::new(),
        models: vec!["provider-model".into()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::new(),
        last_used_at: None,
        last_test_at: None,
        last_test_status: None,
        last_error: None,
    }
}

#[test]
fn active_catalog_refresh_targets_a_direct_source_unless_a_system_gateway_key_owns_it() {
    let binding = codex::ProfileBinding {
        profile_dir: "profile".into(),
        credential_kind: codex::ProfileCredentialKind::LocalGateway,
        credential_id: "source".into(),
        bound_oauth_account_id: None,
        active: true,
    };
    let source = source_record("source");

    assert!(matches!(
        catalog::active_catalog_refresh_target(&binding, &[], std::slice::from_ref(&source)),
        Some(catalog::CodexCatalogRefreshTarget::DirectSource(candidate))
            if candidate.id == source.id
    ));

    let system_key = LocalGatewayKeyRecord {
        id: "source".into(),
        label: "Local gateway".into(),
        enabled: true,
        system: true,
        secret_ref: "key:source".into(),
        created_at: "2026-08-01T00:00:00Z".into(),
        last_used_at: None,
    };
    assert!(matches!(
        catalog::active_catalog_refresh_target(&binding, &[system_key], &[source]),
        Some(catalog::CodexCatalogRefreshTarget::LocalGateway(_))
    ));
}

#[test]
fn active_catalog_refresh_ignores_sources_without_an_enabled_native_responses_binding() {
    let binding = codex::ProfileBinding {
        profile_dir: "profile".into(),
        credential_kind: codex::ProfileCredentialKind::LocalGateway,
        credential_id: "source".into(),
        bound_oauth_account_id: None,
        active: true,
    };
    let mut source = source_record("source");

    source.enabled = false;
    assert!(catalog::active_catalog_refresh_target(&binding, &[], &[source.clone()]).is_none());

    source.enabled = true;
    source.protocol_bindings = vec![SourceProtocolBinding {
        wire_api: WireApi::Responses,
        adapter: SourceAdapter::ResponsesToMessages,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: vec!["bridge-only".to_string()],
    }];
    source.models = vec!["bridge-only".to_string()];
    assert!(catalog::active_catalog_refresh_target(&binding, &[], &[source]).is_none());
}

#[test]
fn lost_profile_rotation_commit_response_is_reconciled_without_a_blind_rollback() {
    let system_credential_id = "key_system";
    let current = RemoteProfileCredential {
        key_id: system_credential_id.into(),
        base_url: "https://relay.example/v1".into(),
        secret: "zrs_current_secret_value_000000".into(),
    };
    let rotation = ProfileKeyRotation {
        schema_version: 1,
        rotation_id: "key_profile_rotation_test".into(),
        key_id: system_credential_id.into(),
        base_url: current.base_url.clone(),
        secret: "zrs_rotated_secret_value_000000".into(),
    };
    let committed = RemoteProfileCredential {
        key_id: rotation.key_id.clone(),
        base_url: rotation.base_url.clone(),
        secret: rotation.secret.clone(),
    };
    let unrelated = RemoteProfileCredential {
        secret: "zrs_unrelated_secret_value_0000".into(),
        ..current.clone()
    };

    assert_eq!(
        profile_rotation_commit_state(Some(&committed), &current, &rotation),
        ProfileRotationCommitState::Committed
    );
    assert_eq!(
        profile_rotation_commit_state(Some(&current), &current, &rotation),
        ProfileRotationCommitState::NotCommitted
    );
    assert_eq!(
        profile_rotation_commit_state(Some(&unrelated), &current, &rotation),
        ProfileRotationCommitState::Unknown
    );
    assert_eq!(
        profile_rotation_commit_state(None, &current, &rotation),
        ProfileRotationCommitState::Unknown
    );
}

#[test]
fn direct_source_launch_requires_an_enabled_responses_binding() {
    let source = source_record("source");
    assert_eq!(
        validate_direct_source(&source).unwrap(),
        vec!["provider-model".to_string()]
    );

    let mut disabled = source.clone();
    disabled.enabled = false;
    assert!(validate_direct_source(&disabled).is_err());

    let mut messages_only = source.clone();
    messages_only.wire_api = WireApi::Messages;
    messages_only.protocol_bindings = vec![SourceProtocolBinding {
        wire_api: WireApi::Messages,
        adapter: SourceAdapter::Native,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: vec!["claude-native".to_string()],
    }];
    assert!(validate_direct_source(&messages_only).is_err());

    let mut bridged_only = source;
    bridged_only.protocol_bindings = vec![SourceProtocolBinding {
        wire_api: WireApi::Responses,
        adapter: SourceAdapter::ResponsesToMessages,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: vec!["claude-bridge".to_string()],
    }];
    bridged_only.models = vec!["claude-bridge".to_string()];
    assert!(validate_direct_source(&bridged_only).is_err());
}

#[test]
fn direct_source_launch_uses_only_models_bound_to_responses() {
    let mut source = source_record("source");
    source.wire_api = WireApi::Messages;
    source.models = vec![
        "claude-native".to_string(),
        "claude-responses".to_string(),
        "gpt-responses".to_string(),
    ];
    source.protocol_bindings = vec![
        SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["claude-native".to_string()],
        },
        SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["claude-responses".to_string(), "gpt-responses".to_string()],
        },
    ];

    assert_eq!(
        validate_direct_source(&source).unwrap(),
        vec!["claude-responses".to_string(), "gpt-responses".to_string()]
    );
}

#[test]
fn missing_zenith_source_secret_is_recovered_once() {
    let saved = RefCell::new(None);
    let api_key = load_direct_source_api_key(
        "https://api.zenithmarket.dev/v1/",
        "source:zenith",
        |_| Ok(None),
        || Some("znt_legacy_key".into()),
        |secret_ref, value| {
            saved.replace(Some((secret_ref.to_string(), value.to_string())));
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(api_key, "znt_legacy_key");
    assert_eq!(
        saved.into_inner(),
        Some(("source:zenith".into(), "znt_legacy_key".into()))
    );
}

#[test]
fn custom_source_does_not_reuse_the_legacy_zenith_key() {
    let error = load_direct_source_api_key(
        "https://api.example.com/v1",
        "source:custom",
        |_| Ok(None),
        || Some("znt_legacy_key".into()),
        |_, _| panic!("custom source secret must not be synthesized"),
    )
    .unwrap_err();

    assert_eq!(error.code, ErrorCode::NotFound);
}
