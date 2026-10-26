use super::{current_time_ms, runtime_account_operational_state};
use crate::local_pool::{
    error::Result,
    models::{GatewaySettings, LocalAccountRecord},
    state::{AccountCredentialFacts, DesktopState},
    store::LocalPoolStore,
};
use std::collections::{BTreeMap, HashMap};
use zenith_relay_core::{
    CandidateQuotaReference, GatewayRuntime, QUOTA_STALE_AFTER_MS,
    providers::chatgpt::OAuthClientKind,
};

pub(super) fn account_quota_references(
    settings: &GatewaySettings,
    accounts: &[LocalAccountRecord],
    credentials: &HashMap<String, Option<AccountCredentialFacts>>,
    now_ms: u64,
) -> BTreeMap<String, CandidateQuotaReference> {
    let mut references = BTreeMap::new();
    for group in &settings.account_routes {
        let Some(selected) = accounts
            .iter()
            .find(|account| account.account.id == group.selected_account_id)
        else {
            continue;
        };
        let Some(facts) = credentials
            .get(&selected.account.id)
            .and_then(Option::as_ref)
        else {
            continue;
        };
        if facts.oauth_client_kind != OAuthClientKind::Siwc
            || !facts.plan_use_enabled
            || !reference_account_usable(selected, now_ms)
            || selected.account.quota.limit_reached
            || selected.account.quota.primary.is_some()
            || selected.account.quota.secondary.is_some()
            || !selected.account.quota.supplemental.is_empty()
        {
            continue;
        }
        let reference = accounts
            .iter()
            .filter(|account| {
                account.account.id != selected.account.id
                    && group.account_ids.contains(&account.account.id)
                    && credentials
                        .get(&account.account.id)
                        .and_then(Option::as_ref)
                        .is_some_and(|facts| {
                            facts.has_oauth
                                && matches!(
                                    facts.oauth_client_kind,
                                    OAuthClientKind::Codex | OAuthClientKind::ExcelBps
                                )
                        })
            })
            .filter_map(|account| quota_reference(account, now_ms))
            .reduce(|current, next| {
                if next.observed_at_ms > current.observed_at_ms {
                    next
                } else {
                    current
                }
            });
        if let Some(reference) = reference {
            references.insert(selected.account.id.clone(), reference);
        }
    }
    references
}

fn reference_account_usable(account: &LocalAccountRecord, now_ms: u64) -> bool {
    account.account.enabled
        && !account.account.draining
        && account.recovery.is_none()
        && runtime_account_operational_state(&account.account, now_ms)
            .health
            .is_eligible()
}

fn quota_reference(account: &LocalAccountRecord, now_ms: u64) -> Option<CandidateQuotaReference> {
    let quota = &account.account.quota;
    if !reference_account_usable(account, now_ms) || quota.error.is_some() {
        return None;
    }
    let mut observed_at_ms = None;
    let mut remaining = 10_000_u64;
    let mut valid_until_ms = account
        .account
        .subscription
        .active_until_ms
        .unwrap_or(u64::MAX);
    for window in quota.primary.iter().chain(quota.secondary.iter()) {
        let value = u64::from(window.available_basis_points?);
        if value > 10_000 || window.observed_at_ms == 0 || window.observed_at_ms > now_ms {
            return None;
        }
        remaining = remaining.min(value);
        observed_at_ms = Some(
            observed_at_ms.map_or(window.observed_at_ms, |previous: u64| {
                previous.min(window.observed_at_ms)
            }),
        );
        valid_until_ms = valid_until_ms
            .min(window.observed_at_ms.saturating_add(QUOTA_STALE_AFTER_MS))
            .min(window.reset_at_ms.unwrap_or(u64::MAX));
    }
    let observed_at_ms = observed_at_ms?;
    if now_ms >= valid_until_ms {
        return None;
    }
    Some(CandidateQuotaReference {
        source_account_id: account.account.id.clone(),
        remaining_basis_points: if quota.limit_reached { 0 } else { remaining },
        observed_at_ms,
        valid_until_ms: Some(valid_until_ms),
    })
}

pub(in crate::local_pool) fn sync_account_quota_references(
    state: &DesktopState,
    runtime: &GatewayRuntime,
    store: &LocalPoolStore,
) -> Result<()> {
    if store.gateway().account_routes.is_empty() {
        runtime.set_account_quota_references(BTreeMap::new());
        return Ok(());
    }
    let facts = match state.cached_account_credential_facts(store.accounts()) {
        Ok(facts) => facts,
        Err(error) => {
            runtime.set_account_quota_references(BTreeMap::new());
            return Err(error);
        }
    };
    runtime.set_account_quota_references(account_quota_references(
        store.gateway(),
        store.accounts(),
        &facts,
        current_time_ms(),
    ));
    Ok(())
}

#[cfg(test)]
mod tests;
