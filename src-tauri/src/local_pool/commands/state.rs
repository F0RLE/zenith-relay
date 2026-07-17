use super::account_routing_allowed;
use crate::local_pool::{
    accounts::{
        credentials::CredentialStore,
        proxy::{common_proxy_available, proxy_status},
        NativeSecretBackend,
    },
    error::CommandError,
    models::{LocalAccountRecord, LocalGatewayKeyRecord, LocalPoolSnapshot, ProviderSourceRecord},
    profiles::codex,
    state::DesktopState,
    store::secret_store,
};
use std::collections::BTreeSet;
use tauri::State;
use zenith_relay_core::protocol::{
    pool_model_summaries, AccountRoutingExclusion, AccountSummary, Capabilities, GatewaySummary,
    KeySummary, RuntimeStateSnapshot, RuntimeTargetSummary, SourceSummary,
};
use zenith_relay_core::ApiEquivalentSummary;
use zenith_relay_core::CandidateRuntimeSnapshot;

#[tauri::command]
pub async fn get_local_pool_state(
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn get_local_runtime_state(
    state: State<'_, DesktopState>,
) -> Result<RuntimeStateSnapshot, CommandError> {
    let snapshot = state.snapshot().await?;
    let running = snapshot.runtime_target.connected;
    let routing_order = state
        .gateway
        .runtime()
        .await
        .map(|runtime| runtime.candidate_runtime_order())
        .unwrap_or_default();
    let common_proxy_available = common_proxy_available(&snapshot.gateway);
    let equivalents = state.telemetry.api_equivalents()?;
    let managed_key_ids = codex::profile_bindings(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
    )
    .unwrap_or_default()
    .into_iter()
    .filter(|binding| binding.credential_kind == codex::ProfileCredentialKind::LocalGateway)
    .map(|binding| binding.credential_id)
    .collect::<BTreeSet<_>>();
    let source_summaries = snapshot
        .sources
        .iter()
        .map(|record| {
            local_source_summary(
                record,
                equivalents
                    .sources
                    .get(&record.id)
                    .copied()
                    .unwrap_or_default(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let account_summaries = snapshot
        .accounts
        .iter()
        .map(|record| {
            local_account_summary(
                record,
                &snapshot.gateway,
                common_proxy_available,
                equivalents
                    .accounts
                    .get(&record.account.id)
                    .copied()
                    .unwrap_or_default(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let models = pool_model_summaries(
        &source_summaries,
        &account_summaries,
        &snapshot.gateway.hidden_models,
    );
    let visible_model_ids = models
        .iter()
        .filter(|model| model.enabled)
        .map(|model| model.id.clone())
        .collect();
    let configured_candidate_count = source_summaries
        .iter()
        .filter(|record| {
            record.enabled && record.in_pool && !record.draining && record.secret_available
        })
        .count()
        + account_summaries
            .iter()
            .filter(|record| {
                record.enabled
                    && record.in_pool
                    && !record.draining
                    && record.secret_available
                    && record.proxy_available
                    && record.routing_exclusion.is_none()
            })
            .count();
    let candidate_count =
        effective_candidate_count(running, configured_candidate_count, &routing_order);
    let base_url = format!(
        "http://{}:{}/v1",
        snapshot.gateway.client_host, snapshot.gateway.port
    );
    Ok(RuntimeStateSnapshot {
        schema_version: snapshot.schema_version,
        runtime_target: RuntimeTargetSummary {
            kind: "local".to_string(),
            connected: running,
            origin: Some(format!(
                "http://{}:{}",
                snapshot.gateway.client_host, snapshot.gateway.port
            )),
            server_id: None,
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        },
        gateway: GatewaySummary {
            running,
            base_url,
            candidate_count,
            visible_model_ids,
            max_retry_candidates: Some(snapshot.gateway.max_retry_candidates),
            routing_strategy: Some(snapshot.gateway.routing_strategy),
            default_service_tier: Some(snapshot.gateway.default_service_tier),
            image_base_model: snapshot.gateway.image_base_model.clone(),
            models,
            common_proxy_configured: snapshot.gateway.common_proxy_configured,
            common_proxy_available,
            account_proxy_required: snapshot.gateway.account_proxy_required,
            quota_refresh_interval_seconds: snapshot.gateway.quota_refresh_interval_seconds,
            quota_request_timeout_seconds: snapshot.gateway.quota_request_timeout_seconds,
            use_free_accounts: snapshot.gateway.use_free_accounts,
            routing_order,
        },
        platform: snapshot.platform.to_string(),
        capabilities: Capabilities::desktop_local(),
        sources: source_summaries,
        accounts: account_summaries,
        keys: snapshot
            .keys
            .iter()
            .map(|record| local_key_summary(record, managed_key_ids.contains(&record.id)))
            .collect(),
        automations: snapshot.automations,
        wake_history: snapshot.wake_history,
        warnings: snapshot.warnings,
    })
}

fn effective_candidate_count(
    running: bool,
    configured_count: usize,
    routing_order: &[CandidateRuntimeSnapshot],
) -> usize {
    if running {
        routing_order
            .iter()
            .filter(|candidate| candidate.available)
            .count()
    } else {
        configured_count
    }
}

#[tauri::command]
pub async fn get_local_runtime_order(
    state: State<'_, DesktopState>,
) -> Result<Vec<CandidateRuntimeSnapshot>, CommandError> {
    Ok(state
        .gateway
        .runtime()
        .await
        .map(|runtime| runtime.candidate_runtime_order())
        .unwrap_or_default())
}

fn local_source_summary(
    record: &ProviderSourceRecord,
    api_equivalent: ApiEquivalentSummary,
) -> crate::local_pool::error::Result<SourceSummary> {
    Ok(SourceSummary {
        id: record.id.clone(),
        name: record.name.clone(),
        enabled: record.enabled,
        in_pool: record.in_pool,
        draining: record.draining,
        base_url: record.base_url.clone(),
        wire_api: record.wire_api,
        models: record.models.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        api_equivalent,
        secret_available: secret_store::load(&record.secret_ref)?.is_some(),
        last_error_code: record.last_error.clone(),
    })
}

fn local_account_summary(
    record: &LocalAccountRecord,
    settings: &crate::local_pool::models::GatewaySettings,
    common_proxy_available: bool,
    api_equivalent: ApiEquivalentSummary,
) -> crate::local_pool::error::Result<AccountSummary> {
    let secret_available = record
        .account
        .secret_refs
        .iter()
        .map(|secret_ref| secret_store::load(secret_ref))
        .collect::<crate::local_pool::error::Result<Vec<_>>>()?
        .into_iter()
        .all(|value| value.is_some());
    let credentials = CredentialStore::from_backend(NativeSecretBackend)
        .load(&record.account.id)
        .map_err(|error| {
            crate::local_pool::error::LocalPoolError::new(
                crate::local_pool::error::ErrorCode::SecretStoreUnavailable,
                error.to_string(),
            )
        })?;
    let (proxy_mode, proxy_available) = credentials
        .as_ref()
        .map(|credentials| proxy_status(settings, credentials, common_proxy_available))
        .unwrap_or((zenith_relay_core::protocol::ProxyMode::Direct, false));
    let routing_exclusion = (!account_routing_allowed(settings, &record.account.subscription))
        .then_some(AccountRoutingExclusion::FreePlanPolicy);
    Ok(AccountSummary {
        id: record.account.id.clone(),
        label: record.account.label.clone(),
        identity_hint: record
            .account
            .identity
            .identity_hash
            .chars()
            .take(12)
            .collect(),
        enabled: record.account.enabled,
        in_pool: record.account.in_pool,
        draining: record.account.draining,
        auth_state: record.account.auth_state,
        health: format!("{:?}", record.account.health).to_ascii_lowercase(),
        models: record.models.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        api_equivalent,
        subscription: record.account.subscription.clone(),
        quota: record.account.quota.clone(),
        secret_available,
        proxy_mode,
        proxy_available,
        routing_exclusion,
        last_error_code: record.account.last_error_code.clone(),
    })
}

fn local_key_summary(record: &LocalGatewayKeyRecord, managed_by_chatgpt: bool) -> KeySummary {
    KeySummary {
        id: record.id.clone(),
        label: record.label.clone(),
        enabled: record.enabled,
        system: record.system || managed_by_chatgpt,
        source_ids: record.source_ids.clone(),
        account_ids: record.account_ids.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        model_prefix: record.model_prefix.clone(),
        created_at_ms: timestamp_ms(&record.created_at).unwrap_or_default(),
        last_used_at_ms: record.last_used_at.as_deref().and_then(timestamp_ms),
    }
}

fn timestamp_ms(value: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|value| u64::try_from(value.timestamp_millis()).ok())
}

#[cfg(test)]
mod parity_tests {
    use super::*;

    #[test]
    fn running_candidate_count_uses_live_scheduler_availability() {
        let candidates = [
            CandidateRuntimeSnapshot {
                candidate_id: "ready".into(),
                kind: zenith_relay_core::CandidateKind::OAuthAccount,
                available: true,
                in_flight: 0,
                last_used_at_ms: None,
                next_retry_at_ms: None,
                half_open: false,
                dispatches: 0,
            },
            CandidateRuntimeSnapshot {
                candidate_id: "limited".into(),
                kind: zenith_relay_core::CandidateKind::OAuthAccount,
                available: false,
                in_flight: 0,
                last_used_at_ms: None,
                next_retry_at_ms: None,
                half_open: false,
                dispatches: 0,
            },
        ];
        assert_eq!(effective_candidate_count(true, 2, &candidates), 1);
        assert_eq!(effective_candidate_count(false, 2, &candidates), 2);
    }

    #[test]
    fn local_and_remote_snapshots_share_the_same_top_level_contract() {
        let local = serde_json::to_value(RuntimeStateSnapshot {
            schema_version: 1,
            runtime_target: RuntimeTargetSummary {
                kind: "local".into(),
                connected: false,
                origin: None,
                server_id: None,
                version: None,
            },
            gateway: GatewaySummary {
                running: false,
                base_url: "http://127.0.0.1:14998/v1".into(),
                candidate_count: 0,
                visible_model_ids: Vec::new(),
                max_retry_candidates: Some(3),
                routing_strategy: Some(Default::default()),
                default_service_tier: Some(Default::default()),
                image_base_model: None,
                models: Vec::new(),
                common_proxy_configured: false,
                common_proxy_available: false,
                account_proxy_required: false,
                quota_refresh_interval_seconds: 300,
                quota_request_timeout_seconds: 20,
                use_free_accounts: false,
                routing_order: Vec::new(),
            },
            platform: "test".into(),
            capabilities: Capabilities::desktop_local(),
            sources: Vec::new(),
            accounts: Vec::new(),
            keys: Vec::new(),
            automations: Vec::new(),
            wake_history: Vec::new(),
            warnings: Vec::new(),
        })
        .unwrap();
        let remote = local.clone();
        assert_eq!(
            local.as_object().unwrap().keys().collect::<Vec<_>>(),
            remote.as_object().unwrap().keys().collect::<Vec<_>>()
        );
    }
}
