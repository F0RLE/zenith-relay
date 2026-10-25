use crate::local_pool::{
    error::Result,
    models::LocalAccountRecord,
    state::{AccountCredentialFacts, DesktopState},
    store::telemetry_db::{CapacityObservation, TelemetryDb},
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::sync::Notify;
use zenith_relay_core::{
    pricing::{PricingCatalog, PricingContext},
    providers::chatgpt::OAuthClientKind,
};

#[cfg(test)]
mod tests;

/// One coalesced worker follows existing persisted-state events even while the
/// window is hidden. No provider polling and no full UI snapshot/model projection.
pub(super) fn start(app: AppHandle) {
    let changed = Arc::new(Notify::new());
    let notify = changed.clone();
    app.listen("zenith-state-changed", move |_| notify.notify_one());
    changed.notify_one();
    tauri::async_runtime::spawn(async move {
        loop {
            changed.notified().await;
            tokio::time::sleep(Duration::from_millis(250)).await;
            let state = app.state::<DesktopState>().inner().clone();
            let result = tauri::async_runtime::spawn_blocking(move || collect(&state)).await;
            match result {
                Ok(Ok(true)) => {
                    let _ = app.emit("zenith-state-changed", ());
                }
                Ok(Ok(false)) => {}
                _ => crate::diagnostics::record_error(
                    "weekly-capacity",
                    Some("observation_failed"),
                    "Weekly capacity observation could not be stored",
                    &[],
                ),
            }
        }
    });
}

fn collect(state: &DesktopState) -> Result<bool> {
    let accounts = state.store()?.accounts().to_vec();
    let facts = state.cached_account_credential_facts(&accounts)?;
    let catalog = state.pricing_catalog();
    // This collector accepts only OpenAI account ledgers. API-source overrides
    // must not invalidate subscription measurements or change their prices.
    let pricing = PricingContext::default();
    let before = state.telemetry.weekly_capacity_revision();
    record_capacity(
        &accounts,
        &facts,
        &state.telemetry,
        &catalog,
        &pricing,
        zenith_relay_core::unix_time_ms(),
    )?;
    Ok(before != state.telemetry.weekly_capacity_revision())
}

/// Reuse completed quota observations and existing telemetry. No provider calls.
fn record_capacity(
    inventory: &[LocalAccountRecord],
    account_facts: &HashMap<String, Option<AccountCredentialFacts>>,
    telemetry: &TelemetryDb,
    catalog: &PricingCatalog,
    pricing: &PricingContext,
    now_ms: u64,
) -> Result<()> {
    let mut ledgers = BTreeMap::<String, Vec<&LocalAccountRecord>>::new();
    for account in inventory {
        if account
            .provider_family
            .as_deref()
            .is_some_and(|provider| provider != "openai")
        {
            continue;
        }
        // A credits-only SIWC route does not spend the shared subscription quota.
        let Some(key) = account_facts
            .get(&account.account.id)
            .and_then(Option::as_ref)
            .filter(|facts| facts.plan_use_enabled)
            .and_then(|facts| facts.credit_balance_key)
        else {
            continue;
        };
        ledgers.entry(hex::encode(key)).or_default().push(account);
    }
    let revision = pricing.revision_key(catalog);
    for (ledger, accounts) in ledgers {
        // SIWC can expose an application ceiling. Only Codex/BPS observations
        // establish the shared weekly window; all linked routes contribute usage.
        let source = accounts
            .iter()
            .filter(|account| account.account.quota.error.is_none())
            .filter(|account| {
                account_facts
                    .get(&account.account.id)
                    .and_then(Option::as_ref)
                    .is_some_and(|facts| {
                        matches!(
                            facts.oauth_client_kind,
                            OAuthClientKind::Codex | OAuthClientKind::ExcelBps
                        )
                    })
            })
            .filter_map(|account| {
                zenith_relay_core::protocol::api_equivalent_projection_window(
                    &account.account.quota,
                )
                .map(|window| (*account, window))
            })
            .max_by_key(|(_, window)| window.observed_at_ms);
        let Some((account, window)) = source else {
            continue;
        };
        let Some(plan) = account.account.subscription.plan_type.as_deref() else {
            continue;
        };
        let mut ids = accounts
            .iter()
            .map(|account| account.account.id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        telemetry.observe_weekly_capacity(
            CapacityObservation {
                ledger: &ledger,
                account_ids: &ids,
                plan,
                window,
                pricing_revision: &revision,
                now_ms,
            },
            catalog,
            pricing,
        )?;
    }
    Ok(())
}
