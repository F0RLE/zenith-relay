use super::capacity::CandidateQuota;
use super::cooldown::active_retry_at;
use crate::error_codes;
use crate::{
    accounts::{AccountAuthState, AccountHealthState},
    quota::SubscriptionStatus,
    ModelRules, WireApi,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    ApiSource,
    OAuthAccount,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateHealth {
    #[default]
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
    ReauthRequired,
    Checkpoint,
    Captcha,
    Blocked,
    Expired,
}

impl CandidateHealth {
    pub fn is_eligible(self) -> bool {
        matches!(self, Self::Unknown | Self::Healthy | Self::Degraded)
    }
}

/// The complete quota data reported for one runtime candidate.
///
/// Keeping the timestamps and provider-credit ledger together prevents a
/// refresh from publishing only part of the provider snapshot to the scheduler.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CandidateQuotaState {
    pub quota: CandidateQuota,
    pub updated_at_ms: Option<u64>,
    pub reset_at_ms: Option<u64>,
    pub provider_credits_micro_units: Option<u64>,
    pub provider_credits_unlimited: bool,
    pub provider_credits_observed_at_ms: Option<u64>,
}

impl CandidateQuotaState {
    pub fn from_snapshot(
        snapshot: &crate::quota::QuotaSnapshot,
        now_ms: u64,
        stale_after_ms: u64,
    ) -> Self {
        Self {
            quota: CandidateQuota::from_snapshot(snapshot, now_ms, stale_after_ms),
            updated_at_ms: snapshot.quota_observed_at_ms(),
            reset_at_ms: snapshot.limiting_reset_at_ms(),
            provider_credits_micro_units: snapshot
                .available_credits_micro_units
                .filter(|_| snapshot.has_usable_provider_credits()),
            provider_credits_unlimited: snapshot.has_usable_provider_credits()
                && snapshot.provider_credits_unlimited,
            provider_credits_observed_at_ms: snapshot.provider_credits_observed_at_ms,
        }
    }
}

pub fn account_candidate_health(
    auth_state: AccountAuthState,
    health: AccountHealthState,
    subscription_status: SubscriptionStatus,
    last_error_code: Option<&str>,
) -> CandidateHealth {
    if auth_state.requires_fresh_login() {
        return CandidateHealth::ReauthRequired;
    }
    if matches!(auth_state, AccountAuthState::Error) {
        return CandidateHealth::Unhealthy;
    }
    match last_error_code {
        Some("checkpoint" | error_codes::UPSTREAM_ACCOUNT_VERIFICATION_REQUIRED) => {
            return CandidateHealth::Checkpoint
        }
        Some("captcha") => return CandidateHealth::Captcha,
        _ => {}
    }
    // An expired entitlement is informational until the Codex path confirms
    // that access is actually denied. ChatGPT Team/Business can keep serving
    // Codex after the UI entitlement date becomes stale, while a forbidden
    // subscription is an explicit upstream block and remains terminal.
    if matches!(subscription_status, SubscriptionStatus::Forbidden) {
        return CandidateHealth::Blocked;
    }
    match health {
        AccountHealthState::Unknown => CandidateHealth::Unknown,
        AccountHealthState::Healthy => CandidateHealth::Healthy,
        AccountHealthState::Degraded => CandidateHealth::Degraded,
        AccountHealthState::Unhealthy => CandidateHealth::Unhealthy,
        AccountHealthState::Blocked => CandidateHealth::Blocked,
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateScope {
    pub source_ids: Option<BTreeSet<String>>,
    pub account_ids: Option<BTreeSet<String>>,
    pub model_rules: ModelRules,
}

impl CandidateScope {
    fn includes(&self, candidate: &RuntimeCandidate) -> bool {
        if self.source_ids.is_none() && self.account_ids.is_none() {
            return true;
        }
        self.source_ids
            .as_ref()
            .is_some_and(|ids| ids.contains(&candidate.source_id))
            || candidate.account_id.as_ref().is_some_and(|account_id| {
                self.account_ids
                    .as_ref()
                    .is_some_and(|account_ids| account_ids.contains(account_id))
            })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCandidate {
    pub id: String,
    pub kind: CandidateKind,
    pub source_id: String,
    pub account_id: Option<String>,
    /// Legacy default, retained for older serialized candidates.
    pub protocol: WireApi,
    /// Exact model/client contracts of one physical member. Empty keeps the
    /// legacy single-protocol behavior; independent unions are insufficient.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_protocols: BTreeMap<String, BTreeSet<WireApi>>,
    pub enabled: bool,
    pub draining: bool,
    pub priority: i32,
    pub weight: u32,
    pub models: BTreeSet<String>,
    pub model_rules: ModelRules,
    pub health: CandidateHealth,
    pub quota: CandidateQuota,
    /// Fresh provider-reported credits are a secondary routing signal. They
    /// remain separate from percentage quota so diagnostics describe the
    /// actual rate-limit window.
    pub provider_credits_micro_units: Option<u64>,
    pub provider_credits_unlimited: bool,
    #[serde(default)]
    pub provider_credits_observed_at_ms: Option<u64>,
    pub quota_updated_at_ms: Option<u64>,
    pub quota_reset_at_ms: Option<u64>,
    pub cooldowns: BTreeMap<String, u64>,
    pub last_used_at: Option<u64>,

    pub secret_available: bool,
}

impl RuntimeCandidate {
    pub(crate) fn set_quota_state(&mut self, state: CandidateQuotaState) {
        self.quota = state.quota;
        self.quota_updated_at_ms = state.updated_at_ms;
        self.quota_reset_at_ms = state.reset_at_ms;
        self.provider_credits_micro_units = state.provider_credits_micro_units;
        self.provider_credits_unlimited = state.provider_credits_unlimited;
        self.provider_credits_observed_at_ms = state.provider_credits_observed_at_ms;
    }

    pub fn is_eligible(
        &self,
        model: &str,
        allowed_protocols: &[WireApi],
        scope: &CandidateScope,
        now_ms: u64,
    ) -> bool {
        self.is_visible(model, allowed_protocols, scope)
            && active_retry_at(&self.cooldowns, model, now_ms).is_none()
    }

    pub fn is_visible(
        &self,
        model: &str,
        allowed_protocols: &[WireApi],
        scope: &CandidateScope,
    ) -> bool {
        self.is_catalog_visible(model, allowed_protocols, scope)
            && (self.quota.is_eligible()
                || (self.kind == CandidateKind::OAuthAccount
                    && self.quota == CandidateQuota::Stale))
    }

    pub(crate) fn is_catalog_visible(
        &self,
        model: &str,
        allowed_protocols: &[WireApi],
        scope: &CandidateScope,
    ) -> bool {
        self.is_configured(model, allowed_protocols, scope) && self.health.is_eligible()
    }

    pub(crate) fn is_discovery_visible(&self, scope: &CandidateScope) -> bool {
        self.enabled
            && !self.draining
            && self.secret_available
            && self.health.is_eligible()
            && scope.includes(self)
    }

    pub(crate) fn is_configured(
        &self,
        model: &str,
        allowed_protocols: &[WireApi],
        scope: &CandidateScope,
    ) -> bool {
        self.enabled
            && !self.draining
            && self.secret_available
            && self.supports_protocol(model, allowed_protocols)
            && self.supports_model(model)
            && self.model_rules.allows(model)
            && scope.includes(self)
            && scope.model_rules.allows(model)
    }

    pub fn retry_at_if_configured(
        &self,
        model: &str,
        allowed_protocols: &[WireApi],
        scope: &CandidateScope,
        now_ms: u64,
    ) -> Option<u64> {
        self.is_catalog_visible(model, allowed_protocols, scope)
            .then(|| active_retry_at(&self.cooldowns, model, now_ms))
            .flatten()
    }

    pub(crate) fn supports_model(&self, model: &str) -> bool {
        self.models
            .iter()
            .any(|candidate_model| candidate_model.eq_ignore_ascii_case(model))
    }

    pub(crate) fn supports_protocol(&self, model: &str, allowed: &[WireApi]) -> bool {
        if !self.model_protocols.is_empty() {
            return self
                .model_protocols
                .get(&crate::model_id_key(model))
                .is_some_and(|protocols| {
                    allowed.iter().any(|protocol| protocols.contains(protocol))
                });
        }
        allowed.contains(&self.protocol)
            || (self.kind == CandidateKind::OAuthAccount
                && self.protocol == WireApi::Responses
                && !allowed.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::ReauthReason;

    #[test]
    fn account_health_has_one_precedence_for_all_runtimes() {
        assert_eq!(
            account_candidate_health(
                AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant),
                AccountHealthState::Healthy,
                SubscriptionStatus::Active,
                None,
            ),
            CandidateHealth::ReauthRequired
        );
        assert_eq!(
            account_candidate_health(
                AccountAuthState::RequiresReauth(ReauthReason::ReusedRefreshToken),
                AccountHealthState::Healthy,
                SubscriptionStatus::Active,
                None,
            ),
            CandidateHealth::Healthy
        );
        assert_eq!(
            account_candidate_health(
                AccountAuthState::Active,
                AccountHealthState::Healthy,
                SubscriptionStatus::Active,
                Some("checkpoint"),
            ),
            CandidateHealth::Checkpoint
        );
        assert_eq!(
            account_candidate_health(
                AccountAuthState::Active,
                AccountHealthState::Healthy,
                SubscriptionStatus::Expired,
                None,
            ),
            CandidateHealth::Healthy
        );
        assert_eq!(
            account_candidate_health(
                AccountAuthState::Active,
                AccountHealthState::Healthy,
                SubscriptionStatus::Forbidden,
                None,
            ),
            CandidateHealth::Blocked
        );
    }
}
