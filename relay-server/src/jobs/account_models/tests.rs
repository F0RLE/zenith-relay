use super::*;
use std::collections::BTreeMap;
use zenith_relay_core::accounts::AccountHealthState;
use zenith_relay_core::quota::QuotaSnapshot;

fn account(models: &[&str]) -> ServerAccountRecord {
    ServerAccountRecord {
        id: "account-test".into(),
        label: "Account".into(),
        identity_hint: "a***@example.test".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        source_id: "codex".into(),
        secret_ref: "account:account-test".into(),
        provider_family: Some("openai".into()),
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        models: models.iter().map(|model| (*model).to_string()).collect(),
        discovered_models: None,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        subscription: Default::default(),
        quota: QuotaSnapshot::default(),
        purchase_cost_micro_usd: None,
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
        created_at_ms: 1,
        last_used_at_ms: None,
        last_error_code: None,
        proxy_id: None,
        bypass_common_proxy: false,
    }
}

#[test]
fn model_refresh_keeps_baseline_and_last_good_effective_list() {
    let mut record = account(&["gpt-old"]);
    apply_discovered_models(&mut record, Ok(vec!["gpt-future-codex".into()]));
    assert_eq!(record.models, ["gpt-old"]);
    assert!(record
        .discovered_models
        .as_ref()
        .is_some_and(|models| models.len() == 1 && models[0] == "gpt-future-codex"));
    assert_eq!(record.effective_models(), ["gpt-future-codex"]);

    apply_discovered_models(&mut record, Err(("models_transport".into(), true, None)));
    assert_eq!(record.models, ["gpt-old"]);
    assert_eq!(record.effective_models(), ["gpt-future-codex"]);
    assert_eq!(record.last_error_code.as_deref(), Some("models_transport"));

    let mut empty = account(&[]);
    apply_discovered_models(&mut empty, Err(("models_transport".into(), true, None)));
    assert_eq!(empty.health, AccountHealthState::Degraded);
    assert_eq!(empty.last_error_code.as_deref(), Some("models_transport"));

    apply_discovered_models(&mut empty, Ok(vec!["gpt-recovered".into()]));
    assert_eq!(empty.models, ["gpt-recovered"]);
    assert!(empty
        .discovered_models
        .as_ref()
        .is_some_and(|models| models.len() == 1 && models[0] == "gpt-recovered"));
    assert_eq!(empty.health, AccountHealthState::Healthy);
    assert!(empty.last_error_code.is_none());
}

#[test]
fn successful_empty_model_refresh_is_authoritative() {
    let mut record = account(&["gpt-old"]);

    apply_discovered_models(&mut record, Ok(Vec::new()));

    assert_eq!(record.discovered_models, Some(Vec::new()));
    assert!(record.effective_models().is_empty());
    assert!(record.last_error_code.is_none());
    assert_eq!(record.health, AccountHealthState::Healthy);
}

#[test]
fn successful_model_refresh_recovers_a_transient_auth_error() {
    let mut record = account(&["gpt-live"]);
    record.auth_state = AccountAuthState::Error;
    record.health = AccountHealthState::Unhealthy;
    record.last_error_code = Some("models_unauthorized".into());

    apply_discovered_models(&mut record, Ok(vec!["gpt-recovered".into()]));

    assert_eq!(record.auth_state, AccountAuthState::Active);
    assert_eq!(record.health, AccountHealthState::Healthy);
    assert_eq!(record.last_error_code, None);
}

#[test]
fn model_unauthorized_removes_a_cached_server_account_from_routing() {
    let mut record = account(&["gpt-live"]);
    let failure = Err(("models_unauthorized".to_string(), false, None));
    assert!(model_discovery_was_unauthorized(&failure));

    apply_discovered_models(&mut record, failure);

    assert_eq!(record.models, ["gpt-live"]);
    assert_eq!(record.auth_state, AccountAuthState::Error);
    assert_eq!(record.health, AccountHealthState::Unhealthy);
    assert_eq!(
        record.last_error_code.as_deref(),
        Some("models_unauthorized")
    );
}

#[test]
fn model_unauthorized_does_not_downgrade_server_reauthentication() {
    let mut record = account(&["gpt-live"]);
    record.auth_state =
        AccountAuthState::RequiresReauth(zenith_relay_core::accounts::ReauthReason::InvalidGrant);
    apply_discovered_models(
        &mut record,
        Err(("models_unauthorized".to_string(), false, None)),
    );

    assert!(matches!(
        record.auth_state,
        AccountAuthState::RequiresReauth(_)
    ));
    assert_eq!(record.health, AccountHealthState::Unhealthy);
}

#[test]
fn shared_model_discovery_failures_keep_server_error_categories() {
    let agent_task = model_discovery_error(ModelDiscoveryFailure {
        code: ModelDiscoveryFailureCode::AgentTaskInvalid,
        retryable: false,
        retry_after_ms: None,
        http_status: Some(401),
    });
    assert_eq!(agent_task, ("models_unauthorized".to_string(), false, None));

    let rate_limit = model_discovery_error(ModelDiscoveryFailure {
        code: ModelDiscoveryFailureCode::RateLimited,
        retryable: true,
        retry_after_ms: None,
        http_status: Some(429),
    });
    assert_eq!(rate_limit, ("models_rate_limited".to_string(), true, None));
}
