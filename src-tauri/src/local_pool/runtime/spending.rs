use super::current_time_ms;
use crate::local_pool::models::LocalAccountRecord;
use std::collections::BTreeMap;
use zenith_relay_core::{
    providers::chatgpt::SUBSCRIPTION_REFRESH_INTERVAL_MS, quota::QuotaSpendDeadline,
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

pub(super) fn sync_spend_deadlines(
    runtime: &zenith_relay_core::GatewayRuntime,
    store: &crate::local_pool::store::LocalPoolStore,
) {
    runtime
        .set_account_spend_deadlines(account_spend_deadlines(store.accounts(), current_time_ms()));
}
