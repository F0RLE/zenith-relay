use crate::local_pool::{
    accounts::{
        credentials::StoredCodexCredentials,
        proxy::{effective_proxy_config, proxy_status},
    },
    models::{GatewaySettings, LocalAccountRecord, ProviderSourceRecord},
};
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::{
    account_operational_state, operational_status, AccountOperationalInput, AccountSummary,
    QuotaWindowUsage, SourceSummary,
};
use zenith_relay_core::{
    ApiEquivalentSummary, CandidateKind, CandidateRuntimeSnapshot, QUOTA_STALE_AFTER_MS,
};

pub(super) fn local_source_summary(
    record: &ProviderSourceRecord,
    refresh_revision: Option<u64>,
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
        pricing_provider: record.pricing_provider.clone(),
        official_provider_family: record.official_provider_family.clone(),
        wire_api: record.wire_api,
        protocol_config: record
            .protocol_config
            .with_effective_capabilities(&record.base_url, &record.models),
        protocol_bindings: record.protocol_bindings.clone(),
        resolved_protocol_bindings: Some(record.effective_protocol_bindings().unwrap_or_default()),
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
        refresh_revision,
        refresh_state: Default::default(),
        provider_stats: None,
    })
}

pub(super) struct LocalAccountSummaryContext<'a> {
    pub(super) settings: &'a GatewaySettings,
    pub(super) credentials: Option<&'a StoredCodexCredentials>,
    pub(super) common_proxy_available: bool,
    pub(super) api_equivalent: ApiEquivalentSummary,
    pub(super) quota_window_usage: Option<QuotaWindowUsage>,
    pub(super) now_ms: u64,
    pub(super) refreshing: bool,
    pub(super) runtime_available: Option<bool>,
}

pub(super) fn local_account_summary(
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
        runtime_available,
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
        provider_family: record.provider_family.clone(),
        basis_points_available: credentials
            .is_some_and(|value| value.has_oauth() && !value.is_agent_identity()),
        basis_points_enabled: settings.basis_points_enabled
            && credentials.is_some_and(|value| value.has_oauth() && !value.is_agent_identity()),
        enabled: record.account.enabled,
        in_pool: record.account.in_pool,
        draining: record.account.draining,
        operational_status: operational.status.with_runtime_available(runtime_available),
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
        refresh_state: Default::default(),
        routing_block_reason: operational.routing_block_reason,
        last_error_code: record.account.last_error_code.clone(),
        client_auth_status: record.client_auth_status.clone(),
        last_client_login_redirect_at_ms: record.last_client_login_redirect_at_ms,
    })
}

pub(in crate::local_pool::commands::state) fn oauth_account_runtime_available(
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

pub(super) fn account_runtime_warning(
    record: &LocalAccountRecord,
    settings: &crate::local_pool::models::GatewaySettings,
    _account_id: &str,
    credentials: Option<&StoredCodexCredentials>,
) -> String {
    let code = match credentials {
        None => error_codes::ACCOUNT_RUNTIME_CREDENTIAL_MISSING,
        Some(credentials) if credentials.provider_account_id().is_none() => {
            error_codes::ACCOUNT_RUNTIME_PROVIDER_ACCOUNT_ID_MISSING
        }
        Some(credentials) if effective_proxy_config(settings, credentials).is_err() => {
            error_codes::ACCOUNT_RUNTIME_PROXY_INVALID
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
