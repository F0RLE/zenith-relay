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
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tauri::{AppHandle, Emitter, Manager};
use zenith_relay_core::error_codes;
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

/// A malformed source record must not make an otherwise usable local pool
/// disappear. Keep the source in the inventory, exclude only its runtime
/// route, and expose stable codes to the UI so it can be repaired.
const SOURCE_PROTOCOL_INVALID_CODE: &str = error_codes::SOURCE_PROTOCOL_INVALID;
const SOURCE_RUNTIME_INVALID_CODE: &str = error_codes::SOURCE_RUNTIME_INVALID;

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
    crate::diagnostics::breadcrumb("gateway-runtime", "build_started", &[]);
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
    let pool_routing = settings.pool_routing_for(&source_records, &account_records);
    let quota_stale_after_ms = QUOTA_STALE_AFTER_MS;
    // The managed profile can expose every verified source protocol. Requests
    // still select only the protocol they actually use at the gateway edge.
    let (mut pool_source_ids, pool_account_ids) =
        pool::local_pool_member_ids(&source_records, &account_records)?;
    let mut sources = Vec::new();
    let mut source_ids = HashSet::new();
    for source in source_records {
        let Some(api_key) = source_api_keys.get(&source.id).cloned().flatten() else {
            continue;
        };
        let protocol_bindings = match source.effective_protocol_bindings() {
            Ok(bindings) => bindings,
            Err(_) => {
                quarantine_source_runtime_error(state, &source, SOURCE_PROTOCOL_INVALID_CODE);
                continue;
            }
        };
        let runtime_source = ProviderSource {
            id: source.id.clone(),
            name: source.name.clone(),
            base_url: source.base_url.clone(),
            api_key,
            wire_api: source.wire_api,
            models: source.models.clone(),
        };
        if runtime_source.validate().is_err()
            || source.weight == 0
            || source.recovery_delay_seconds > 24 * 60 * 60
        {
            quarantine_source_runtime_error(state, &source, SOURCE_RUNTIME_INVALID_CODE);
            continue;
        }
        if !source_ids.insert(source.id.clone()) {
            quarantine_source_runtime_error(state, &source, SOURCE_RUNTIME_INVALID_CODE);
            continue;
        }
        clear_source_runtime_error(state, &source);
        sources.push(RuntimeSource {
            source: runtime_source,
            protocol_bindings,
            protocol_config: source.protocol_config,
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
    // Key scopes must reference only source executors admitted above. Keeping
    // a stale id for a malformed or credential-less source can make the core
    // reject an otherwise valid mixed pool while rebuilding the gateway.
    pool_source_ids.retain(|id| source_ids.contains(id));
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
                // Runtime reconstruction must not erase a newer in-memory
                // refresh or its pending durable metadata retry with the
                // snapshot read at the beginning of this build.
                .register_if_newer(
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
        pool_routing: Some(pool_routing),
        subscription_plan_order: settings.subscription_plan_order,
        hidden_models: settings.hidden_models,
        default_service_tier: settings.default_service_tier,
        quota_stale_after_ms,
        image_base_model: None,
        image_pricing_catalog: Some(state.pricing_catalog()),
        model_metadata_catalog: Some(state.model_metadata_loader().catalog_handle()),
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
    .map_err(|error| {
        let local = core_error(error);
        crate::diagnostics::record_error(
            "gateway-runtime",
            Some("runtime_build_failed"),
            &local.message,
            &[],
        );
        local
    })?;
    runtime.set_activity_callback(state.runtime_activity_callback());
    runtime.set_chatgpt_team_breaker_callback(state.runtime_team_breaker_callback());
    runtime.set_codex_background_tasks_enabled(settings.codex_background_tasks_enabled);
    runtime.set_codex_websockets_enabled(settings.codex_websockets_enabled);
    runtime.set_chatgpt_retry_until_available(settings.chatgpt_retry_until_available);
    runtime
        .set_model_service_tier_overrides(settings.model_service_tier_overrides)
        .map_err(core_error)?;
    runtime.set_model_display_order(settings.model_display_order);
    runtime.set_protected_candidate(
        protected_account_id.as_deref(),
        settings.chatgpt_interface_quota_reserve_basis_points,
    );
    crate::diagnostics::breadcrumb("gateway-runtime", "build_completed", &[]);
    Ok(Arc::new(runtime))
}

/// Persist a stable, redacted error for a source that cannot be admitted to
/// the runtime. The source remains visible in the Connections and Pool
/// screens, where the missing runtime candidate is rendered as unavailable;
/// only this source is removed from the current runtime build.
fn quarantine_source_runtime_error(
    state: &DesktopState,
    source: &ProviderSourceRecord,
    code: &str,
) {
    let source_hash = crate::diagnostics::hash_identifier(&source.id);
    crate::diagnostics::record_error(
        "gateway-runtime",
        Some(code),
        "source configuration is invalid and was excluded from the local gateway",
        &[("source", source_hash.clone())],
    );

    let persisted = (|| -> Result<()> {
        let mut store = state.store()?;
        let Some(current) = store.source(&source.id).cloned() else {
            return Ok(());
        };
        if !same_source_runtime_configuration(&current, source)
            || current.last_error.as_deref() == Some(code)
        {
            return Ok(());
        }
        let mut updated = current;
        updated.last_error = Some(code.to_string());
        store.upsert_source(updated)
    })();
    if persisted.is_err() {
        crate::diagnostics::record_error(
            "gateway-runtime",
            Some("source_runtime_error_persist_failed"),
            "source runtime error could not be saved",
            &[("source", source_hash)],
        );
    }
}

/// Clear only the error owned by runtime admission. Probe/discovery errors
/// belong to their own operation and must not be erased by an unrelated
/// runtime rebuild.
fn clear_source_runtime_error(state: &DesktopState, source: &ProviderSourceRecord) {
    let persisted = (|| -> Result<()> {
        let mut store = state.store()?;
        let Some(current) = store.source(&source.id).cloned() else {
            return Ok(());
        };
        let runtime_error = current.last_error.as_deref().is_some_and(|error| {
            error == SOURCE_PROTOCOL_INVALID_CODE || error == SOURCE_RUNTIME_INVALID_CODE
        });
        if !same_source_runtime_configuration(&current, source) || !runtime_error {
            return Ok(());
        }
        let mut updated = current;
        updated.last_error = None;
        store.upsert_source(updated)
    })();
    if persisted.is_err() {
        crate::diagnostics::record_error(
            "gateway-runtime",
            Some("source_runtime_error_clear_failed"),
            "source runtime error could not be cleared",
            &[("source", crate::diagnostics::hash_identifier(&source.id))],
        );
    }
}

fn same_source_runtime_configuration(
    current: &ProviderSourceRecord,
    expected: &ProviderSourceRecord,
) -> bool {
    current.id == expected.id
        && current.name == expected.name
        && current.enabled == expected.enabled
        && current.in_pool == expected.in_pool
        && current.draining == expected.draining
        && current.base_url == expected.base_url
        && current.secret_ref == expected.secret_ref
        && current.wire_api == expected.wire_api
        && current.protocol_bindings == expected.protocol_bindings
        && current.models == expected.models
        && current.allowed_models == expected.allowed_models
        && current.excluded_models == expected.excluded_models
        && current.priority == expected.priority
        && current.weight == expected.weight
        && current.recovery_delay_seconds == expected.recovery_delay_seconds
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
    let (sources, accounts, settings) = {
        let store = state.store()?;
        (
            store.sources().to_vec(),
            store.accounts().to_vec(),
            store.gateway().clone(),
        )
    };
    runtime
        .set_pool_routing_policy(
            settings.pool_routing_for(&sources, &accounts),
            settings.max_retry_candidates,
            settings.cooldown_after_failures,
            settings.keep_last_candidate_available,
        )
        .map_err(core_error)?;
    let (source_ids, account_ids) = pool::local_pool_member_ids(&sources, &accounts)?;
    // Authorization follows configured membership. Temporary auth failures,
    // cooldowns and disables are enforced by the scheduler and must recover
    // without a second membership edit.
    let scope = zenith_relay_core::CandidateScope {
        source_ids: Some(source_ids),
        account_ids: Some(account_ids),
        model_rules: Default::default(),
    };
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

/// Refresh authentication health and quota from the current durable record.
/// Policy edits use a separate path so changing a label or priority cannot
/// clear a scheduler failure observed during an in-flight request.
pub(in crate::local_pool) async fn sync_account_state_if_running(
    state: &DesktopState,
    account_id: &str,
) -> bool {
    let Some(runtime) = state.gateway.runtime().await else {
        return true;
    };
    let Ok(store) = state.store() else {
        return false;
    };
    store
        .account(account_id)
        .is_some_and(|account| sync_runtime_account_state(&runtime, account, current_time_ms()))
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
        let result = profiles::refresh_active_client_catalogs(&state).await;
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

pub(in crate::local_pool) async fn sync_account_or_rollback(
    state: &DesktopState,
    previous_account: LocalAccountRecord,
    attempted_account: LocalAccountRecord,
) -> Result<()> {
    restart_or_rollback(state, move || {
        state
            .store()?
            .restore_account_if_current(&previous_account, &attempted_account)
            .map(|_| ())
    })
    .await
}

/// Reconciles account policy and quota snapshots that may have changed while
/// the startup runtime was being constructed. Automatic quota refreshes run
/// independently of Gateway startup, so a refresh can finish before the
/// listener exists and otherwise have no live scheduler to update.
pub(in crate::local_pool) async fn sync_running_account_states(state: &DesktopState) -> Result<()> {
    let Some(runtime) = state.gateway.runtime().await else {
        return Ok(());
    };
    let accounts = state.store()?.accounts().to_vec();
    let observed_at_ms = current_time_ms();
    for account in accounts {
        sync_runtime_account_state(&runtime, &account, observed_at_ms);
    }
    Ok(())
}

pub(in crate::local_pool) async fn sync_refreshed_account_or_rollback(
    state: &DesktopState,
    previous_account: LocalAccountRecord,
    attempted_account: LocalAccountRecord,
    models_changed: bool,
) -> Result<()> {
    let account_id = attempted_account.account.id.clone();
    if models_changed {
        return sync_account_or_rollback(state, previous_account, attempted_account).await;
    }
    let Some(runtime) = state.gateway.runtime().await else {
        return Ok(());
    };
    let account = state
        .store()?
        .account(&account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    if sync_runtime_account_state(&runtime, &account, current_time_ms()) {
        return Ok(());
    }
    sync_account_or_rollback(state, previous_account, attempted_account).await
}

pub(in crate::local_pool) fn sync_runtime_account_state(
    runtime: &GatewayRuntime,
    account: &LocalAccountRecord,
    observed_at_ms: u64,
) -> bool {
    let operational = runtime_account_operational_state(&account.account, observed_at_ms);
    runtime.sync_account_availability_with_quota(
        &account.account.id,
        account_candidate_enabled(account.account.enabled, operational.routing_block_reason),
        operational.health,
        &account.account.quota,
        observed_at_ms,
    )
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
    crate::diagnostics::breadcrumb("gateway-runtime", "restart_started", &[]);
    let Some(address) = state.gateway.address().await else {
        crate::diagnostics::breadcrumb("gateway-runtime", "restart_skipped", &[]);
        return Ok(());
    };
    let next_port = state.store()?.gateway().port;
    let mut rollback = Some(rollback);
    let runtime = match runtime_from_store(state).await {
        Ok(runtime) => runtime,
        Err(error) => {
            crate::diagnostics::record_error(
                "gateway-runtime",
                Some("runtime_rebuild_failed"),
                &error.message,
                &[],
            );
            let Some(rollback) = rollback.take() else {
                return Err(fail_closed(
                    state,
                    format!("{error}; gateway rollback callback was consumed unexpectedly"),
                )
                .await);
            };
            apply_rollback(state, rollback).await?;
            return Err(error);
        }
    };

    crate::diagnostics::breadcrumb(
        "gateway-runtime",
        "gateway_stop_started",
        &[("port", address.port().to_string())],
    );
    state.gateway.stop().await;
    crate::diagnostics::breadcrumb(
        "gateway-runtime",
        "gateway_start_started",
        &[("port", next_port.to_string())],
    );
    let restart_error = state.gateway.start(runtime, next_port).await.err();
    if let Some(error) = restart_error {
        crate::diagnostics::record_error(
            "gateway-runtime",
            Some("gateway_restart_failed"),
            &error.to_string(),
            &[],
        );
        state.gateway.stop().await;
        let Some(rollback) = rollback.take() else {
            return Err(fail_closed(
                state,
                format!("{error}; gateway rollback callback was consumed unexpectedly"),
            )
            .await);
        };
        apply_rollback(state, rollback).await?;
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
    crate::diagnostics::breadcrumb(
        "gateway-runtime",
        "gateway_started",
        &[("port", next_port.to_string())],
    );
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.prefetch_source_model_metadata();
    }
    crate::diagnostics::breadcrumb("gateway-runtime", "catalog_refresh_started", &[]);
    let result = profiles::refresh_active_client_catalogs(state).await;
    record_catalog_refresh_result(state, &result);
    crate::diagnostics::record_operation("gateway-runtime", "restart_completed", &[]);
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
    crate::diagnostics::record_error("gateway-runtime", Some("fail_closed"), &message, &[]);
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
    use crate::local_pool::accounts::{
        authority::AccountMetadataSink, credentials::StoredCodexCredentials, records,
    };
    use std::fs;

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
                protocol_config: Default::default(),
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
    async fn invalid_source_is_quarantined_without_blocking_other_routes() {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("zenith-relay-source-quarantine-{id}"));
        let valid_secret_ref = format!("source:valid-quarantine-{id}");
        let invalid_secret_ref = format!("source:invalid-quarantine-{id}");
        let state = DesktopState::open(root.clone()).unwrap();
        secret_store::save(&valid_secret_ref, "valid-upstream-secret").unwrap();
        secret_store::save(&invalid_secret_ref, "invalid-upstream-secret").unwrap();
        let source = |id: &str, secret_ref: String, model: &str| ProviderSourceRecord {
            id: id.into(),
            name: id.into(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "http://127.0.0.1:9/v1".into(),
            secret_ref,
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
            model_price_overrides: Default::default(),
            detected_model_prices: Default::default(),
            last_used_at: None,
            last_test_at: None,
            last_test_status: None,
            last_error: None,
        };
        let valid = source("valid_source", valid_secret_ref.clone(), "gpt-valid");
        let mut invalid = source("invalid_source", invalid_secret_ref.clone(), "gpt-invalid");
        invalid.protocol_bindings = vec![zenith_relay_core::SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: zenith_relay_core::SourceAdapter::ResponsesToMessages,
            reasoning_mode: zenith_relay_core::MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["gpt-invalid".into()],
        }];
        state
            .store()
            .unwrap()
            .replace_records(vec![valid, invalid], Vec::new())
            .unwrap();

        let runtime = runtime_from_store(&state).await.unwrap();
        let order = runtime.candidate_runtime_order();
        assert!(order
            .iter()
            .any(|candidate| candidate.candidate_id == "valid_source"));
        assert!(!order
            .iter()
            .any(|candidate| candidate.candidate_id == "invalid_source"));
        assert_eq!(
            state
                .store()
                .unwrap()
                .source("invalid_source")
                .and_then(|source| source.last_error.as_deref()),
            Some("source_protocol_invalid")
        );

        secret_store::delete(&valid_secret_ref).unwrap();
        secret_store::delete(&invalid_secret_ref).unwrap();
        let key_secret_ref = state
            .store()
            .unwrap()
            .keys()
            .iter()
            .find(|key| key.system)
            .map(|key| key.secret_ref.clone());
        if let Some(secret_ref) = key_secret_ref {
            secret_store::delete(&secret_ref).unwrap();
        }
        drop(runtime);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn startup_reconciles_quota_persisted_before_listener_creation() {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("zenith-relay-startup-quota-{id}"));
        let account_id = format!("account_startup_{id}");
        let state = DesktopState::open(root.clone()).unwrap();
        let now_ms = current_time_ms();
        let credentials = StoredCodexCredentials::new(
            &account_id,
            "access-startup".into(),
            Some("refresh-startup".into()),
            Some("id-startup".into()),
            Some(now_ms.saturating_add(60_000)),
            now_ms,
            1,
            None,
            Some(format!("provider-{id}")),
            None,
            None,
            Some("plus".into()),
            false,
        )
        .unwrap();
        let credentials_store = CredentialStore::from_backend(NativeSecretBackend);
        credentials_store.save(&credentials).unwrap();
        let mut account = records::new_account_record(
            &credentials,
            zenith_relay_core::accounts::AccountAuthMode::OAuth,
            vec!["gpt-test".into()],
            0,
            now_ms,
        )
        .unwrap();
        account.account.in_pool = true;
        account.account.quota = zenith_relay_core::quota::QuotaSnapshot {
            limit_reached: true,
            updated_at_ms: Some(now_ms),
            ..Default::default()
        };
        state
            .store()
            .unwrap()
            .upsert_account(account.clone())
            .unwrap();

        // Build the same stale runtime that can be captured while a startup
        // quota refresh is still writing its result to the store.
        let stale_runtime = runtime_from_store(&state).await.unwrap();
        assert!(
            !stale_runtime
                .candidate_runtime_order()
                .into_iter()
                .find(|candidate| candidate.candidate_id == account_id)
                .expect("startup account candidate")
                .available
        );
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        state.gateway.start(stale_runtime, port).await.unwrap();

        account.account.quota = zenith_relay_core::quota::QuotaSnapshot {
            limit_reached: true,
            available_credits_micro_units: Some(123),
            provider_credits_available: true,
            updated_at_ms: Some(now_ms.saturating_add(1)),
            ..Default::default()
        };
        state.store().unwrap().upsert_account(account).unwrap();
        sync_running_account_states(&state).await.unwrap();

        assert!(
            state
                .gateway
                .runtime()
                .await
                .unwrap()
                .candidate_runtime_order()
                .into_iter()
                .find(|candidate| candidate.candidate_id == account_id)
                .expect("reconciled account candidate")
                .available
        );

        let key_secret_ref = state
            .store()
            .unwrap()
            .keys()
            .iter()
            .find(|key| key.system)
            .map(|key| key.secret_ref.clone());
        state.gateway.stop().await;
        credentials_store.delete(&account_id).unwrap();
        if let Some(secret_ref) = key_secret_ref {
            secret_store::delete(&secret_ref).unwrap();
        }
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn persisted_reauth_disables_only_its_running_pool_account() {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("zenith-relay-persisted-reauth-{id}"));
        let broken_account_id = format!("account-reauth-{id}");
        let healthy_account_id = format!("account-healthy-{id}");
        let now_ms = current_time_ms();
        let state = DesktopState::open(root.clone()).unwrap();
        let credentials_store = CredentialStore::from_backend(NativeSecretBackend);
        let credentials = |account_id: &str, provider_account_id: &str| {
            StoredCodexCredentials::new(
                account_id,
                "synthetic-access".into(),
                Some("synthetic-refresh".into()),
                Some("synthetic-id".into()),
                Some(now_ms.saturating_add(60_000)),
                now_ms,
                1,
                None,
                Some(provider_account_id.to_string()),
                None,
                None,
                Some("plus".into()),
                false,
            )
            .unwrap()
        };
        let broken_credentials = credentials(&broken_account_id, "provider-reauth");
        let healthy_credentials = credentials(&healthy_account_id, "provider-healthy");
        credentials_store.save(&broken_credentials).unwrap();
        credentials_store.save(&healthy_credentials).unwrap();

        let account = |credentials: &StoredCodexCredentials| {
            let mut account = records::new_account_record(
                credentials,
                zenith_relay_core::accounts::AccountAuthMode::OAuth,
                vec!["gpt-test".into()],
                0,
                now_ms,
            )
            .unwrap();
            account.account.in_pool = true;
            account
        };
        state
            .store()
            .unwrap()
            .upsert_account(account(&broken_credentials))
            .unwrap();
        state
            .store()
            .unwrap()
            .upsert_account(account(&healthy_credentials))
            .unwrap();

        let runtime = runtime_from_store(&state).await.unwrap();
        state.gateway.start(runtime, 0).await.unwrap();
        state
            .account_metadata_sink()
            .persist_auth_state(
                &broken_account_id,
                zenith_relay_core::accounts::AccountAuthState::RequiresReauth(
                    zenith_relay_core::accounts::ReauthReason::InvalidatedRefreshToken,
                ),
            )
            .await
            .unwrap();
        let candidates = state
            .gateway
            .runtime()
            .await
            .unwrap()
            .candidate_runtime_order();
        let broken_available = candidates
            .iter()
            .find(|candidate| candidate.candidate_id == broken_account_id)
            .expect("reauth candidate")
            .available;
        let healthy_available = candidates
            .iter()
            .find(|candidate| candidate.candidate_id == healthy_account_id)
            .expect("healthy candidate")
            .available;
        let persisted_auth_state = state
            .store()
            .unwrap()
            .account(&broken_account_id)
            .expect("persisted reauth account")
            .account
            .auth_state;
        let key_secret_ref = state
            .store()
            .unwrap()
            .keys()
            .iter()
            .find(|key| key.system)
            .map(|key| key.secret_ref.clone());

        state.gateway.stop().await;
        credentials_store.delete(&broken_account_id).unwrap();
        credentials_store.delete(&healthy_account_id).unwrap();
        if let Some(secret_ref) = key_secret_ref {
            secret_store::delete(&secret_ref).unwrap();
        }
        drop(state);
        std::fs::remove_dir_all(root).unwrap();

        assert_eq!(
            persisted_auth_state,
            zenith_relay_core::accounts::AccountAuthState::RequiresReauth(
                zenith_relay_core::accounts::ReauthReason::InvalidatedRefreshToken,
            )
        );
        assert!(!broken_available);
        assert!(healthy_available);
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
            protocol_config: Default::default(),
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
                protocol_config: Default::default(),
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
    async fn invalid_source_start_keeps_the_remaining_pool_route_available() {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("zenith-relay-source-restart-{id}"));
        let source_secret_ref = format!("source:source-restart-{id}");
        let invalid_secret_ref = format!("source:invalid-restart-{id}");
        let key_secret_ref = format!("key:source-restart-{id}");
        let state = DesktopState::open(root.clone()).unwrap();
        secret_store::save(&source_secret_ref, "upstream-secret").unwrap();
        secret_store::save(&invalid_secret_ref, "invalid-upstream-secret").unwrap();
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
            protocol_config: Default::default(),
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
        let mut invalid_source = source.clone();
        invalid_source.id = "invalid_source".into();
        invalid_source.name = "Invalid".into();
        invalid_source.secret_ref = invalid_secret_ref.clone();
        invalid_source.models = vec!["invalid-model".into()];
        invalid_source.protocol_bindings = vec![zenith_relay_core::SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: zenith_relay_core::SourceAdapter::ResponsesToMessages,
            reasoning_mode: zenith_relay_core::MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["invalid-model".into()],
        }];
        state
            .store()
            .unwrap()
            .replace_records(vec![source, invalid_source], vec![key])
            .unwrap();
        let runtime = runtime_from_store(&state).await.unwrap();
        let address = state.gateway.start(runtime, 0).await.unwrap();
        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://{address}/v1/models"))
            .bearer_auth("old-secret")
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let models = response.json::<serde_json::Value>().await.unwrap();
        let model_ids = models["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|model| model["id"].as_str())
            .collect::<Vec<_>>();
        assert!(model_ids.contains(&"old-model"));
        assert!(!model_ids.contains(&"invalid-model"));
        drop(client);

        state.gateway.stop().await;
        secret_store::delete(&source_secret_ref).unwrap();
        secret_store::delete(&invalid_secret_ref).unwrap();
        secret_store::delete(&key_secret_ref).unwrap();
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
                protocol_config: Default::default(),
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
