use super::model_policy::configured_pool_model_ids;
use super::presets::apply_source_preset_policy;
use super::*;
use crate::local_pool::models::ProviderSourceRecord;
use std::collections::BTreeSet;
use uuid::Uuid;
use zenith_relay_core::protocol::{complete_model_display_order, SourcePresetRule};
use zenith_relay_core::{SourceProtocolBinding, WireApi};

fn source(id: &str, in_pool: bool, wire_api: WireApi) -> ProviderSourceRecord {
    ProviderSourceRecord {
        id: id.into(),
        name: id.into(),
        enabled: true,
        in_pool,
        draining: false,
        base_url: "https://example.test/v1".into(),
        secret_ref: format!("source:{id}"),
        pricing_provider: None,
        official_provider_family: None,
        wire_api,
        protocol_config: Default::default(),
        protocol_bindings: Vec::new(),
        models: vec!["test-model".into()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: Default::default(),
        detected_model_prices: Default::default(),
        last_used_at: None,
        last_test_at: None,
        last_test_status: None,
        last_error: None,
    }
}

async fn check_routing_modes_with_inventory(stored_policy: bool) {
    use zenith_relay_core::PoolRoutingMode;

    let root = std::env::temp_dir().join(format!("relay-routing-edit-{}", Uuid::new_v4()));
    let state = DesktopState::open(root.join("relay")).unwrap();
    let codex_home = root.join("codex");
    {
        let mut store = state.store().unwrap();
        let first = source("first", true, WireApi::Responses);
        let mut second = source("second", true, WireApi::Messages);
        second.enabled = false;
        if stored_policy {
            let mut gateway = store.gateway().clone();
            let mut persisted_policy = gateway.pool_routing_for(
                &[source("removed", true, WireApi::Responses), first.clone()],
                &[],
            );
            persisted_policy.members[0].weight = 7;
            persisted_policy.members[0].max_concurrency = 2;
            gateway.pool_routing = Some(persisted_policy);
            store.replace_gateway(gateway).unwrap();
        }
        for record in [first, second, source("outside", false, WireApi::Responses)] {
            store.upsert_source(record).unwrap();
        }
    }

    for mode in [
        PoolRoutingMode::InOrder,
        PoolRoutingMode::RoundRobin,
        PoolRoutingMode::Automatic,
    ] {
        let displayed = super::super::state::build_local_runtime_state(&state)
            .await
            .unwrap()
            .gateway
            .pool_routing
            .unwrap();
        assert_eq!(
            displayed
                .members
                .iter()
                .map(|member| member.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        let mut updated_policy = displayed.clone();
        updated_policy.mode = mode;
        let input = || {
            serde_json::from_value(serde_json::json!({
                "poolRouting": updated_policy,
                "expectedPoolRouting": displayed,
                "maxRetryCandidates": 3,
                "defaultServiceTier": "standard",
                "cooldownAfterFailures": 0,
                "keepLastCandidateAvailable": true,
                "routingStrategy": "quota_highest",
                "subscriptionPlanOrder": ["not a valid\nplan"]
            }))
            .unwrap()
        };
        let result = update_local_routing_at(input(), &state, &codex_home)
            .await
            .unwrap();
        assert_eq!(result.gateway.pool_routing, Some(updated_policy.clone()));
        let saved_gateway = state.store().unwrap().gateway().clone();
        assert_eq!(saved_gateway.max_retry_candidates, 3);
        let refreshed = super::super::state::build_local_runtime_state(&state)
            .await
            .unwrap();
        assert_eq!(
            refreshed.gateway.pool_routing.as_ref(),
            Some(&updated_policy)
        );
        // A real edit since the read must still conflict, without overwriting it.
        let error = update_local_routing_at(input(), &state, &codex_home)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Conflict);
    }
    let saved = state
        .store()
        .unwrap()
        .gateway()
        .pool_routing
        .clone()
        .unwrap();
    assert_eq!(saved.members[0].weight, if stored_policy { 7 } else { 1 });
    assert_eq!(
        saved.members[0].max_concurrency,
        if stored_policy { 2 } else { 0 }
    );
    drop(state);
    let reopened = DesktopState::open(root.join("relay")).unwrap();
    assert_eq!(
        reopened.store().unwrap().gateway().pool_routing.as_ref(),
        Some(&saved)
    );
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn routing_modes_save_from_an_unsaved_default_policy() {
    check_routing_modes_with_inventory(false).await;
}

#[tokio::test]
async fn routing_modes_save_after_members_join_or_leave() {
    check_routing_modes_with_inventory(true).await;
}

#[test]
fn source_preset_policy_preserves_source_identity() {
    let mut record = source("source", true, WireApi::Responses);
    record.name = "Existing connection".into();
    record.base_url = "https://existing.test/v1".into();
    let rule = SourcePresetRule {
        legacy_protocol_mode: None,
        id: "source".into(),
        name: "Imported name".into(),
        base_url: "https://imported.test/v1".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_bindings: Vec::new(),
        enabled: false,
        in_pool: true,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 8,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: Default::default(),
    };

    apply_source_preset_policy(&mut record, &rule);
    assert_eq!(record.name, "Existing connection");
    assert_eq!(record.base_url, "https://existing.test/v1");
    assert_eq!(record.wire_api, WireApi::Responses);
    assert!(!record.enabled);
    assert_eq!(record.priority, 8);
}

#[test]
fn local_pool_member_ids_include_native_messages_through_the_runtime_bridge() {
    let (source_ids, account_ids) = local_pool_member_ids(
        &[
            source("responses", true, WireApi::Responses),
            source("messages", true, WireApi::Messages),
            source("outside", false, WireApi::Responses),
        ],
        &[],
    )
    .unwrap();

    assert_eq!(
        source_ids,
        BTreeSet::from(["messages".to_string(), "responses".to_string()])
    );
    assert!(account_ids.is_empty());
}

#[test]
fn model_display_order_accepts_models_before_automatic_route_confirmation() {
    let mut source = source("automatic", true, WireApi::Responses);
    source.models = vec!["gpt-pending".into()];
    source.protocol_config = zenith_relay_core::SourceProtocolConfig::automatic(&source.base_url);

    let order = complete_model_display_order(
        configured_pool_model_ids(&[source], &[]),
        &["gpt-pending".into()],
        &[],
    )
    .unwrap();

    assert_eq!(order, ["gpt-pending"]);
}

#[test]
fn pool_model_inventory_includes_ids_only_present_on_source_bindings() {
    let mut binding_only = source("binding-only", true, WireApi::Responses);
    binding_only.models.clear();
    binding_only.protocol_bindings = vec![SourceProtocolBinding::legacy(
        WireApi::Responses,
        &["binding-model".to_string()],
    )];

    let sources = [binding_only];
    let current = configured_pool_model_ids(&sources, &[])
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(current, ["binding-model"]);
}

#[test]
fn model_edits_keep_unavailable_members_and_prefer_discovered_account_inventory() {
    use crate::local_pool::accounts::{credentials::StoredCodexCredentials, records};
    use zenith_relay_core::{accounts::AccountAuthMode, protocol::canonical_pool_model_id};

    let credentials = StoredCodexCredentials::new(
        "account_model_inventory",
        "synthetic-access".into(),
        Some("synthetic-refresh".into()),
        None,
        None,
        1,
        0,
        None,
        Some("synthetic-account".into()),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let mut account = records::new_account_record(
        &credentials,
        AccountAuthMode::OAuth,
        vec!["old-account-model".into()],
        0,
        1,
    )
    .unwrap();
    account.account.in_pool = true;
    account.account.enabled = false;
    account.discovered_models = Some(vec!["SHARED".into(), "account-model".into()]);
    account.excluded_models = vec!["account-model".into()];
    let mut outside_account = account.clone();
    outside_account.account.in_pool = false;
    outside_account.discovered_models = Some(vec!["outside-account-model".into()]);
    let accounts = [account, outside_account];

    let mut included = source("included", true, WireApi::Messages);
    included.enabled = false;
    included.draining = true;
    included.models = vec!["Shared".into()];
    included.excluded_models = vec!["Shared".into()];
    let sources = [included, source("outside", false, WireApi::Responses)];
    let inventory = || configured_pool_model_ids(&sources, &accounts);

    assert_eq!(canonical_pool_model_id(inventory(), "shared"), Ok("Shared"));
    assert_eq!(
        complete_model_display_order(inventory(), &["account-model".into()], &[]).unwrap(),
        ["account-model", "Shared"]
    );
    for absent in ["old-account-model", "outside-account-model", "test-model"] {
        assert!(canonical_pool_model_id(inventory(), absent).is_err());
    }
}
