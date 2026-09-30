use super::super::super::{
    accounts::{
        authority::{CredentialPersistence, StoredRefreshAdapter},
        credentials::{
            credential_invalid_state_error as account_credential_error, CredentialStore,
            StoredCodexCredentials,
        },
        proxy::{effective_proxy_config, ProxyRefreshClient},
        records::CODEX_RESPONSES_URL,
        NativeSecretBackend,
    },
    error::{ErrorCode, LocalPoolError, Result},
    models::{GatewaySettings, LocalAccountRecord, ProviderSourceRecord},
    profiles::codex,
    state::{DesktopState, LocalRuntimeInputs},
};
use super::super::pool;
use super::sync::core_error;
use super::{current_time_ms, pending_move_account_ids, runtime_account_operational_state};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};
use zenith_relay_core::error_codes;
use zenith_relay_core::{
    accounts::TokenAuthority, protocol::account_candidate_enabled,
    providers::chatgpt::AgentIdentityCredential, GatewayRuntime, GatewayRuntimeOptions,
    LocalGatewayKey, ProviderSource, ProxyConfig, RuntimeChatGptAccount, RuntimeChatGptAuth,
    RuntimeMixedLocalKey, RuntimeSource, SourceTransportRecord, QUOTA_STALE_AFTER_MS,
};

/// A malformed source record must not make an otherwise usable local pool
/// disappear. Keep the source in the inventory, exclude only its runtime
/// route, and expose stable codes to the UI so it can be repaired.
const SOURCE_PROTOCOL_INVALID_CODE: &str = error_codes::SOURCE_PROTOCOL_INVALID;
const SOURCE_RUNTIME_INVALID_CODE: &str = error_codes::SOURCE_RUNTIME_INVALID;

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
    let pending_move_ids = pending_move_account_ids(state)?;
    let pool_routing = settings.pool_routing_for(&source_records, &account_records);
    let quota_stale_after_ms = QUOTA_STALE_AFTER_MS;
    // The managed profile can expose every verified source protocol. Requests
    // still select only the protocol they actually use at the gateway edge.
    let (mut pool_source_ids, mut pool_account_ids) =
        pool::local_pool_member_ids(&source_records, &account_records)?;
    pool_account_ids.retain(|id| !pending_move_ids.contains(id));
    let (sources, source_ids) = admit_runtime_sources(state, source_records, &source_api_keys);
    // Key scopes must reference only source executors admitted above. Keeping
    // a stale id for a malformed or credential-less source can make the core
    // reject an otherwise valid mixed pool while rebuilding the gateway.
    pool_source_ids.retain(|id| source_ids.contains(id));
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let authority = state.token_authority();
    let AdmittedAccounts {
        accounts,
        refresh_proxies,
        agent_identities,
    } = admit_runtime_accounts(
        authority.as_ref(),
        &settings,
        account_records,
        &account_credentials,
        &pending_move_ids,
    )
    .await?;
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
        wire_apis: Some(zenith_relay_core::protocol::local_gateway_client_wire_apis()),
    }];
    let oauth = Arc::new(ProxyRefreshClient::new(refresh_proxies)?);
    let refresh = Arc::new(
        StoredRefreshAdapter::new(
            state.transient_root(),
            credentials.clone(),
            oauth,
            zenith_relay_core::accounts::TOKEN_REFRESH_SKEW_MS,
        )
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
        state.transient_root(),
    ));
    let auth = RuntimeChatGptAuth {
        token_authority: authority,
        refresh_adapter: refresh,
        persistence_adapter: persistence,
        refresh_skew_ms: zenith_relay_core::accounts::TOKEN_REFRESH_SKEW_MS,
        agent_identities,
    };
    let options = GatewayRuntimeOptions {
        tool_policy: settings.tool_policy,
        max_retry_candidates: usize::from(settings.max_retry_candidates),
        pool_routing: Some(pool_routing),
        hidden_models: settings.hidden_models,
        default_service_tier: settings.default_service_tier,
        quota_stale_after_ms,
        image_base_model: None,
        image_pricing_catalog: Some(state.pricing_catalog()),
        model_metadata_catalog: Some(state.model_metadata_loader().catalog_handle()),
        model_reasoning_allowed_levels: settings.model_reasoning_allowed_levels,
        response_affinity_store: Some(state.response_affinity_store()),
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
    runtime.set_route_recovery_enabled(settings.chatgpt_retry_until_available);
    runtime
        .set_model_service_tier_overrides(settings.model_service_tier_overrides)
        .map_err(core_error)?;
    runtime.set_model_display_order(settings.model_display_order);
    runtime.set_official_codex_ultra_models(
        crate::local_pool::profiles::codex::official_codex_ultra_models()
            .into_iter()
            .collect(),
    );
    runtime.set_protected_candidate(
        protected_account_id.as_deref(),
        settings.chatgpt_interface_quota_reserve_basis_points,
    );
    crate::diagnostics::breadcrumb("gateway-runtime", "build_completed", &[]);
    Ok(Arc::new(runtime))
}

fn admit_runtime_sources(
    state: &DesktopState,
    source_records: Vec<ProviderSourceRecord>,
    source_api_keys: &BTreeMap<String, Option<String>>,
) -> (Vec<RuntimeSource>, HashSet<String>) {
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
            || source.recovery_delay_seconds > zenith_relay_core::MAX_SOURCE_RECOVERY_DELAY_SECONDS
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
    (sources, source_ids)
}

struct AdmittedAccounts {
    accounts: Vec<RuntimeChatGptAccount>,
    refresh_proxies: Vec<(String, Option<ProxyConfig>)>,
    agent_identities: HashMap<String, AgentIdentityCredential>,
}

struct AdmittedRuntimeAccount {
    account: RuntimeChatGptAccount,
    refresh_proxy: Option<(String, Option<ProxyConfig>)>,
    agent_identity: Option<(String, AgentIdentityCredential)>,
}

async fn admit_runtime_accounts(
    authority: &TokenAuthority,
    settings: &GatewaySettings,
    account_records: Vec<LocalAccountRecord>,
    account_credentials: &HashMap<String, Option<StoredCodexCredentials>>,
    pending_move_ids: &HashSet<String>,
) -> Result<AdmittedAccounts> {
    let mut accounts = Vec::new();
    let mut refresh_proxies = Vec::new();
    let mut agent_identities = HashMap::new();
    for account in account_records {
        let Some(secret) = account_credentials
            .get(&account.account.id)
            .and_then(Option::as_ref)
        else {
            continue;
        };
        let Some(admitted) =
            admit_runtime_account(authority, settings, account, secret, pending_move_ids).await?
        else {
            continue;
        };
        if let Some((account_id, agent)) = admitted.agent_identity {
            agent_identities.insert(account_id, agent);
        }
        accounts.push(admitted.account);
        if let Some(refresh_proxy) = admitted.refresh_proxy {
            refresh_proxies.push(refresh_proxy);
        }
    }
    Ok(AdmittedAccounts {
        accounts,
        refresh_proxies,
        agent_identities,
    })
}

async fn admit_runtime_account(
    authority: &TokenAuthority,
    settings: &GatewaySettings,
    account: LocalAccountRecord,
    secret: &StoredCodexCredentials,
    pending_move_ids: &HashSet<String>,
) -> Result<Option<AdmittedRuntimeAccount>> {
    let Some(chatgpt_account_id) = secret.provider_account_id() else {
        return Ok(None);
    };
    let Ok(proxy) = effective_proxy_config(settings, secret) else {
        return Ok(None);
    };
    let account_id = account.account.id.clone();
    let agent_identity = secret
        .agent_identity()
        .map(|agent| (account_id.clone(), agent.clone()));
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
    // Candidate `enabled` represents base configuration availability.
    // Quota remains a separate scheduler decision for every request and
    // model-list response. Do not fold a temporary exhausted quota into
    // this flag: doing so makes a healthy pool look structurally invalid
    // until a later refresh happens to repair it.
    let candidate_enabled = account_candidate_enabled(
        account.account.enabled
            && account.remote_location.is_none()
            && !pending_move_ids.contains(&account_id),
        operational.routing_block_reason,
    );
    let refresh_proxy = secret
        .has_oauth()
        .then(|| (account_id.clone(), proxy.clone()));
    let models = account.effective_models().to_vec();
    Ok(Some(AdmittedRuntimeAccount {
        account: RuntimeChatGptAccount {
            id: account_id,
            source_id: account.account.source_id,
            chatgpt_account_id: chatgpt_account_id.to_string(),
            responses_url: CODEX_RESPONSES_URL.to_string(),
            basis_points_enabled: settings.basis_points_enabled
                && secret.has_oauth()
                && !secret.is_agent_identity(),
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
            proxy,
        },
        refresh_proxy,
        agent_identity,
    }))
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
    // Transport identity includes the catalog. A catalog-only edit must not
    // receive an admission error captured for the previous evidence.
    current.transport_identity() == expected.transport_identity()
        && current.name == expected.name
        && current.enabled == expected.enabled
        && current.in_pool == expected.in_pool
        && current.draining == expected.draining
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
pub(super) fn managed_chatgpt_account_id_for_reserve(
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

pub(super) fn timestamp_ms(value: &str) -> Option<u64> {
    zenith_relay_core::unix_time_ms_from_rfc3339(value)
}

#[cfg(test)]
mod tests {
    use super::same_source_runtime_configuration;
    use crate::local_pool::models::ProviderSourceRecord;
    use std::collections::BTreeMap;
    use zenith_relay_core::WireApi;

    fn source() -> ProviderSourceRecord {
        ProviderSourceRecord {
            id: "source".into(),
            name: "Provider".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "https://provider.test/v1".into(),
            secret_ref: "source:test".into(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_config: Default::default(),
            protocol_bindings: Vec::new(),
            models: vec!["model-a".into()],
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
    fn catalog_evidence_changes_the_admitted_source_configuration() {
        let current = source();
        let mut catalog = current.clone();
        catalog.protocol_config.revision = 1;

        assert!(same_source_runtime_configuration(&current, &current));
        assert!(!same_source_runtime_configuration(&current, &catalog));
    }
}
