use crate::local_pool::{
    accounts::{
        credentials::StoredCodexCredentials,
        proxy::{common_proxy_available, effective_proxy_config, proxy_status},
    },
    error::CommandError,
    models::{GatewaySettings, LocalAccountRecord, LocalPoolSnapshot, ProviderSourceRecord},
    state::DesktopState,
};
use crate::platform;
use std::collections::BTreeMap;
use std::time::Instant;
use tauri::State;
use zenith_relay_core::protocol::{
    account_operational_state, apply_model_display_order, apply_pool_model_configuration,
    operational_status, pool_candidate_count, pool_model_summaries,
    pooled_source_runtime_available, source_runtime_available, AccountOperationalInput,
    AccountSummary, Capabilities, GatewaySummary, QuotaWindowUsage, RuntimeStateSnapshot,
    RuntimeTargetSummary, SourceSummary, UsageQuery,
};
use zenith_relay_core::{
    unix_time_ms, ApiEquivalentSummary, CandidateKind, CandidateRuntimeSnapshot,
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
    let inputs = state.runtime_inputs().await?;
    let running = inputs.running;
    let runtime = state.gateway.runtime().await;
    let routing_order = runtime
        .as_ref()
        .map(|runtime| runtime.candidate_runtime_order())
        .unwrap_or_default();
    let common_proxy_available = common_proxy_available(&inputs.gateway);
    let snapshot_at_ms = unix_time_ms();
    let source_price_overrides = super::source_model_price_overrides(&inputs.sources);
    let equivalents = state.telemetry.api_equivalents_with_price_overrides(
        &inputs.gateway.model_price_overrides,
        &source_price_overrides,
    )?;
    let mut quota_window_usages = BTreeMap::new();
    for record in &inputs.accounts {
        let Some(window) =
            zenith_relay_core::protocol::api_equivalent_projection_window(&record.account.quota)
        else {
            continue;
        };
        let window_start_ms = window.window_start_ms.unwrap_or_default();
        let window_minutes = window.window_minutes.unwrap_or_default();
        let usage = state.telemetry.usage_page_with_price_overrides(
            &UsageQuery {
                page: 1,
                page_size: 1,
                from_ms: Some(window_start_ms),
                to_ms: Some(window.observed_at_ms),
                source_or_account_query: Some(record.account.id.clone()),
                ..UsageQuery::default()
            },
            &inputs.gateway.model_price_overrides,
            &source_price_overrides,
        )?;
        quota_window_usages.insert(
            record.account.id.clone(),
            QuotaWindowUsage {
                kind: window.kind,
                window_start_ms,
                observed_at_ms: window.observed_at_ms,
                window_minutes,
                api_equivalent: usage.totals.api_equivalent,
            },
        );
    }
    let source_summaries = inputs
        .sources
        .iter()
        .map(|record| {
            local_source_summary(
                record,
                inputs
                    .source_api_keys
                    .get(&record.id)
                    .and_then(Option::as_ref)
                    .is_some(),
                (running && record.enabled).then(|| {
                    if record.in_pool {
                        pooled_source_runtime_available(&routing_order, &record.id)
                    } else {
                        source_runtime_available(&routing_order, &record.id)
                    }
                }),
                equivalents
                    .sources
                    .get(&record.id)
                    .copied()
                    .unwrap_or_default(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let account_summaries = inputs
        .accounts
        .iter()
        .map(|record| {
            local_account_summary(
                record,
                LocalAccountSummaryContext {
                    settings: &inputs.gateway,
                    credentials: inputs
                        .account_credentials
                        .get(&record.account.id)
                        .and_then(Option::as_ref),
                    common_proxy_available,
                    api_equivalent: equivalents
                        .accounts
                        .get(&record.account.id)
                        .copied()
                        .unwrap_or_default(),
                    quota_window_usage: quota_window_usages.get(&record.account.id).cloned(),
                    now_ms: snapshot_at_ms,
                    refreshing: state.quota_refresh_in_flight(&record.account.id)?,
                },
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut warnings = inputs.warnings;
    if running {
        for record in &inputs.accounts {
            if record.account.enabled
                && record.account.in_pool
                && !record.account.draining
                && oauth_account_runtime_available(&routing_order, &record.account.id).is_none()
            {
                warnings.push(account_runtime_warning(
                    record,
                    &inputs.gateway,
                    &record.account.id,
                    inputs
                        .account_credentials
                        .get(&record.account.id)
                        .and_then(Option::as_ref),
                ));
            }
        }
    }
    let mut models = pool_model_summaries(
        &source_summaries,
        &account_summaries,
        &inputs.gateway.hidden_models,
    );
    apply_pool_model_configuration(
        &mut models,
        &source_summaries,
        &account_summaries,
        &inputs.gateway.model_price_overrides,
        &inputs.gateway.model_reasoning_allowed_levels,
        &inputs.gateway.model_service_tier_overrides,
        runtime.as_deref(),
    );
    apply_model_display_order(&mut models, &inputs.gateway.model_display_order);
    let visible_model_ids = models
        .iter()
        .filter(|model| model.enabled)
        .map(|model| model.id.clone())
        .collect();
    let candidate_count = pool_candidate_count(&source_summaries, &account_summaries);
    let base_url = format!(
        "http://{}:{}/v1",
        inputs.gateway.client_host, inputs.gateway.port
    );
    let response = RuntimeStateSnapshot {
        schema_version: crate::local_pool::models::CURRENT_SCHEMA_VERSION,
        configuration_revision: None,
        runtime_target: RuntimeTargetSummary {
            kind: "local".to_string(),
            connected: running,
            origin: Some(format!(
                "http://{}:{}",
                inputs.gateway.client_host, inputs.gateway.port
            )),
            server_id: None,
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        },
        gateway: GatewaySummary {
            running,
            base_url,
            candidate_count,
            visible_model_ids,
            max_retry_candidates: inputs.gateway.max_retry_candidates,
            cooldown_after_failures: inputs.gateway.cooldown_after_failures,
            keep_last_candidate_available: inputs.gateway.keep_last_candidate_available,
            routing_strategy: inputs.gateway.routing_strategy,
            subscription_plan_order: inputs.gateway.subscription_plan_order.clone(),
            default_service_tier: inputs.gateway.default_service_tier,
            image_base_model: inputs.gateway.image_base_model.clone(),
            models,
            common_proxy_configured: inputs.gateway.common_proxy_configured,
            common_proxy_available,
            common_proxy_id: None,
            account_proxy_required: inputs.gateway.account_proxy_required,
            quota_request_timeout_seconds: inputs.gateway.quota_request_timeout_seconds,
            chatgpt_interface_quota_reserve_basis_points: Some(
                inputs.gateway.chatgpt_interface_quota_reserve_basis_points,
            ),
            codex_background_tasks_enabled: inputs.gateway.codex_background_tasks_enabled,
            codex_websockets_enabled: inputs.gateway.codex_websockets_enabled,
            routing_order,
        },
        platform: platform::platform_name().to_string(),
        capabilities: Capabilities::desktop_local(),
        sources: source_summaries,
        accounts: account_summaries,
        automations: inputs.automations.tasks,
        wake_history: inputs.automations.state.history().iter().cloned().collect(),
        warnings,
    };
    state.record_performance_async(
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
    secret_available: bool,
    runtime_available: Option<bool>,
    api_equivalent: ApiEquivalentSummary,
) -> crate::local_pool::error::Result<SourceSummary> {
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

struct LocalAccountSummaryContext<'a> {
    settings: &'a GatewaySettings,
    credentials: Option<&'a StoredCodexCredentials>,
    common_proxy_available: bool,
    api_equivalent: ApiEquivalentSummary,
    quota_window_usage: Option<QuotaWindowUsage>,
    now_ms: u64,
    refreshing: bool,
}

fn local_account_summary(
    record: &LocalAccountRecord,
    context: LocalAccountSummaryContext<'_>,
) -> crate::local_pool::error::Result<AccountSummary> {
    let LocalAccountSummaryContext {
        settings,
        credentials,
        common_proxy_available,
        api_equivalent,
        quota_window_usage,
        now_ms,
        refreshing,
    } = context;
    let secret_available = credentials.is_some();
    let (proxy_mode, proxy_available) = credentials
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
        models: record.effective_models().to_vec(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        api_equivalent,
        quota_window_usage,
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
    _account_id: &str,
    credentials: Option<&StoredCodexCredentials>,
) -> String {
    let code = match credentials {
        None => "account_runtime_credential_missing",
        Some(credentials) if credentials.provider_account_id().is_none() => {
            "account_runtime_provider_account_id_missing"
        }
        Some(credentials) if effective_proxy_config(settings, credentials).is_err() => {
            "account_runtime_proxy_invalid"
        }
        Some(_) => "account_runtime_not_registered",
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
                codex_background_tasks_enabled: true,
                codex_websockets_enabled: true,
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
            model_retries: Vec::new(),
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
            model_retries: Vec::new(),
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
