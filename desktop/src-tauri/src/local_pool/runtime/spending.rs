use super::current_time_ms;
use crate::local_pool::state::AccountCredentialFacts;
use crate::local_pool::{
    accounts::pool_resources::{weekly_reset_member, weekly_reset_rule_matches},
    models::{AutomationRecords, GatewaySettings, LocalAccountRecord},
};
use std::collections::{BTreeMap, HashMap};
use zenith_relay_core::{
    CandidateQuotaReference,
    providers::chatgpt::SUBSCRIPTION_REFRESH_INTERVAL_MS,
    quota::{QuotaSpendDeadline, QuotaSpendResets, SubscriptionStatus},
};

pub(super) fn account_spend_deadlines(
    accounts: &[LocalAccountRecord],
    now_ms: u64,
) -> BTreeMap<String, QuotaSpendDeadline> {
    accounts
        .iter()
        .filter_map(|record| {
            if record.recovery.is_some() {
                return None;
            }
            QuotaSpendDeadline::from_subscription(
                &record.account.subscription,
                now_ms,
                SUBSCRIPTION_REFRESH_INTERVAL_MS,
            )
            .map(|deadline| (record.account.id.clone(), deadline))
        })
        .collect()
}

pub(super) fn inherit_linked_deadlines(
    deadlines: &mut BTreeMap<String, QuotaSpendDeadline>,
    accounts: &[LocalAccountRecord],
    references: &BTreeMap<String, CandidateQuotaReference>,
) {
    for (selected_id, reference) in references {
        if deadlines.contains_key(selected_id) {
            continue;
        }
        let Some(selected) = accounts.iter().find(|r| r.account.id == *selected_id) else {
            continue;
        };
        let Some(reader) = accounts
            .iter()
            .find(|r| r.account.id == reference.source_account_id)
        else {
            continue;
        };
        let subscription = &selected.account.subscription;
        if matches!(
            subscription.status,
            SubscriptionStatus::Expired | SubscriptionStatus::Forbidden
        ) || subscription.is_free_plan()
            || subscription
                .plan_type
                .as_ref()
                .is_some_and(|plan| reader.account.subscription.plan_type.as_ref() != Some(plan))
        {
            continue;
        }
        if let Some(mut deadline) = deadlines.get(&reader.account.id).copied() {
            deadline.valid_until_ms = deadline
                .valid_until_ms
                .min(reference.valid_until_ms.unwrap_or(u64::MAX));
            deadlines.insert(selected_id.clone(), deadline);
        }
    }
}

pub(super) fn apply_reset_budgets(
    deadlines: &mut BTreeMap<String, QuotaSpendDeadline>,
    settings: &GatewaySettings,
    accounts: &[LocalAccountRecord],
    automations: &AutomationRecords,
    credentials: &HashMap<String, Option<AccountCredentialFacts>>,
    now_ms: u64,
) {
    for reader in accounts {
        let Some(selected) = weekly_reset_member(settings, accounts, reader, credentials) else {
            continue;
        };
        if !automations
            .tasks
            .iter()
            .any(|task| weekly_reset_rule_matches(task, reader, selected))
        {
            continue;
        }
        let Some(deadline) = deadlines.get_mut(&selected.account.id) else {
            continue;
        };
        deadline.reset_credits = QuotaSpendResets::from_snapshot(
            &reader.account.quota,
            now_ms,
            zenith_relay_core::QUOTA_STALE_AFTER_MS,
        );
    }
}

pub(super) fn sync_spend_deadlines(
    runtime: &zenith_relay_core::GatewayRuntime,
    store: &crate::local_pool::store::LocalPoolStore,
    references: &BTreeMap<String, CandidateQuotaReference>,
    credentials: &HashMap<String, Option<AccountCredentialFacts>>,
) {
    let mut deadlines = account_spend_deadlines(store.accounts(), current_time_ms());
    inherit_linked_deadlines(&mut deadlines, store.accounts(), references);
    apply_reset_budgets(
        &mut deadlines,
        store.gateway(),
        store.accounts(),
        store.automations(),
        credentials,
        current_time_ms(),
    );
    runtime.set_account_spend_deadlines(deadlines);
}

pub(in crate::local_pool) async fn refresh_spending_policy(
    state: &crate::local_pool::state::DesktopState,
) -> crate::local_pool::error::Result<()> {
    if let Some(runtime) = state.gateway.runtime().await {
        let store = state.store()?;
        super::sync_account_quota_references(state, &runtime, &store)?;
    }
    Ok(())
}
