pub(crate) mod accounts;
pub(crate) mod automations;
pub(crate) mod connections;
pub(crate) mod gateway;
pub(crate) mod oauth;
pub(crate) mod pool;
pub(crate) mod profiles;
pub(crate) mod state;
pub(crate) mod usage;

use super::{
    accounts::{
        authority::{CredentialPersistence, StoredRefreshAdapter},
        credentials::CredentialStore,
        oauth::CodexOAuthClient,
        records::{candidate_health, candidate_quota, CODEX_RESPONSES_URL},
        NativeSecretBackend,
    },
    error::{ErrorCode, LocalPoolError, Result},
    models::{GatewaySettings, LocalGatewayKeyRecord, ProviderSourceRecord},
    state::DesktopState,
    store::secret_store,
};
use std::{sync::Arc, time::Duration};
use zenith_relay_core::{
    GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeAccount,
    RuntimeAccountAuth, RuntimeMixedLocalKey, RuntimeSource,
};

async fn runtime_from_store(state: &DesktopState) -> Result<Arc<GatewayRuntime>> {
    let (source_records, account_records, key_records, settings) = {
        let store = state.store()?;
        (
            store.sources().to_vec(),
            store.accounts().to_vec(),
            store.keys().to_vec(),
            store.gateway().clone(),
        )
    };
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
            enabled: source.enabled,
            draining: source.draining,
            priority: source.priority,
            weight: source.weight,
            allowed_models: source.allowed_models,
            excluded_models: source.excluded_models,
            last_used_at_ms: source.last_used_at.as_deref().and_then(timestamp_ms),
        });
    }
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let authority = state.token_authority();
    let mut accounts = Vec::new();
    for account in account_records {
        let Some(secret) = credentials
            .load(&account.account.id)
            .map_err(account_credential_error)?
        else {
            continue;
        };
        let Some(chatgpt_account_id) = secret.provider_account_id() else {
            continue;
        };
        authority
            .register(
                &account.account.id,
                secret.to_token_set().map_err(account_credential_error)?,
                account.account.auth_state,
            )
            .await
            .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
        let health = candidate_health(&account.account);
        let quota = candidate_quota(&account.account.quota, current_time_ms());
        accounts.push(RuntimeAccount {
            id: account.account.id,
            source_id: account.account.source_id,
            chatgpt_account_id: chatgpt_account_id.to_string(),
            responses_url: CODEX_RESPONSES_URL.to_string(),
            models: account.models,
            enabled: account.account.enabled,
            draining: account.account.draining,
            priority: account.priority,
            weight: account.weight,
            allowed_models: account.allowed_models,
            excluded_models: account.excluded_models,
            health,
            quota,
            last_used_at_ms: account.account.last_used_at_ms,
            cooldowns: account.cooldowns,
            consecutive_failures: account.consecutive_failures,
        });
    }
    let mut keys = Vec::new();
    for key in key_records {
        let Some(secret) = secret_store::load(&key.secret_ref)? else {
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
        });
    }
    let oauth = Arc::new(
        CodexOAuthClient::new()
            .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?,
    );
    let refresh = Arc::new(
        StoredRefreshAdapter::new(state.root.clone(), credentials.clone(), oauth, 60_000).map_err(
            |error| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    format!("failed to initialize account refresh locks: {error:?}"),
                )
            },
        )?,
    );
    let persistence = Arc::new(CredentialPersistence::new(
        credentials,
        state.account_metadata_sink(),
    ));
    GatewayRuntime::from_mixed_pool(
        sources,
        accounts,
        keys,
        RuntimeAccountAuth {
            token_authority: authority,
            refresh_adapter: refresh,
            persistence_adapter: persistence,
            refresh_skew_ms: 60_000,
        },
        GatewayRuntimeOptions {
            max_retry_candidates: usize::from(settings.max_retry_candidates),
            session_affinity_ttl: settings
                .session_affinity
                .then(|| Duration::from_secs(settings.session_affinity_ttl_seconds)),
            max_affinity_entries: 4_096,
        },
        state.usage_callback(),
    )
    .map(Arc::new)
    .map_err(core_error)
}

fn timestamp_ms(value: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|value| u64::try_from(value.timestamp_millis()).ok())
}

fn current_time_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or_default()
}

fn account_credential_error(
    error: super::accounts::credentials::CredentialError,
) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::InvalidState, error.to_string())
}

async fn sync_records_or_rollback(
    state: &DesktopState,
    old_sources: Vec<ProviderSourceRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
) -> Result<()> {
    restart_or_rollback(state, || {
        state.store()?.replace_records(old_sources, old_keys)
    })
    .await
}

async fn sync_accounts_or_rollback(
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
    let mut rollback = Some(rollback);
    let runtime = match runtime_from_store(state).await {
        Ok(runtime) => runtime,
        Err(error) => {
            apply_rollback(state, rollback.take().unwrap()).await?;
            return Err(error);
        }
    };

    state.gateway.stop().await;
    if let Err(error) = state.gateway.start(runtime, address.port()).await {
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
    fn persisted_lru_timestamp_maps_to_epoch_milliseconds() {
        assert_eq!(timestamp_ms("1970-01-01T00:00:00.001Z"), Some(1));
        assert_eq!(timestamp_ms("not-a-date"), None);
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
}
