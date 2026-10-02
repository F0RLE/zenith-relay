use crate::config::Config;
use crate::state::{AppState, ServerAccountRecord, SourceRecord};
use crate::store::{Store, Vault};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use zenith_relay_core::accounts::{AccountAuthState, AccountHealthState};
use zenith_relay_core::automations::{
    AccountSelector, WakeExecutionPolicy, WakeModelPolicy, WakeTask, WakeTrigger,
};
use zenith_relay_core::quota::QuotaWindowKind;
use zenith_relay_core::WireApi;

pub(crate) fn synthetic_server_account(id: &str) -> ServerAccountRecord {
    ServerAccountRecord {
        id: id.to_string(),
        label: id.to_string(),
        identity_hint: id.to_string(),
        enabled: true,
        in_pool: true,
        draining: false,
        source_id: "openai_codex".to_string(),
        secret_ref: format!("account:{id}"),
        provider_family: Some("openai".to_string()),
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        models: vec!["gpt-test".to_string()],
        discovered_models: None,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        subscription: Default::default(),
        quota: Default::default(),
        purchase_cost_micro_usd: None,
        cooldowns: Default::default(),
        consecutive_failures: 0,
        created_at_ms: 1,
        last_used_at_ms: None,
        last_error_code: None,
        proxy_id: None,
        bypass_common_proxy: false,
    }
}

pub(crate) fn wake_task() -> WakeTask {
    WakeTask {
        id: "wake_test".into(),
        name: "Test".into(),
        enabled: true,
        account_selector: AccountSelector::AllEligible,
        window_kinds: BTreeSet::from([QuotaWindowKind::Primary]),
        model_policy: WakeModelPolicy::LightestSupported,
        trigger: WakeTrigger::QuotaFull,
        fallback_schedule: None,
        execution_policy: WakeExecutionPolicy::Automatic,
        jitter_seconds: 0,
        max_attempts_per_cycle: 1,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

pub(crate) fn test_app_state(root: &Path) -> Arc<AppState> {
    test_app_state_bound(root, "127.0.0.1:0".parse().unwrap())
}

pub(crate) fn test_app_state_bound(root: &Path, bind: SocketAddr) -> Arc<AppState> {
    let config = Config::for_test(root.to_path_buf(), bind);
    let store = Arc::new(Store::open(root.join("relay.sqlite")).unwrap());
    let vault = Arc::new(Vault::open(&root.join("vault"), config.vault_key).unwrap());
    AppState::new(config, store, vault).unwrap()
}

pub(crate) fn pooled_source(id: &str, model: &str) -> SourceRecord {
    SourceRecord {
        id: id.into(),
        name: "Synthetic source".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        base_url: "https://example.test/v1".into(),
        secret_ref: format!("source:{id}"),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_config: Default::default(),
        protocol_bindings: Vec::new(),
        models: vec![model.into()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::new(),
        last_error_code: None,
    }
}
