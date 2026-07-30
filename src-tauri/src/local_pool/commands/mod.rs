pub(crate) mod automations;
pub(crate) mod connections;
pub(crate) mod gateway;
pub(crate) mod oauth;
pub(crate) mod pool;
pub(crate) mod profiles;
pub(crate) mod proxies;
pub(crate) mod recovery;
pub(crate) mod remote_server;
pub(crate) mod state;
pub(crate) mod usage;

use super::{
    accounts::{
        authority::{CredentialPersistence, StoredRefreshAdapter},
        credentials::CredentialStore,
        proxy::{effective_proxy_config, ProxyRefreshClient},
        records::{candidate_health, candidate_quota_with_stale_after, CODEX_RESPONSES_URL},
        NativeSecretBackend,
    },
    error::{ErrorCode, LocalPoolError, Result},
    models::{GatewaySettings, LocalGatewayKeyRecord, ProviderSourceRecord},
    profiles::codex,
    state::DesktopState,
    store::secret_store,
};
use std::{collections::HashMap, sync::Arc};
use zenith_relay_core::{
    protocol::{account_operational_state, AccountOperationalInput},
    GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeChatGptAccount,
    RuntimeChatGptAuth, RuntimeMixedLocalKey, RuntimeSource, QUOTA_STALE_AFTER_MS,
};

async fn runtime_from_store(state: &DesktopState) -> Result<Arc<GatewayRuntime>> {
    pool::ensure_system_gateway_key(state)?;
    let codex_home = crate::platform::default_codex_home();
    let protected_account_id = if codex::credential_kind(&codex_home, &state.profile_backup_root())?
        == Some(codex::ProfileCredentialKind::LocalGateway)
    {
        codex::active_managed_account_id(&codex_home, &state.profile_backup_root())?
    } else {
        None
    };
    let (source_records, account_records, key_records, settings) = {
        let store = state.store()?;
        (
            store.sources().to_vec(),
            store.accounts().to_vec(),
            store.keys().to_vec(),
            store.gateway().clone(),
        )
    };
    let quota_stale_after_ms = QUOTA_STALE_AFTER_MS;
    let mut sources = Vec::new();
    for source in source_records {
        let Some(api_key) = secret_store::load(&source.secret_ref)? else {
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
            enabled: source.enabled && source.in_pool,
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
        let Some(secret) = credentials
            .load(&account_id)
            .map_err(account_credential_error)?
        else {
            continue;
        };
        let Some(chatgpt_account_id) = secret.provider_account_id() else {
            continue;
        };
        let Ok(proxy) = effective_proxy_config(&settings, &secret) else {
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
                .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
        }
        let operational = account_operational_state(AccountOperationalInput {
            enabled: account.account.enabled,
            in_pool: account.account.in_pool,
            draining: account.account.draining,
            secret_available: true,
            proxy_available: true,
            auth_state: account.account.auth_state,
            health: account.account.health,
            subscription: &account.account.subscription,
            quota: &account.account.quota,
            last_error_code: account.account.last_error_code.as_deref(),
            now_ms: current_time_ms(),
            quota_stale_after_ms,
        });
        accounts.push(RuntimeChatGptAccount {
            id: account_id.clone(),
            source_id: account.account.source_id,
            chatgpt_account_id: chatgpt_account_id.to_string(),
            responses_url: CODEX_RESPONSES_URL.to_string(),
            models: account.models,
            enabled: operational.routing_eligible,
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
    let mut keys = Vec::new();
    for key in key_records {
        let secret = if key.enabled {
            pool::ensure_local_gateway_key_secret(&key)?
        } else if let Some(secret) = secret_store::load(&key.secret_ref)? {
            secret
        } else {
            continue;
        };
        keys.push(RuntimeMixedLocalKey {
            key: LocalGatewayKey { id: key.id, secret },
            enabled: key.enabled,
            source_ids: key.source_ids,
            account_ids: key.account_ids,
            allowed_models: key.allowed_models,
            excluded_models: key.excluded_models,
            model_prefix: key.model_prefix,
            wire_apis: None,
        });
    }
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
    let runtime = GatewayRuntime::from_mixed_pool(
        sources,
        accounts,
        keys,
        RuntimeChatGptAuth {
            token_authority: authority,
            refresh_adapter: refresh,
            persistence_adapter: persistence,
            refresh_skew_ms: 60_000,
            agent_identities,
        },
        GatewayRuntimeOptions {
            max_retry_candidates: usize::from(settings.max_retry_candidates),
            routing_strategy: settings.routing_strategy,
            subscription_plan_order: settings.subscription_plan_order,
            hidden_models: settings.hidden_models,
            default_service_tier: settings.default_service_tier,
            quota_stale_after_ms,
            image_base_model: None,
            response_affinity_store: Some(state.response_affinity_store()),
            provider_storm_breaker: false,
        },
        state.usage_callback(),
    )
    .map_err(core_error)?;
    runtime.set_protected_candidate(
        protected_account_id.as_deref(),
        settings.chatgpt_interface_quota_reserve_basis_points,
    );
    Ok(Arc::new(runtime))
}

fn timestamp_ms(value: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|value| u64::try_from(value.timestamp_millis()).ok())
}

pub(super) fn current_time_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or_default()
}

fn account_credential_error(
    error: super::accounts::credentials::CredentialError,
) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::InvalidState, error.to_string())
}

pub(super) async fn sync_records_or_rollback(
    state: &DesktopState,
    old_sources: Vec<ProviderSourceRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
) -> Result<()> {
    restart_or_rollback(state, || {
        state.store()?.replace_records(old_sources, old_keys)
    })
    .await
}

pub(super) async fn sync_accounts_or_rollback(
    state: &DesktopState,
    old_accounts: Vec<super::models::LocalAccountRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
) -> Result<()> {
    restart_or_rollback(state, || {
        state
            .store()?
            .replace_accounts_and_keys(old_accounts, old_keys)
    })
    .await
}

pub(super) async fn sync_refreshed_account_or_rollback(
    state: &DesktopState,
    account_id: &str,
    models_changed: bool,
    old_accounts: Vec<super::models::LocalAccountRecord>,
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
        (
            account.account.enabled && account.account.in_pool,
            candidate_health(&account.account),
            candidate_quota_with_stale_after(
                &account.account.quota,
                current_time_ms(),
                QUOTA_STALE_AFTER_MS,
            ),
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

async fn sync_gateway_or_rollback(
    state: &DesktopState,
    old_gateway: GatewaySettings,
) -> Result<()> {
    restart_or_rollback(state, || state.store()?.replace_gateway(old_gateway)).await
}

async fn restart_after_secret_change(
    state: &DesktopState,
    secret_ref: &str,
    old_secret: &str,
) -> Result<()> {
    restart_or_rollback(state, || secret_store::save(secret_ref, old_secret)).await
}

async fn restart_or_rollback(
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
    if let Err(error) = state.gateway.start(runtime, next_port).await {
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

async fn fail_closed(state: &DesktopState, message: String) -> LocalPoolError {
    state.gateway.stop().await;
    match disable_gateway(state) {
        Ok(()) => LocalPoolError::new(ErrorCode::RecoveryRequired, message),
        Err(error) => LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("{message}; failed to disable gateway state: {error}"),
        ),
    }
}

fn core_error(error: zenith_relay_core::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::InvalidState, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn persisted_last_used_timestamp_maps_to_epoch_milliseconds() {
        assert_eq!(timestamp_ms("1970-01-01T00:00:00.001Z"), Some(1));
        assert_eq!(timestamp_ms("not-a-date"), None);
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
                wire_api: zenith_relay_core::WireApi::Responses,
                models: vec!["gpt-test".into()],
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                priority: 0,
                weight: 1,
                recovery_delay_seconds: 0,
                model_price_overrides: Default::default(),
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
                wire_api: zenith_relay_core::WireApi::Responses,
                models: vec!["gpt-test".into()],
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                priority: 0,
                weight: 1,
                recovery_delay_seconds: 0,
                model_price_overrides: Default::default(),
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
                system: false,
                secret_ref: key_secret_ref.clone(),
                source_ids: None,
                account_ids: None,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                model_prefix: None,
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
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-command-rollback-{}",
            uuid::Uuid::new_v4()
        ));
        let state = DesktopState::open(root.clone()).unwrap();
        let runtime = Arc::new(
            GatewayRuntime::new(
                ProviderSource {
                    id: "old_source".into(),
                    name: "Old".into(),
                    base_url: "http://127.0.0.1:9/v1".into(),
                    api_key: "upstream".into(),
                    wire_api: zenith_relay_core::WireApi::Responses,
                    models: vec!["old-model".into()],
                },
                LocalGatewayKey {
                    id: "old_key".into(),
                    secret: "old-secret".into(),
                },
                Arc::new(|_| {}),
            )
            .unwrap(),
        );
        let address = state.gateway.start(runtime, 0).await.unwrap();
        let rollback_calls = Arc::new(AtomicUsize::new(0));
        let observed = rollback_calls.clone();

        assert!(restart_or_rollback(&state, move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
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
        assert!(response.status().is_success());
        assert!(response.text().await.unwrap().contains("old-model"));

        state.gateway.stop().await;
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
                wire_api: zenith_relay_core::WireApi::Responses,
                models: vec!["gpt-test".into()],
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                priority: 0,
                weight: 1,
                recovery_delay_seconds: 0,
                model_price_overrides: Default::default(),
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
                system: false,
                secret_ref: key_secret_ref.clone(),
                source_ids: None,
                account_ids: None,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                model_prefix: None,
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
