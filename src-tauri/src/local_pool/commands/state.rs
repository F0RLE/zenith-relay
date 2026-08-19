use crate::local_pool::{
    accounts::{
        credentials::CredentialStore,
        proxy::{common_proxy_available, effective_proxy_config, proxy_status},
        NativeSecretBackend,
    },
    error::CommandError,
    models::{LocalAccountRecord, LocalPoolSnapshot, ProviderSourceRecord},
    state::DesktopState,
    store::secret_store,
};
use std::collections::BTreeMap;
use std::time::Instant;
use tauri::State;
use zenith_relay_core::protocol::{
    account_operational_state, apply_model_reasoning_summary, model_has_native_account_route,
    operational_status, pool_model_summaries, source_runtime_available, AccountOperationalInput,
    AccountSummary, Capabilities, GatewaySummary, OperationalStatus, RuntimeStateSnapshot,
    RuntimeTargetSummary, SourceSummary,
};
use zenith_relay_core::{
    unix_time_ms, ApiEquivalentSummary, CandidateKind, CandidateRuntimeSnapshot, WireApi,
    QUOTA_STALE_AFTER_MS,
};

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
    let started = Instant::now();
    let snapshot = state.snapshot().await?;
    let running = snapshot.runtime_target.connected;
    let runtime = state.gateway.runtime().await;
    if let Some(runtime) = runtime.as_ref() {
        runtime.prefetch_source_model_metadata();
    }
    let routing_order = runtime
        .as_ref()
        .map(|runtime| runtime.candidate_runtime_order())
        .unwrap_or_default();
    let common_proxy_available = common_proxy_available(&snapshot.gateway);
    let snapshot_at_ms = unix_time_ms();
    let equivalents = state.telemetry.api_equivalents_with_price_overrides(
        &snapshot.gateway.model_price_overrides,
        &snapshot
            .sources
            .iter()
            .map(|source| {
                let mut prices = BTreeMap::new();
                for model in source
                    .model_price_overrides
                    .keys()
                    .chain(source.detected_model_prices.keys())
                {
                    prices.entry(model.clone()).or_insert_with(|| {
                        zenith_relay_core::ApiModelPriceSources {
                            provider: source.detected_model_prices.get(model).copied(),
                            manual: source.model_price_overrides.get(model).copied(),
                        }
                    });
                }
                (source.id.clone(), prices)
            })
            .collect::<BTreeMap<_, _>>(),
    )?;
    let source_summaries = snapshot
        .sources
        .iter()
        .map(|record| {
            local_source_summary(
                record,
                (running && record.enabled)
                    .then(|| source_runtime_available(&routing_order, &record.id)),
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
                snapshot_at_ms,
                state.quota_refresh_in_flight(&record.account.id)?,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut warnings = snapshot.warnings;
    if running {
        for record in &snapshot.accounts {
            if record.account.enabled
                && record.account.in_pool
                && !record.account.draining
                && oauth_account_runtime_available(&routing_order, &record.account.id).is_none()
            {
                warnings.push(account_runtime_warning(
                    record,
                    &snapshot.gateway,
                    &record.account.id,
                ));
            }
        }
    }
    let mut models = pool_model_summaries(
        &source_summaries,
        &account_summaries,
        &snapshot.gateway.hidden_models,
    );
    for model in &mut models {
        let model_id = model.id.clone();
        if let Some(price) = snapshot
            .gateway
            .model_price_overrides
            .get(&model_id.to_ascii_lowercase())
        {
            model.input_micro_usd_per_million = Some(price.input_micro_usd_per_million);
            model.cached_input_micro_usd_per_million = Some(
                price
                    .cached_input_micro_usd_per_million
                    .unwrap_or(price.input_micro_usd_per_million),
            );
            model.cache_write_5m_micro_usd_per_million = price.cache_write_5m_micro_usd_per_million;
            model.cache_write_1h_micro_usd_per_million = price.cache_write_1h_micro_usd_per_million;
            model.output_micro_usd_per_million = Some(price.output_micro_usd_per_million);
            model.custom_price = true;
        }
        apply_model_reasoning_summary(
            model,
            runtime
                .as_ref()
                .map(|runtime| runtime.confirmed_source_reasoning_levels(&model_id))
                .unwrap_or_default(),
            snapshot
                .gateway
                .model_reasoning_allowed_levels
                .get(&model_id.to_ascii_lowercase())
                .map(Vec::as_slice),
            model_has_native_account_route(&account_summaries, &model_id),
        );
        model.reasoning_probe = runtime
            .as_ref()
            .and_then(|runtime| runtime.reasoning_probe_progress(&model_id));
    }
    let visible_model_ids = models
        .iter()
        .filter(|model| model.enabled)
        .map(|model| model.id.clone())
        .collect();
    let candidate_count = source_summaries
        .iter()
        .filter(|record| {
            record.in_pool
                && record.supports_wire_api(WireApi::Responses)
                && record.operational_status == OperationalStatus::Rotation
        })
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
    let response = RuntimeStateSnapshot {
        schema_version: snapshot.schema_version,
        configuration_revision: None,
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
            cooldown_after_failures: snapshot.gateway.cooldown_after_failures,
            keep_last_candidate_available: snapshot.gateway.keep_last_candidate_available,
            routing_strategy: snapshot.gateway.routing_strategy,
            subscription_plan_order: snapshot.gateway.subscription_plan_order.clone(),
            default_service_tier: snapshot.gateway.default_service_tier,
            image_base_model: snapshot.gateway.image_base_model.clone(),
            models,
            common_proxy_configured: snapshot.gateway.common_proxy_configured,
            common_proxy_available,
            common_proxy_id: None,
            account_proxy_required: snapshot.gateway.account_proxy_required,
            quota_request_timeout_seconds: snapshot.gateway.quota_request_timeout_seconds,
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
        automations: snapshot.automations,
        wake_history: snapshot.wake_history,
        warnings,
    };
    let _ = state.record_performance(
        "full_snapshot_native",
        started.elapsed().as_secs_f64() * 1_000.0,
        Some("local"),
    );
    Ok(response)
}

#[tauri::command]
pub fn record_local_performance_sample(
    name: String,
    duration_ms: f64,
    context: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    state
        .record_performance(&name, duration_ms, context.as_deref())
        .map_err(Into::into)
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
        protocol_bindings: record.protocol_bindings.clone(),
        models: record.models.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        recovery_delay_seconds: record.recovery_delay_seconds,
        model_price_overrides: record.model_price_overrides.clone(),
        detected_model_prices: record.detected_model_prices.clone(),
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
    now_ms: u64,
    refreshing: bool,
) -> crate::local_pool::error::Result<AccountSummary> {
    let credentials = CredentialStore::from_backend(NativeSecretBackend)
        .load(&record.account.id)
        .map_err(|error| {
            crate::local_pool::error::LocalPoolError::new(
                crate::local_pool::error::ErrorCode::SecretStoreUnavailable,
                error.to_string(),
            )
        })?;
    let secret_available = credentials.is_some();
    let (proxy_mode, proxy_available) = credentials
        .as_ref()
        .map(|credentials| proxy_status(settings, credentials, common_proxy_available))
        .unwrap_or((zenith_relay_core::protocol::ProxyMode::Direct, false));
    let quota_stale_after_ms = QUOTA_STALE_AFTER_MS;
    let operational = account_operational_state(AccountOperationalInput {
        enabled: record.account.enabled,
        in_pool: record.account.in_pool,
        draining: record.account.draining,
        secret_available,
        proxy_available,
        auth_state: record.account.auth_state,
        health: record.account.health,
        subscription: &record.account.subscription,
        quota: &record.account.quota,
        last_error_code: record.account.last_error_code.as_deref(),
        now_ms,
        quota_stale_after_ms,
    });
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
        operational_status: operational.status,
        auth_state: record.account.auth_state,
        health: format!("{:?}", record.account.health).to_ascii_lowercase(),
        models: record.models.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        api_equivalent,
        purchase_cost_micro_usd: record.purchase_cost_micro_usd,
        subscription: record.account.subscription.clone(),
        quota: record.account.quota.clone(),
        secret_available,
        remote_location: record.remote_location.clone(),
        proxy_mode,
        proxy_available,
        proxy_id: None,
        quota_refresh_status: zenith_relay_core::protocol::quota_refresh_status(
            record.account.auth_state,
            &record.account.quota,
            refreshing,
        ),
        routing_block_reason: operational.routing_block_reason,
        last_error_code: record.account.last_error_code.clone(),
    })
}

fn oauth_account_runtime_available(
    routing_order: &[CandidateRuntimeSnapshot],
    account_id: &str,
) -> Option<bool> {
    routing_order
        .iter()
        .find(|candidate| {
            candidate.kind == CandidateKind::OAuthAccount && candidate.candidate_id == account_id
        })
        .map(|candidate| candidate.available)
}

fn account_runtime_warning(
    record: &LocalAccountRecord,
    settings: &crate::local_pool::models::GatewaySettings,
    account_id: &str,
) -> String {
    let code = match CredentialStore::from_backend(NativeSecretBackend).load(account_id) {
        Err(_) => "account_runtime_credential_unreadable",
        Ok(None) => "account_runtime_credential_missing",
        Ok(Some(credentials)) if credentials.provider_account_id().is_none() => {
            "account_runtime_provider_account_id_missing"
        }
        Ok(Some(credentials)) if effective_proxy_config(settings, &credentials).is_err() => {
            "account_runtime_proxy_invalid"
        }
        Ok(Some(_)) => "account_runtime_not_registered",
    };
    let redacted = if record.account.id.chars().count() <= 12 {
        record.account.id.clone()
    } else {
        format!(
            "{}...",
            record.account.id.chars().take(8).collect::<String>()
        )
    };
    format!("{code}:{redacted}")
}

#[cfg(test)]
mod parity_tests {
    use super::*;

    #[test]
    fn local_and_remote_snapshots_share_the_same_top_level_contract() {
        let local = serde_json::to_value(RuntimeStateSnapshot {
            schema_version: 1,
            configuration_revision: None,
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
                cooldown_after_failures: zenith_relay_core::DEFAULT_COOLDOWN_AFTER_FAILURES,
                keep_last_candidate_available:
                    zenith_relay_core::DEFAULT_KEEP_LAST_CANDIDATE_AVAILABLE,
                routing_strategy: Default::default(),
                subscription_plan_order: Vec::new(),
                default_service_tier: Default::default(),
                image_base_model: None,
                models: Vec::new(),
                common_proxy_configured: false,
                common_proxy_available: false,
                common_proxy_id: None,
                account_proxy_required: false,
                quota_request_timeout_seconds: 20,
                chatgpt_interface_quota_reserve_basis_points: Some(100),
                routing_order: Vec::new(),
            },
            platform: "test".into(),
            capabilities: Capabilities::desktop_local(),
            sources: Vec::new(),
            accounts: Vec::new(),
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

    #[test]
    fn runtime_account_presence_is_bound_to_the_oauth_candidate() {
        let candidate = CandidateRuntimeSnapshot {
            candidate_id: "account_plus".into(),
            kind: CandidateKind::OAuthAccount,
            available: true,
            in_flight: 0,
            active_request_count: 0,
            active_models: Vec::new(),
            last_used_at_ms: None,
            next_retry_at_ms: None,
            half_open: false,
            dispatches: 0,
        };
        assert_eq!(
            oauth_account_runtime_available(std::slice::from_ref(&candidate), "account_plus"),
            Some(true)
        );
        let unavailable = CandidateRuntimeSnapshot {
            candidate_id: "account_unavailable".into(),
            kind: CandidateKind::OAuthAccount,
            available: false,
            in_flight: 0,
            active_request_count: 0,
            active_models: Vec::new(),
            last_used_at_ms: None,
            next_retry_at_ms: None,
            half_open: false,
            dispatches: 0,
        };
        assert_eq!(
            oauth_account_runtime_available(
                std::slice::from_ref(&unavailable),
                "account_unavailable"
            ),
            Some(false)
        );
        assert_eq!(
            oauth_account_runtime_available(&[candidate], "missing"),
            None
        );
    }
}
