use super::super::{
    accounts::{
        authority::{CredentialPersistence, StoredRefreshAdapter},
        credentials::{
            credential_invalid_state_error as account_credential_error, CredentialStore,
        },
        proxy::{effective_proxy_config, ProxyRefreshClient},
        records::CODEX_RESPONSES_URL,
        NativeSecretBackend,
    },
    error::{ErrorCode, ErrorDiagnostics, LocalPoolError, Result},
    models::{GatewaySettings, LocalAccountRecord, LocalGatewayKeyRecord, ProviderSourceRecord},
    profiles::codex,
    state::{DesktopState, LocalRuntimeInputs},
    store::secret_store,
};
use super::{pool, profiles};
use std::{collections::HashMap, sync::Arc};
use tauri::{AppHandle, Emitter, Manager};
use zenith_relay_core::{
    accounts::AccountRecord,
    changed_runtime_source_policy_updates,
    protocol::{
        account_candidate_enabled, account_operational_state, AccountOperationalInput,
        AccountOperationalState, ClientWireApi,
    },
    GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeCandidatePolicy,
    RuntimeChatGptAccount, RuntimeChatGptAuth, RuntimeMixedLocalKey, RuntimeSource,
    QUOTA_STALE_AFTER_MS,
};
#[cfg(test)]
use zenith_relay_core::{protocol::AccountRoutingBlockReason, WireApi};

pub(in crate::local_pool) use zenith_relay_core::unix_time_ms as current_time_ms;

pub(in crate::local_pool) fn record_catalog_refresh_result(
    state: &DesktopState,
    result: &std::result::Result<profiles::CodexCatalogRefreshStatus, LocalPoolError>,
) {
    match result {
        Ok(profiles::CodexCatalogRefreshStatus::Deferred) => {
            state.record_catalog_refresh_deferred()
        }
        Ok(_) => state.record_catalog_refresh_result(None),
        Err(error) => state.record_catalog_refresh_result(Some(error)),
    }
}

pub(in crate::local_pool) async fn runtime_from_store(
    state: &DesktopState,
) -> Result<Arc<GatewayRuntime>> {
    let system_key = pool::ensure_system_gateway_key(state)?;
    let codex_home = crate::platform::default_codex_home();
    let protected_account_id =
        managed_chatgpt_account_id_for_reserve(&codex_home, &state.profile_backup_root());
    let LocalRuntimeInputs {
        gateway: settings,
        sources: source_records,
        accounts: account_records,
        source_api_keys,
        account_credentials,
        ..
    } = state.runtime_inputs().await?;
    let quota_stale_after_ms = QUOTA_STALE_AFTER_MS;
    // The managed profile can expose every verified source protocol. Requests
    // still select only the protocol they actually use at the gateway edge.
    let (pool_source_ids, pool_account_ids) =
        pool::local_pool_member_ids(&source_records, &account_records)?;
    let mut sources = Vec::new();
    for source in source_records {
        let Some(api_key) = source_api_keys.get(&source.id).cloned().flatten() else {
            continue;
        };
        sources.push(RuntimeSource {
            source: ProviderSource {
                id: source.id,
                name: source.name,
                base_url: source.base_url,
                api_key,
                wire_api: source.wire_api,
                models: source.models,
            },
            protocol_bindings: source.protocol_bindings,
            enabled: source.enabled,
            draining: source.draining,
            priority: source.priority,
            weight: source.weight,
            recovery_delay_seconds: source.recovery_delay_seconds,
            allowed_models: source.allowed_models,
            excluded_models: source.excluded_models,
            last_used_at_ms: source.last_used_at.as_deref().and_then(timestamp_ms),
        });
    }
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let authority = state.token_authority();
    let mut accounts = Vec::new();
    let mut refresh_proxies = Vec::new();
    let mut agent_identities = HashMap::new();
    for account in account_records {
        let account_id = account.account.id.clone();
        let Some(secret) = account_credentials
            .get(&account_id)
            .and_then(Option::as_ref)
        else {
            continue;
        };
        let Some(chatgpt_account_id) = secret.provider_account_id() else {
            continue;
        };
        let Ok(proxy) = effective_proxy_config(&settings, secret) else {
            continue;
        };
        if let Some(agent) = secret.agent_identity() {
            agent_identities.insert(account_id.clone(), agent.clone());
        }
        if secret.has_oauth() {
            authority
                .register(
                    &account_id,
                    secret.to_token_set().map_err(account_credential_error)?,
                    account.account.auth_state,
                )
                .await
                .map_err(LocalPoolError::invalid_state)?;
        }
        let operational = runtime_account_operational_state(&account.account, current_time_ms());
        let models = account.effective_models().to_vec();
        // Candidate `enabled` represents base configuration availability.
        // Quota remains a separate scheduler decision for every request and
        // model-list response. Do not fold a temporary exhausted quota into
        // this flag: doing so makes a healthy pool look structurally invalid
        // until a later refresh happens to repair it.
        let candidate_enabled =
            account_candidate_enabled(account.account.enabled, operational.routing_block_reason);
        accounts.push(RuntimeChatGptAccount {
            id: account_id.clone(),
            source_id: account.account.source_id,
            chatgpt_account_id: chatgpt_account_id.to_string(),
            responses_url: CODEX_RESPONSES_URL.to_string(),
            models,
            enabled: candidate_enabled,
            draining: account.account.draining,
            priority: account.priority,
            weight: account.weight,
            allowed_models: account.allowed_models,
            excluded_models: account.excluded_models,
            health: operational.health,
            quota: operational.quota,
            quota_updated_at_ms: account.account.quota.updated_at_ms,
            quota_snapshot: account.account.quota.clone(),
            subscription_plan_type: account.account.subscription.plan_type.clone(),
            subscription_expires_at_ms: account.account.subscription.active_until_ms,
            last_used_at_ms: account.account.last_used_at_ms,
            cooldowns: Default::default(),
            consecutive_failures: 0,
            proxy: proxy.clone(),
        });
        if secret.has_oauth() {
            refresh_proxies.push((account_id, proxy));
        }
    }
    let secret = pool::ensure_local_gateway_key_secret(&system_key)?;
    let keys = vec![RuntimeMixedLocalKey {
        key: LocalGatewayKey {
            id: system_key.id,
            secret,
        },
        enabled: system_key.enabled,
        source_ids: Some(pool_source_ids.into_iter().collect()),
        account_ids: Some(pool_account_ids.into_iter().collect()),
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        model_prefix: None,
        wire_apis: Some(vec![
            ClientWireApi::Responses,
            ClientWireApi::Messages,
            ClientWireApi::ChatCompletions,
            ClientWireApi::Gemini,
        ]),
    }];
    let oauth = Arc::new(ProxyRefreshClient::new(refresh_proxies)?);
    let refresh = Arc::new(
        StoredRefreshAdapter::new(state.transient_root(), credentials.clone(), oauth, 60_000)
            .map_err(|error| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    format!("failed to initialize account refresh locks: {error:?}"),
                )
            })?,
    );
    let persistence = Arc::new(CredentialPersistence::new(
        credentials,
        state.account_metadata_sink(),
    ));
    let auth = RuntimeChatGptAuth {
        token_authority: authority,
        refresh_adapter: refresh,
        persistence_adapter: persistence,
        refresh_skew_ms: 60_000,
        agent_identities,
    };
    let options = GatewayRuntimeOptions {
        max_retry_candidates: usize::from(settings.max_retry_candidates),
        cooldown_after_failures: settings.cooldown_after_failures,
        keep_last_candidate_available: settings.keep_last_candidate_available,
        routing_strategy: settings.routing_strategy,
        subscription_plan_order: settings.subscription_plan_order,
        hidden_models: settings.hidden_models,
        default_service_tier: settings.default_service_tier,
        quota_stale_after_ms,
        image_base_model: None,
        image_pricing_catalog: Some(state.pricing_catalog()),
        model_reasoning_allowed_levels: settings.model_reasoning_allowed_levels,
        response_affinity_store: Some(state.response_affinity_store()),
        provider_storm_breaker: false,
    };
    let usage_callback = state.usage_callback();
    // A desktop configuration can legitimately have no route while the user
    // is editing sources, recovering an account, or has removed its final
    // member. Runtime construction must preserve that manageable state across
    // restarts; individual requests still require an eligible candidate.
    let runtime = GatewayRuntime::from_mixed_pool_allow_unroutable(
        sources,
        accounts,
        keys,
        auth,
        options,
        usage_callback,
    )
    .map_err(core_error)?;
    runtime.set_activity_callback(state.runtime_activity_callback());
    runtime.set_chatgpt_team_breaker_callback(state.runtime_team_breaker_callback());
    runtime.set_codex_background_tasks_enabled(settings.codex_background_tasks_enabled);
    runtime.set_codex_websockets_enabled(settings.codex_websockets_enabled);
    runtime
        .set_model_service_tier_overrides(settings.model_service_tier_overrides)
        .map_err(core_error)?;
    runtime.set_model_display_order(settings.model_display_order);
    runtime.set_protected_candidate(
        protected_account_id.as_deref(),
        settings.chatgpt_interface_quota_reserve_basis_points,
    );
    Ok(Arc::new(runtime))
}

/// ChatGPT profile recovery is an optional desktop integration. Its metadata
/// must never prevent the independent local API gateway from starting or from
/// enabling an API source. If the profile needs recovery, omit only the
/// interface quota reserve; profile actions still surface their own errors.
fn managed_chatgpt_account_id_for_reserve(
    codex_home: &std::path::Path,
    backup_root: &std::path::Path,
) -> Option<String> {
    (codex::credential_kind(codex_home, backup_root).ok()
        == Some(Some(codex::ProfileCredentialKind::LocalGateway)))
    .then(|| {
        codex::active_managed_account_id(codex_home, backup_root)
            .ok()
            .flatten()
    })
    .flatten()
}

fn timestamp_ms(value: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|value| u64::try_from(value.timestamp_millis()).ok())
}

pub(in crate::local_pool) fn runtime_account_operational_state(
    account: &AccountRecord,
    now_ms: u64,
) -> AccountOperationalState {
    account_operational_state(AccountOperationalInput {
        enabled: account.enabled,
        in_pool: account.in_pool,
        draining: account.draining,
        secret_available: true,
        proxy_available: true,
        auth_state: account.auth_state,
        health: account.health,
        subscription: &account.subscription,
        quota: &account.quota,
        last_error_code: account.last_error_code.as_deref(),
        now_ms,
        quota_stale_after_ms: QUOTA_STALE_AFTER_MS,
    })
}

pub(in crate::local_pool) async fn apply_source_policy_if_running(
    state: &DesktopState,
    previous: &[ProviderSourceRecord],
    source: &ProviderSourceRecord,
) -> bool {
    apply_source_policies_if_running(state, previous, std::slice::from_ref(source)).await
}

pub(in crate::local_pool) async fn apply_source_policies_if_running(
    state: &DesktopState,
    previous: &[ProviderSourceRecord],
    sources: &[ProviderSourceRecord],
) -> bool {
    let Some(runtime) = state.gateway.runtime().await else {
        return true;
    };
    let updates = changed_runtime_source_policy_updates(previous, sources);
    updates.is_empty() || runtime.update_source_policies(&updates)
}

/// Applies the configured Responses pool to an existing runtime. The live
/// scheduler is the source of truth for candidate availability, so this never
/// reopens credentials merely to rebuild a key scope.
pub(in crate::local_pool) fn apply_local_gateway_key_scope(
    state: &DesktopState,
    runtime: &GatewayRuntime,
) -> Result<bool> {
    let system_key = pool::ensure_system_gateway_key(state)?;
    let (sources, accounts) = {
        let store = state.store()?;
        (store.sources().to_vec(), store.accounts().to_vec())
    };
    let (source_ids, account_ids) = pool::local_pool_member_ids(&sources, &accounts)?;
    let scope = runtime.active_responses_scope(&source_ids, &account_ids);
    Ok(runtime.update_key_scope(&system_key.id, scope))
}

/// Refreshes the managed local key's candidate scope without replacing the
/// listener or any source/account executor. Source membership is represented
/// by this scope, so a policy-only source edit that also changes `in_pool`
/// does not need a gateway restart.
pub(in crate::local_pool) async fn refresh_local_gateway_key_scope_if_running(
    state: &DesktopState,
) -> Result<bool> {
    let Some(runtime) = state.gateway.runtime().await else {
        return Ok(true);
    };
    apply_local_gateway_key_scope(state, &runtime)
}

pub(in crate::local_pool) async fn apply_account_policy_if_running(
    state: &DesktopState,
    account: &LocalAccountRecord,
) -> bool {
    let Some(runtime) = state.gateway.runtime().await else {
        return true;
    };
    runtime.update_account_policy(
        &account.account.id,
        runtime_account_policy(account, current_time_ms()),
    )
}

/// Maps the persisted account state into the part of a live candidate that can
/// change without replacing its OAuth executor. Pool membership affects this
/// policy through `runtime_account_operational_state`, so adding or removing
/// an account from the local pool can be applied without restarting the
/// listener or interrupting active streams.
pub(in crate::local_pool) fn runtime_account_policy(
    account: &LocalAccountRecord,
    now_ms: u64,
) -> RuntimeCandidatePolicy {
    let operational = runtime_account_operational_state(&account.account, now_ms);
    RuntimeCandidatePolicy {
        enabled: account_candidate_enabled(
            account.account.enabled,
            operational.routing_block_reason,
        ),
        draining: account.account.draining,
        priority: account.priority,
        weight: account.weight,
        allowed_models: account.allowed_models.clone(),
        excluded_models: account.excluded_models.clone(),
    }
}

pub(in crate::local_pool) fn refresh_active_codex_catalog_in_background(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<DesktopState>();
        let result = profiles::refresh_active_codex_catalog(&state).await;
        record_catalog_refresh_result(&state, &result);
        let _ = app.emit("zenith-state-changed", ());
    });
}

pub(in crate::local_pool) async fn sync_records_or_rollback(
    state: &DesktopState,
    old_sources: Vec<ProviderSourceRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
) -> Result<()> {
    restart_or_rollback(state, || {
        state.store()?.replace_records(old_sources, old_keys)
    })
    .await
}

pub(in crate::local_pool) async fn sync_accounts_or_rollback(
    state: &DesktopState,
    old_accounts: Vec<LocalAccountRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
) -> Result<()> {
    restart_or_rollback(state, || {
        state
            .store()?
            .replace_accounts_and_keys(old_accounts, old_keys)
    })
    .await
}

pub(in crate::local_pool) async fn sync_refreshed_account_or_rollback(
    state: &DesktopState,
    account_id: &str,
    models_changed: bool,
    old_accounts: Vec<LocalAccountRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
) -> Result<()> {
    if models_changed {
        return sync_accounts_or_rollback(state, old_accounts, old_keys).await;
    }
    let Some(runtime) = state.gateway.runtime().await else {
        return Ok(());
    };
    let (enabled, health, quota, quota_updated_at_ms) = {
        let store = state.store()?;
        let account = store
            .account(account_id)
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
        let operational = runtime_account_operational_state(&account.account, current_time_ms());
        (
            account_candidate_enabled(account.account.enabled, operational.routing_block_reason),
            operational.health,
            operational.quota,
            account.account.quota.updated_at_ms,
        )
    };
    if runtime.update_candidate_availability_at(
        account_id,
        enabled,
        health,
        quota,
        quota_updated_at_ms,
    ) {
        return Ok(());
    }
    sync_accounts_or_rollback(state, old_accounts, old_keys).await
}

pub(in crate::local_pool) async fn sync_gateway_or_rollback(
    state: &DesktopState,
    old_gateway: GatewaySettings,
) -> Result<()> {
    restart_or_rollback(state, || state.store()?.replace_gateway(old_gateway)).await
}

pub(in crate::local_pool) async fn restart_after_secret_change(
    state: &DesktopState,
    secret_ref: &str,
    old_secret: &str,
) -> Result<()> {
    restart_or_rollback(state, || secret_store::save(secret_ref, old_secret)).await
}

pub(in crate::local_pool) async fn restart_or_rollback(
    state: &DesktopState,
    rollback: impl FnOnce() -> Result<()> + Send,
) -> Result<()> {
    let Some(address) = state.gateway.address().await else {
        return Ok(());
    };
    let next_port = state.store()?.gateway().port;
    let mut rollback = Some(rollback);
    let runtime = match runtime_from_store(state).await {
        Ok(runtime) => runtime,
        Err(error) => {
            apply_rollback(state, rollback.take().unwrap()).await?;
            return Err(error);
        }
    };

    state.gateway.stop().await;
    let restart_error = state.gateway.start(runtime, next_port).await.err();
    if let Some(error) = restart_error {
        state.gateway.stop().await;
        apply_rollback(state, rollback.take().unwrap()).await?;
        let old_runtime = match runtime_from_store(state).await {
            Ok(runtime) => runtime,
            Err(restore) => {
                return Err(fail_closed(
                    state,
                    format!("{error}; failed to rebuild previous gateway: {restore}"),
                )
                .await)
            }
        };
        if let Err(restart) = state.gateway.start(old_runtime, address.port()).await {
            return Err(fail_closed(
                state,
                format!("{error}; failed to restart previous gateway: {restart}"),
            )
            .await);
        }
        return Err(error);
    }
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.prefetch_source_model_metadata();
    }
    let result = profiles::refresh_active_codex_catalog(state).await;
    record_catalog_refresh_result(state, &result);
    Ok(())
}

async fn apply_rollback(state: &DesktopState, rollback: impl FnOnce() -> Result<()>) -> Result<()> {
    if let Err(error) = rollback() {
        return Err(fail_closed(
            state,
            format!("failed to restore previous gateway state: {error}"),
        )
        .await);
    }
    Ok(())
}

fn disable_gateway(state: &DesktopState) -> Result<()> {
    state.store()?.set_gateway_enabled(false)
}

pub(in crate::local_pool) async fn fail_closed(
    state: &DesktopState,
    message: String,
) -> LocalPoolError {
    state.gateway.stop().await;
    match disable_gateway(state) {
        Ok(()) => LocalPoolError::new(ErrorCode::RecoveryRequired, message),
        Err(error) => LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("{message}; failed to disable gateway state: {error}"),
        ),
    }
}

pub(in crate::local_pool) fn core_error(error: zenith_relay_core::Error) -> LocalPoolError {
    let message = error.to_string();
    let code = match &error {
        zenith_relay_core::Error::Upstream(_)
        | zenith_relay_core::Error::UpstreamBodyTooLarge
        | zenith_relay_core::Error::UpstreamStatus(_)
        | zenith_relay_core::Error::InvalidUpstreamResponse(_) => ErrorCode::SourceTestFailed,
        zenith_relay_core::Error::Validation(_) | zenith_relay_core::Error::UnsupportedWireApi => {
            ErrorCode::InvalidState
        }
    };
    let mut local_error = LocalPoolError::new(code, message);
    if let zenith_relay_core::Error::UpstreamStatus(status) = error {
        local_error = local_error.with_diagnostic(ErrorDiagnostics {
            status: Some(status),
            retryable: Some(status == 408 || status == 429 || status >= 500),
            ..Default::default()
        });
    }
    local_error
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn persisted_last_used_timestamp_maps_to_epoch_milliseconds() {
        assert_eq!(timestamp_ms("1970-01-01T00:00:00.001Z"), Some(1));
        assert_eq!(timestamp_ms("not-a-date"), None);
    }

    #[test]
    fn profile_recovery_error_does_not_block_the_api_gateway_reserve_setup() {
        let root =
            std::env::temp_dir().join(format!("zenith-runtime-profile-{}", uuid::Uuid::new_v4()));
        let profile = root.join("profile");
        let recovery = root.join("recovery");
        fs::create_dir_all(&profile).unwrap();
        fs::create_dir_all(&recovery).unwrap();
        fs::write(recovery.join("codex-default.json"), "invalid backup").unwrap();

        assert_eq!(
            managed_chatgpt_account_id_for_reserve(&profile, &recovery),
            None
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exhausted_account_stays_configured_while_the_scheduler_blocks_requests() {
        assert!(account_candidate_enabled(
            true,
            Some(AccountRoutingBlockReason::QuotaExhausted)
        ));
        assert!(account_candidate_enabled(true, None));
        assert!(!account_candidate_enabled(
            true,
            Some(AccountRoutingBlockReason::ReauthRequired)
        ));
        for reason in [
            AccountRoutingBlockReason::AuthError,
            AccountRoutingBlockReason::Checkpoint,
            AccountRoutingBlockReason::Captcha,
            AccountRoutingBlockReason::SubscriptionForbidden,
            AccountRoutingBlockReason::SubscriptionExpired,
            AccountRoutingBlockReason::AccountUnhealthy,
        ] {
            assert!(!account_candidate_enabled(true, Some(reason)));
        }
        assert!(!account_candidate_enabled(
            false,
            Some(AccountRoutingBlockReason::QuotaExhausted)
        ));
    }

    #[tokio::test]
    async fn runtime_creates_and_reuses_the_system_gateway_key() {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("zenith-relay-system-key-{id}"));
        let source_secret_ref = format!("source:system-key-{id}");
        let state = DesktopState::open(root.clone()).unwrap();
        secret_store::save(&source_secret_ref, "upstream-secret").unwrap();
        state
            .store()
            .unwrap()
            .upsert_source(ProviderSourceRecord {
                id: "source_1".into(),
                name: "Synthetic".into(),
                enabled: true,
                in_pool: true,
                draining: false,
                base_url: "http://127.0.0.1:9/v1".into(),
                secret_ref: source_secret_ref.clone(),
                pricing_provider: None,
                official_provider_family: None,
                wire_api: zenith_relay_core::WireApi::Responses,
                protocol_bindings: Vec::new(),
                models: vec!["gpt-test".into()],
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
            })
            .unwrap();

        let runtime = runtime_from_store(&state).await.unwrap();
        let key = state.store().unwrap().keys()[0].clone();
        let secret = secret_store::load(&key.secret_ref).unwrap().unwrap();
        assert!(key.system);
        assert!(key.enabled);
        assert!(secret.starts_with("zlr_"));

        runtime_from_store(&state).await.unwrap();
        let reused = state.store().unwrap().keys()[0].clone();
        assert_eq!(reused.id, key.id);
        assert_eq!(
            secret_store::load(&reused.secret_ref).unwrap().as_deref(),
            Some(secret.as_str())
        );

        let address = state.gateway.start(runtime, 0).await.unwrap();
        let response = reqwest::Client::new()
            .get(format!("http://{address}/v1/models"))
            .bearer_auth(&secret)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());

        state.gateway.stop().await;
        secret_store::delete(&source_secret_ref).unwrap();
        secret_store::delete(&key.secret_ref).unwrap();
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn runtime_restarts_after_pool_eviction_and_source_deletion() {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("zenith-relay-empty-pool-{id}"));
        let source_secret_ref = format!("source:empty-pool-{id}");
        let state = DesktopState::open(root.clone()).unwrap();
        secret_store::save(&source_secret_ref, "upstream-secret").unwrap();
        let source = ProviderSourceRecord {
            id: "source_1".into(),
            name: "Synthetic".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "http://127.0.0.1:9/v1".into(),
            secret_ref: source_secret_ref.clone(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            models: vec!["gpt-test".into()],
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
        };
        state
            .store()
            .unwrap()
            .upsert_source(source.clone())
            .unwrap();

        let runtime = runtime_from_store(&state).await.unwrap();
        let key = state
            .store()
            .unwrap()
            .keys()
            .iter()
            .find(|key| key.system)
            .cloned()
            .unwrap();
        let secret = secret_store::load(&key.secret_ref).unwrap().unwrap();
        let address = state.gateway.start(runtime, 0).await.unwrap();
        let mut gateway = state.store().unwrap().gateway().clone();
        gateway.port = address.port();
        state.store().unwrap().replace_gateway(gateway).unwrap();
        let client = reqwest::Client::new();
        let initial_models: serde_json::Value = client
            .get(format!("http://{address}/v1/models"))
            .bearer_auth(&secret)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(initial_models["data"].as_array().unwrap().len(), 1);

        let mut outside_pool = source;
        outside_pool.in_pool = false;
        let (old_sources, keys) = {
            let store = state.store().unwrap();
            (store.sources().to_vec(), store.keys().to_vec())
        };
        state
            .store()
            .unwrap()
            .replace_records(vec![outside_pool], keys.clone())
            .unwrap();
        restart_or_rollback(&state, || {
            state.store()?.replace_records(old_sources, keys.clone())
        })
        .await
        .unwrap();
        assert_eq!(state.gateway.address().await, Some(address));
        let client = reqwest::Client::new();
        let evicted_models: serde_json::Value = client
            .get(format!("http://{address}/v1/models"))
            .bearer_auth(&secret)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(evicted_models["data"].as_array().unwrap().is_empty());

        let (old_sources, old_keys) = {
            let store = state.store().unwrap();
            (store.sources().to_vec(), store.keys().to_vec())
        };
        state
            .store()
            .unwrap()
            .replace_records(Vec::new(), old_keys.clone())
            .unwrap();
        restart_or_rollback(&state, || {
            state.store()?.replace_records(old_sources, old_keys)
        })
        .await
        .unwrap();
        assert_eq!(state.gateway.address().await, Some(address));
        let client = reqwest::Client::new();
        let deleted_models: serde_json::Value = client
            .get(format!("http://{address}/v1/models"))
            .bearer_auth(&secret)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(deleted_models["data"].as_array().unwrap().is_empty());

        state.gateway.stop().await;
        let restarted_runtime = runtime_from_store(&state).await.unwrap();
        assert!(restarted_runtime
            .visible_models_for_secret(&secret, &[WireApi::Responses], current_time_ms())
            .is_empty());
        drop(restarted_runtime);
        secret_store::delete(&source_secret_ref).unwrap();
        secret_store::delete(&key.secret_ref).unwrap();
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn runtime_repairs_missing_enabled_gateway_key_secret() {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("zenith-relay-key-repair-{id}"));
        let source_secret_ref = format!("source:key-repair-{id}");
        let key_secret_ref = format!("key:key-repair-{id}");
        let state = DesktopState::open(root.clone()).unwrap();
        secret_store::save(&source_secret_ref, "upstream-secret").unwrap();
        state
            .store()
            .unwrap()
            .upsert_source(ProviderSourceRecord {
                id: "source_1".into(),
                name: "Synthetic".into(),
                enabled: true,
                in_pool: true,
                draining: false,
                base_url: "http://127.0.0.1:9/v1".into(),
                secret_ref: source_secret_ref.clone(),
                pricing_provider: None,
                official_provider_family: None,
                wire_api: zenith_relay_core::WireApi::Responses,
                protocol_bindings: Vec::new(),
                models: vec!["gpt-test".into()],
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
            })
            .unwrap();
        state
            .store()
            .unwrap()
            .upsert_key(LocalGatewayKeyRecord {
                id: "key_1".into(),
                label: "Default".into(),
                enabled: true,
                system: true,
                secret_ref: key_secret_ref.clone(),
                created_at: "2026-07-15T00:00:00Z".into(),
                last_used_at: None,
            })
            .unwrap();

        let runtime = runtime_from_store(&state).await.unwrap();
        let generated = secret_store::load(&key_secret_ref).unwrap().unwrap();
        assert!(generated.starts_with("zlr_"));
        let address = state.gateway.start(runtime, 0).await.unwrap();
        let response = reqwest::Client::new()
            .get(format!("http://{address}/v1/models"))
            .bearer_auth(&generated)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());

        state.gateway.stop().await;
        secret_store::delete(&source_secret_ref).unwrap();
        secret_store::delete(&key_secret_ref).unwrap();
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn build_failure_rolls_back_once_without_stopping_old_gateway() {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("zenith-relay-command-rollback-{id}"));
        let source_secret_ref = format!("source:command-rollback-{id}");
        let key_secret_ref = format!("key:command-rollback-{id}");
        let state = DesktopState::open(root.clone()).unwrap();
        secret_store::save(&source_secret_ref, "upstream-secret").unwrap();
        secret_store::save(&key_secret_ref, "old-secret").unwrap();
        let source = ProviderSourceRecord {
            id: "old_source".into(),
            name: "Old".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "http://127.0.0.1:9/v1".into(),
            secret_ref: source_secret_ref.clone(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            models: vec!["old-model".into()],
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
        };
        let key = LocalGatewayKeyRecord {
            id: "old_key".into(),
            label: "Old key".into(),
            enabled: true,
            system: true,
            secret_ref: key_secret_ref.clone(),
            created_at: "2026-08-05T00:00:00Z".into(),
            last_used_at: None,
        };
        state
            .store()
            .unwrap()
            .replace_records(vec![source.clone()], vec![key])
            .unwrap();
        let runtime = runtime_from_store(&state).await.unwrap();
        let address = state.gateway.start(runtime, 0).await.unwrap();
        let (old_sources, old_keys) = {
            let store = state.store().unwrap();
            (store.sources().to_vec(), store.keys().to_vec())
        };
        let secret_refs = old_keys
            .iter()
            .map(|key| key.secret_ref.clone())
            .collect::<Vec<_>>();
        let mut invalid_source = source;
        invalid_source.protocol_bindings = vec![zenith_relay_core::SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: zenith_relay_core::SourceAdapter::ResponsesToMessages,
            reasoning_mode: zenith_relay_core::MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["old-model".into()],
        }];
        state
            .store()
            .unwrap()
            .replace_records(vec![invalid_source], old_keys.clone())
            .unwrap();
        let rollback_calls = Arc::new(AtomicUsize::new(0));

        assert!(restart_or_rollback(&state, || {
            rollback_calls.fetch_add(1, Ordering::SeqCst);
            state.store()?.replace_records(old_sources, old_keys)
        })
        .await
        .is_err());
        assert_eq!(rollback_calls.load(Ordering::SeqCst), 1);
        assert_eq!(state.gateway.address().await, Some(address));
        let response = reqwest::Client::new()
            .get(format!("http://{address}/v1/models"))
            .bearer_auth("old-secret")
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert!(
            status.is_success(),
            "old listener returned {status}: {body}"
        );
        assert!(body.contains("old-model"));

        state.gateway.stop().await;
        secret_store::delete(&source_secret_ref).unwrap();
        for secret_ref in secret_refs {
            secret_store::delete(&secret_ref).unwrap();
        }
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn occupied_new_port_restores_settings_and_previous_listener() {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("zenith-relay-port-rollback-{id}"));
        let source_secret_ref = format!("source:port-rollback-{id}");
        let key_secret_ref = format!("key:port-rollback-{id}");
        let state = DesktopState::open(root.clone()).unwrap();
        secret_store::save(&source_secret_ref, "upstream-secret").unwrap();
        secret_store::save(&key_secret_ref, "local-secret").unwrap();
        state
            .store()
            .unwrap()
            .upsert_source(ProviderSourceRecord {
                id: "source_1".into(),
                name: "Synthetic".into(),
                enabled: true,
                in_pool: true,
                draining: false,
                base_url: "http://127.0.0.1:9/v1".into(),
                secret_ref: source_secret_ref.clone(),
                pricing_provider: None,
                official_provider_family: None,
                wire_api: zenith_relay_core::WireApi::Responses,
                protocol_bindings: Vec::new(),
                models: vec!["gpt-test".into()],
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
            })
            .unwrap();
        state
            .store()
            .unwrap()
            .upsert_key(LocalGatewayKeyRecord {
                id: "key_1".into(),
                label: "Default".into(),
                enabled: true,
                system: true,
                secret_ref: key_secret_ref.clone(),
                created_at: "2026-07-11T00:00:00Z".into(),
                last_used_at: None,
            })
            .unwrap();
        let address = state
            .gateway
            .start(runtime_from_store(&state).await.unwrap(), 0)
            .await
            .unwrap();
        let mut old_gateway = state.store().unwrap().gateway().clone();
        old_gateway.port = address.port();
        state
            .store()
            .unwrap()
            .replace_gateway(old_gateway.clone())
            .unwrap();
        let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut next_gateway = old_gateway.clone();
        next_gateway.port = occupied.local_addr().unwrap().port();
        state
            .store()
            .unwrap()
            .replace_gateway(next_gateway)
            .unwrap();

        assert!(sync_gateway_or_rollback(&state, old_gateway.clone())
            .await
            .is_err());
        assert_eq!(state.store().unwrap().gateway().port, old_gateway.port);
        assert_eq!(state.gateway.address().await, Some(address));

        drop(occupied);
        state.gateway.stop().await;
        secret_store::delete(&source_secret_ref).unwrap();
        secret_store::delete(&key_secret_ref).unwrap();
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
