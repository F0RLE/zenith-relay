use super::*;
use crate::{
    accounts::{AccountAuthState, AccountHealthState},
    model_metadata::ModelMetadataCatalog,
    quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind, Subscription, SubscriptionStatus},
    CandidateHealth, CandidateQuota,
};
use crate::{
    ActiveModelRuntime, ApiEquivalentSummary, CandidateKind, CandidateRuntimeSnapshot,
    GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey, MessagesReasoningMode, PriceEvidence,
    ProviderSource, RuntimeLocalKey, RuntimeSource, SourceAdapter, SourcePricingMetadata,
};
use std::sync::Arc;
mod accounts;
mod inventory;
mod pricing;
mod reasoning;
mod saved_presets;
mod speed;

fn runtime_candidate(
    candidate_id: &str,
    kind: CandidateKind,
    available: bool,
) -> CandidateRuntimeSnapshot {
    CandidateRuntimeSnapshot {
        candidate_id: candidate_id.into(),
        kind,
        available,
        next_for_new_request: false,
        activity_revision: 0,
        runtime_id: 0,
        in_flight: 0,
        active_request_count: 0,
        active_models: Vec::<ActiveModelRuntime>::new(),
        model_retries: Vec::new(),
        last_used_at_ms: None,
        next_retry_at_ms: None,
        half_open: false,
        dispatches: 0,
    }
}

fn account_summary(in_pool: bool, models: &[&str]) -> AccountSummary {
    AccountSummary {
        id: "account".into(),
        label: "Account".into(),
        identity_hint: "account".into(),
        provider_family: None,
        basis_points_available: false,
        basis_points_enabled: false,
        enabled: true,
        in_pool,
        draining: false,
        operational_status: OperationalStatus::Rotation,
        auth_state: AccountAuthState::Active,
        health: "healthy".into(),
        models: models.iter().map(ToString::to_string).collect(),
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        api_equivalent: ApiEquivalentSummary::default(),
        quota_window_usage: None,
        purchase_cost_micro_usd: None,
        subscription: Subscription::default(),
        quota: QuotaSnapshot::default(),
        quota_refresh_status: QuotaRefreshStatus::default(),
        refresh_state: AccountRefreshState::default(),
        secret_available: true,
        remote_location: None,
        proxy_mode: ProxyMode::Direct,
        proxy_available: true,
        proxy_id: None,
        routing_block_reason: None,
        last_error_code: None,
        client_auth_status: None,
        last_client_login_redirect_at_ms: None,
    }
}

fn source_summary(id: &str, models: &[&str]) -> SourceSummary {
    SourceSummary {
        resolved_protocol_bindings: None,
        id: id.into(),
        name: id.into(),
        enabled: true,
        in_pool: true,
        draining: false,
        operational_status: OperationalStatus::Rotation,
        base_url: "https://example.test/v1".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_bindings: Vec::new(),
        protocol_config: crate::SourceProtocolConfig::default(),
        models: models.iter().map(ToString::to_string).collect(),
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::new(),
        api_equivalent: ApiEquivalentSummary::default(),
        secret_available: true,
        last_error_code: None,
        refresh_revision: None,
        refresh_state: SourceRefreshState::default(),
        provider_stats: None,
    }
}

fn test_token_price(input: u64, output: u64) -> TokenPrice {
    TokenPrice {
        input,
        cache_read: Some(input / 10),
        cache_write_5m: None,
        cache_write_1h: None,
        output,
        flex: crate::pricing::TokenRateSet::EMPTY,
        priority: crate::pricing::TokenRateSet::EMPTY,
        above_200k: crate::pricing::LongContextRates::EMPTY,
        above_272k: crate::pricing::LongContextRates::EMPTY,
    }
}

fn valid_configuration_preset() -> ConfigurationPreset {
    serde_json::from_value(serde_json::json!({
            "format": CONFIGURATION_PRESET_FORMAT,
            "schemaVersion": CONFIGURATION_PRESET_SCHEMA_VERSION,
            "settings": {
                "sources": [{
                    "id": "source_1", "name": "Source", "baseUrl": "https://example.test/v1",
                    "wireApi": "responses", "enabled": true, "inPool": true,
                    "allowedModels": [], "excludedModels": [], "priority": 0, "weight": 1
                }],
                "accounts": [],
                "routing": {
                    "maxRetryCandidates": 3, "cooldownAfterFailures": 3,
                    "keepLastCandidateAvailable": true, "routingStrategy": "adaptive",
                    "subscriptionPlanOrder": [], "defaultServiceTier": "standard", "imageBaseModel": null
                },
                "quota": { "requestTimeoutSeconds": 20, "accountProxyRequired": false, "commonProxyId": null },
                "hiddenModels": [], "modelPriceOverrides": {}, "modelReasoningAllowedLevels": {},
                "modelServiceTierOverrides": {}, "modelDisplayOrder": []
            }
        }))
        .expect("static configuration preset is valid")
}
