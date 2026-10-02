mod auth;
mod identity;
mod usage;

#[cfg(test)]
mod tests;

use crate::quota::{QuotaSnapshot, Subscription};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub use auth::{
    apply_model_discovery_failure, automatic_quota_monitoring_eligible, clear_false_upstream_block,
    provider_account_failure, recover_model_discovery_state, AccountAuthMode, AccountAuthState,
    AccountHealthState, ProviderAccountFailure, ReauthReason,
};
pub use identity::AccountIdentity;
pub use usage::{
    reduce_account_usage, AccountAccessState, AccountUsageObservation, AccountUsageState,
    AccountUsageUpdate,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRecord {
    pub id: String,
    pub label: String,
    pub identity: AccountIdentity,
    pub auth_mode: AccountAuthMode,
    pub auth_state: AccountAuthState,
    pub health: AccountHealthState,
    pub source_id: String,
    pub secret_refs: Vec<String>,
    pub subscription: Subscription,
    pub quota: QuotaSnapshot,
    pub token_generation: u64,
    pub token_updated_at_ms: Option<u64>,
    pub tags: BTreeSet<String>,
    pub enabled: bool,
    #[serde(default)]
    pub in_pool: bool,
    pub draining: bool,
    pub created_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
    pub last_error_code: Option<String>,
}

impl AccountRecord {
    pub fn is_automatic_quota_monitoring_eligible(&self) -> bool {
        automatic_quota_monitoring_eligible(self.enabled, self.auth_state)
    }

    pub fn is_wake_eligible(&self) -> bool {
        self.enabled
            && self.in_pool
            && !self.draining
            && self.auth_state == AccountAuthState::Active
            && self.health == AccountHealthState::Healthy
    }
}

crate::impl_account_operational_source!(AccountRecord);
