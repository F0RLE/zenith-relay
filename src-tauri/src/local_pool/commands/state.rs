use super::account_routing_allowed;
use crate::local_pool::{
    accounts::{
        credentials::CredentialStore,
        proxy::{common_proxy_available, proxy_status},
        records::candidate_health,
        NativeSecretBackend,
    },
    error::CommandError,
    models::{LocalAccountRecord, LocalGatewayKeyRecord, LocalPoolSnapshot, ProviderSourceRecord},
    profiles::codex,
    state::DesktopState,
    store::secret_store,
};
use std::collections::{BTreeSet, HashMap};
use tauri::State;
use zenith_relay_core::protocol::{
    operational_status, pool_model_summaries, AccountRoutingExclusion, AccountSummary,
    Capabilities, GatewaySummary, KeySummary, OperationalStatus, RuntimeStateSnapshot,
    RuntimeTargetSummary, SourceSummary,
};
use zenith_relay_core::{ApiEquivalentSummary, CandidateKind, CandidateRuntimeSnapshot};

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
    let source_runtime = routing_order
        .iter()
        .filter(|candidate| candidate.kind == CandidateKind::ApiSource)
        .map(|candidate| (candidate.candidate_id.as_str(), candidate.available))
        .collect::<HashMap<_, _>>();
    let common_proxy_available = common_proxy_available(&snapshot.gateway);
    let equivalents = state
        .telemetry
        .api_equivalents_with_price_overrides(&snapshot.gateway.model_price_overrides)?;
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
                (running && record.in_pool).then(|| {
                    source_runtime
                        .get(record.id.as_str())
                        .copied()
                        .unwrap_or(false)
                }),
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
    let mut models = pool_model_summaries(
        &source_summaries,
        &account_summaries,
        &snapshot.gateway.hidden_models,
    );
    for model in &mut models {
        if let Some(price) = snapshot
            .gateway
            .model_price_overrides
            .get(&model.id.to_ascii_lowercase())
        {
            model.input_micro_usd_per_million = Some(price.input_micro_usd_per_million);
            model.cached_input_micro_usd_per_million = Some(
                price
                    .cached_input_micro_usd_per_million
                    .unwrap_or(price.input_micro_usd_per_million),
            );
            model.output_micro_usd_per_million = Some(price.output_micro_usd_per_million);
            model.custom_price = true;
        }
    }
    let visible_model_ids = models
        .iter()
        .filter(|model| model.enabled)
        .map(|model| model.id.clone())
        .collect();
    let candidate_count = source_summaries
        .iter()
        .filter(|record| record.in_pool && record.operational_status == OperationalStatus::Rotation)
        .count()
        + account_summaries
            .iter()
            .filter(|record| {
                record.in_pool && record.operational_status == OperationalStatus::Rotation
            })
            .count();
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
            max_retry_candidates: snapshot.gateway.max_retry_candidates,
            routing_strategy: snapshot.gateway.routing_strategy,
            subscription_plan_order: snapshot.gateway.subscription_plan_order.clone(),
            default_service_tier: snapshot.gateway.default_service_tier,
            image_base_model: snapshot.gateway.image_base_model.clone(),
            models,
            common_proxy_configured: snapshot.gateway.common_proxy_configured,
            common_proxy_available,
            account_proxy_required: snapshot.gateway.account_proxy_required,
            quota_refresh_interval_seconds: snapshot.gateway.quota_refresh_interval_seconds,
            quota_request_timeout_seconds: snapshot.gateway.quota_request_timeout_seconds,
            use_free_accounts: snapshot.gateway.use_free_accounts,
            chatgpt_interface_quota_reserve_basis_points: Some(
                snapshot
                    .gateway
                    .chatgpt_interface_quota_reserve_basis_points,
            ),
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
    runtime_available: Option<bool>,
    api_equivalent: ApiEquivalentSummary,
) -> crate::local_pool::error::Result<SourceSummary> {
    let secret_available = secret_store::load(&record.secret_ref)?.is_some();
    Ok(SourceSummary {
        id: record.id.clone(),
        name: record.name.clone(),
        enabled: record.enabled,
        in_pool: record.in_pool,
        draining: record.draining,
        operational_status: operational_status(
            record.enabled,
            false,
            !record.draining && secret_available,
            runtime_available,
        ),
        base_url: record.base_url.clone(),
        wire_api: record.wire_api,
        models: record.models.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        api_equivalent,
        secret_available,
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
    let quota_wait = routing_exclusion.is_some()
        || record.account.quota.limit_reached
        || record
            .account
            .quota
            .primary
            .iter()
            .chain(record.account.quota.secondary.iter())
            .any(|window| window.available_basis_points == Some(0));
    let configured_available = !record.account.draining
        && secret_available
        && proxy_available
        && routing_exclusion.is_none()
        && candidate_health(&record.account).is_eligible();
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
        operational_status: operational_status(
            record.account.enabled,
            quota_wait,
            configured_available,
            None,
        ),
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
        remote_location: record.remote_location.clone(),
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
                max_retry_candidates: 3,
                routing_strategy: Default::default(),
                subscription_plan_order: Vec::new(),
                default_service_tier: Default::default(),
                image_base_model: None,
                models: Vec::new(),
                common_proxy_configured: false,
                common_proxy_available: false,
                account_proxy_required: false,
                quota_refresh_interval_seconds: 300,
                quota_request_timeout_seconds: 20,
                use_free_accounts: false,
                chatgpt_interface_quota_reserve_basis_points: Some(100),
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
