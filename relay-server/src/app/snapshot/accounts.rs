use super::*;

pub(super) fn account_summaries(
    state: &AppState,
    records: &[(ServerAccountRecord, AccountRefreshFence)],
    inputs: AccountSnapshotInputs<'_>,
    warnings: &mut Vec<String>,
) -> Result<Vec<AccountSummary>, String> {
    let quota_windows = records
        .iter()
        .filter_map(|(account_record, _)| {
            let window = zenith_relay_core::protocol::api_equivalent_projection_window(
                &account_record.quota,
            )?;
            Some((
                identity_hint(&account_record.id),
                window.window_start_ms.unwrap_or_default(),
                window.observed_at_ms,
            ))
        })
        .collect::<Vec<_>>();
    let quota_equivalents = state.store.quota_window_equivalents_with_pricing(
        &quota_windows,
        inputs.pricing_catalog,
        inputs.pricing_context,
    )?;
    records
        .iter()
        .map(|(account_record, fence)| {
            let secret = state.vault.load(&account_record.secret_ref)?;
            let secret_available = secret.is_some();
            if !secret_available {
                warnings.push(format!("account_secret_missing:{}", account_record.id));
            }
            let credential = secret.as_deref().and_then(|credential_json| {
                serde_json::from_str::<AccountCredential>(credential_json).ok()
            });
            let basis_points_available = credential.as_ref().is_some_and(|account_credential| {
                account_credential.has_oauth() && !account_credential.is_agent_identity()
            });
            let (proxy_mode, proxy_available) = credential
                .as_ref()
                .map(|credential| {
                    account_proxy_status(
                        state,
                        account_record,
                        credential,
                        inputs.proxy_settings.common_configured,
                        inputs.proxy_settings.common_available,
                        inputs.proxy_settings.required,
                    )
                })
                .unwrap_or((ProxyMode::Direct, false));
            let quota_window_usage = quota_window_usage(account_record, &quota_equivalents);
            let mut summary = account_summary(
                account_record,
                AccountSummaryInputs {
                    secret_available,
                    basis_points_available,
                    basis_points_enabled: inputs.basis_points_enabled,
                    proxy_mode,
                    proxy_available,
                    api_equivalent: inputs
                        .equivalents
                        .get(&identity_hint(&account_record.id))
                        .copied()
                        .unwrap_or_default(),
                    quota_window_usage,
                    quota_stale_after_ms: QUOTA_STALE_AFTER_MS,
                },
            );
            summary.refresh_state = AccountRefreshState {
                models: RefreshStatus::from_evidence(
                    state
                        .refresh
                        .freshness(&fence.identity(), RefreshKind::Models),
                    !account_record.models.is_empty(),
                ),
                quota: RefreshStatus::from_evidence(
                    state
                        .refresh
                        .freshness(&fence.identity(), RefreshKind::Quota),
                    account_record.quota.updated_at_ms.is_some(),
                ),
            };
            Ok(summary)
        })
        .collect()
}

fn quota_window_usage(
    account_record: &ServerAccountRecord,
    equivalents: &HashMap<String, ApiEquivalentSummary>,
) -> Option<QuotaWindowUsage> {
    let window =
        zenith_relay_core::protocol::api_equivalent_projection_window(&account_record.quota)?;
    let hint = identity_hint(&account_record.id);
    Some(QuotaWindowUsage {
        kind: window.kind,
        window_start_ms: window.window_start_ms.unwrap_or_default(),
        observed_at_ms: window.observed_at_ms,
        window_minutes: window.window_minutes.unwrap_or_default(),
        api_equivalent: equivalents.get(&hint).copied().unwrap_or_default(),
    })
}
