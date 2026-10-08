use crate::local_pool::{
    accounts::proxy::{proxy_route_is_usable, proxy_route_status},
    models::{GatewaySettings, LocalAccountRecord, ProviderSourceRecord},
    state::AccountCredentialFacts,
};
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::ProxyMode;
use zenith_relay_core::protocol::{
    account_operational_state, AccountOperationalInput, AccountSummary, QuotaWindowUsage,
    SourceSummary,
};
use zenith_relay_core::{
    ApiEquivalentSummary, CandidateKind, CandidateRuntimeSnapshot, QUOTA_STALE_AFTER_MS,
};

pub(super) fn local_source_summary(
    source_record: &ProviderSourceRecord,
    refresh_revision: Option<u64>,
    secret_available: bool,
    runtime_available: Option<bool>,
    api_equivalent: ApiEquivalentSummary,
) -> crate::local_pool::error::Result<SourceSummary> {
    Ok(SourceSummary::from_stored_source(
        source_record,
        secret_available,
        runtime_available,
        api_equivalent,
        source_record.last_error.clone(),
        refresh_revision,
    ))
}

pub(super) struct LocalAccountSummaryContext<'a> {
    pub(super) settings: &'a GatewaySettings,
    pub(super) credentials: Option<AccountCredentialFacts>,
    pub(super) common_proxy_available: bool,
    pub(super) api_equivalent: ApiEquivalentSummary,
    pub(super) quota_window_usage: Option<QuotaWindowUsage>,
    pub(super) now_ms: u64,
    pub(super) refreshing: bool,
    pub(super) runtime_available: Option<bool>,
}

pub(super) fn local_account_summary(
    account_record: &LocalAccountRecord,
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
        .map(|credentials| {
            proxy_route_status(settings, credentials.proxy_route(), common_proxy_available)
        })
        .unwrap_or((ProxyMode::Direct, false));
    let quota_stale_after_ms = QUOTA_STALE_AFTER_MS;
    let operational = account_operational_state(AccountOperationalInput::from_source(
        &account_record.account,
        secret_available,
        proxy_available,
        now_ms,
        quota_stale_after_ms,
    ));
    Ok(AccountSummary {
        id: account_record.account.id.clone(),
        label: account_record.account.label.clone(),
        identity_hint: account_record
            .account
            .identity
            .identity_hash
            .chars()
            .take(12)
            .collect(),
        provider_family: account_record.provider_family.clone(),
        basis_points_available: credentials
            .is_some_and(AccountCredentialFacts::basis_points_available),
        basis_points_enabled: settings.basis_points_enabled
            && credentials.is_some_and(AccountCredentialFacts::basis_points_available),
        enabled: account_record.account.enabled,
        in_pool: account_record.account.in_pool,
        draining: account_record.account.draining,
        operational_status: operational.status.with_runtime_available(runtime_available),
        auth_state: account_record.account.auth_state,
        health: account_record.account.health.summary_label(),
        models: account_record.effective_models().to_vec(),
        allowed_models: account_record.allowed_models.clone(),
        excluded_models: account_record.excluded_models.clone(),
        priority: account_record.priority,
        weight: account_record.weight,
        api_equivalent,
        quota_window_usage,
        purchase_cost_micro_usd: account_record.purchase_cost_micro_usd,
        subscription: account_record.account.subscription.clone(),
        quota: account_record.account.quota.clone(),
        secret_available,
        remote_location: account_record.remote_location.clone(),
        proxy_mode,
        proxy_available,
        proxy_id: None,
        quota_refresh_status: zenith_relay_core::protocol::quota_refresh_status(
            account_record.account.auth_state,
            &account_record.account.quota,
            refreshing,
        ),
        refresh_state: Default::default(),
        routing_block_reason: operational.routing_block_reason,
        last_error_code: account_record.account.last_error_code.clone(),
        client_auth_status: account_record.client_auth_status.clone(),
        last_client_login_redirect_at_ms: account_record.last_client_login_redirect_at_ms,
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
    account_record: &LocalAccountRecord,
    settings: &GatewaySettings,
    credentials: Option<AccountCredentialFacts>,
    common_proxy_available: bool,
) -> String {
    let code = match credentials {
        None => error_codes::ACCOUNT_RUNTIME_CREDENTIAL_MISSING,
        Some(credentials) if !credentials.has_provider_account_id => {
            error_codes::ACCOUNT_RUNTIME_PROVIDER_ACCOUNT_ID_MISSING
        }
        Some(credentials)
            if !proxy_route_is_usable(
                settings,
                credentials.proxy_route(),
                common_proxy_available,
            ) =>
        {
            error_codes::ACCOUNT_RUNTIME_PROXY_INVALID
        }
        Some(_) => "account_runtime_not_registered",
    };
    let redacted = if account_record.account.id.chars().count() <= 12 {
        account_record.account.id.clone()
    } else {
        format!(
            "{}...",
            account_record
                .account
                .id
                .chars()
                .take(8)
                .collect::<String>()
        )
    };
    format!("{code}:{redacted}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::accounts::{
        credentials::StoredCodexCredentials,
        proxy::{effective_proxy_config, proxy_route_is_usable, proxy_route_status, proxy_status},
    };

    fn credentials() -> StoredCodexCredentials {
        StoredCodexCredentials::new(
            "account_snapshot_facts",
            "synthetic-access".into(),
            Some("synthetic-refresh".into()),
            None,
            None,
            1,
            1,
            None,
            Some("provider-account".into()),
            None,
            None,
            None,
            false,
        )
        .unwrap()
    }

    #[test]
    fn snapshot_facts_match_proxy_decisions_without_reloading_the_common_secret() {
        let settings = GatewaySettings::default();
        let direct = credentials();
        let direct_facts = AccountCredentialFacts::from_stored(&direct);
        assert_eq!(
            proxy_status(&settings, &direct, false),
            proxy_route_status(&settings, direct_facts.proxy_route(), false)
        );
        assert_eq!(
            effective_proxy_config(&settings, &direct).is_err(),
            !proxy_route_is_usable(&settings, direct_facts.proxy_route(), false)
        );

        let mut required = settings.clone();
        required.account_proxy_required = true;
        assert!(!proxy_route_is_usable(
            &required,
            direct_facts.proxy_route(),
            false
        ));
        assert!(effective_proxy_config(&required, &direct).is_err());

        let account_proxy = direct
            .clone()
            .with_proxy_url(Some("http://127.0.0.1:8080".into()))
            .unwrap();
        let account_facts = AccountCredentialFacts::from_stored(&account_proxy);
        assert_eq!(
            proxy_status(&required, &account_proxy, false),
            proxy_route_status(&required, account_facts.proxy_route(), false)
        );
        assert!(proxy_route_is_usable(
            &required,
            account_facts.proxy_route(),
            false
        ));
        assert!(effective_proxy_config(&required, &account_proxy).is_ok());

        let bypass = credentials().with_proxy_route(None, true).unwrap();
        let bypass_facts = AccountCredentialFacts::from_stored(&bypass);
        assert_eq!(
            proxy_status(&required, &bypass, true),
            proxy_route_status(&required, bypass_facts.proxy_route(), true)
        );
        assert!(!proxy_route_is_usable(
            &required,
            bypass_facts.proxy_route(),
            true
        ));

        let mut common = settings.clone();
        common.common_proxy_configured = true;
        assert_eq!(
            proxy_status(&common, &direct, false),
            proxy_route_status(&common, direct_facts.proxy_route(), false)
        );
        assert_eq!(
            proxy_status(&common, &direct, true),
            proxy_route_status(&common, direct_facts.proxy_route(), true)
        );
        assert!(!proxy_route_is_usable(
            &common,
            direct_facts.proxy_route(),
            false
        ));
        assert!(proxy_route_is_usable(
            &common,
            direct_facts.proxy_route(),
            true
        ));
    }
}
