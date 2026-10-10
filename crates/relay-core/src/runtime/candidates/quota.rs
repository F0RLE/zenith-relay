use super::super::GatewayRuntime;
use crate::error_codes;
use crate::quota::QuotaSnapshot;
use crate::{CandidateHealth, CandidateQuota, CandidateQuotaState, UsageEvent};
use reqwest::{header::HeaderMap, StatusCode};

const PASSIVE_QUOTA_PERSIST_DEBOUNCE_MS: u64 = 5_000;

impl GatewayRuntime {
    pub(crate) fn observe_codex_quota_headers(
        &self,
        candidate_id: &str,
        status: StatusCode,
        headers: &HeaderMap,
        observed_at_ms: u64,
    ) -> bool {
        if !(status.is_success()
            || status == StatusCode::SWITCHING_PROTOCOLS
            || status == StatusCode::TOO_MANY_REQUESTS)
            || !self.chatgpt_accounts.contains_key(candidate_id)
        {
            return false;
        }
        let mut quotas = crate::poison::mutex(&self.passive_quotas);
        let Some(passive_quota_state) = quotas.get_mut(candidate_id) else {
            return false;
        };
        let Some(merged) = crate::providers::chatgpt::merge_codex_quota_headers(
            &passive_quota_state.snapshot,
            headers,
            observed_at_ms,
        ) else {
            return false;
        };
        if merged == passive_quota_state.snapshot {
            return false;
        }
        let previous_quota = CandidateQuota::from_snapshot(
            &passive_quota_state.snapshot,
            observed_at_ms,
            self.quota_stale_after_ms,
        );
        let quota =
            CandidateQuota::from_snapshot(&merged, observed_at_ms, self.quota_stale_after_ms);
        passive_quota_state.force_persist |= previous_quota != quota
            && matches!(
                (previous_quota, quota),
                (CandidateQuota::Exhausted, _) | (_, CandidateQuota::Exhausted)
            );
        passive_quota_state.snapshot = merged;
        passive_quota_state.dirty = true;
        let updated = self.lock_scheduler().update_candidate_quota_at(
            candidate_id,
            quota,
            passive_quota_state.snapshot.updated_at_ms,
            passive_quota_state.snapshot.limiting_reset_at_ms(),
            passive_quota_state.snapshot.available_credits_micro_units,
            passive_quota_state.snapshot.provider_credits_unlimited,
        );
        drop(quotas);
        if updated {
            self.candidate_availability.notify_waiters();
        }
        updated
    }

    /// Publishes a complete quota refresh to both the passive header cache and
    /// the scheduler. Keeping the two views on one monotonic snapshot prevents
    /// a later response header merge from resurrecting stale quota or credits.
    pub(crate) fn sync_account_quota_snapshot(
        &self,
        candidate_id: &str,
        snapshot: &QuotaSnapshot,
        observed_at_ms: u64,
    ) -> bool {
        if !self.chatgpt_accounts.contains_key(candidate_id) {
            return false;
        }
        let mut quotas = crate::poison::mutex(&self.passive_quotas);
        let effective = quotas
            .get_mut(candidate_id)
            .map(|passive_quota_state| {
                reconcile_passive_quota_snapshot(passive_quota_state, snapshot, observed_at_ms)
            })
            .unwrap_or_else(|| snapshot.clone());
        let quota =
            CandidateQuota::from_snapshot(&effective, observed_at_ms, self.quota_stale_after_ms);
        let updated = self.lock_scheduler().update_candidate_quota_at(
            candidate_id,
            quota,
            effective.updated_at_ms,
            effective.limiting_reset_at_ms(),
            effective.available_credits_micro_units,
            effective.provider_credits_unlimited,
        );
        drop(quotas);
        if updated {
            self.candidate_availability.notify_waiters();
        }
        updated
    }

    /// Applies account policy and a complete refreshed quota snapshot while
    /// retaining the same passive-cache ordering as header observations.
    pub fn sync_account_availability_with_quota(
        &self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        snapshot: &QuotaSnapshot,
        observed_at_ms: u64,
    ) -> bool {
        self.sync_account_availability_with_quota_inner(
            candidate_id,
            enabled,
            health,
            snapshot,
            observed_at_ms,
            false,
        )
    }

    /// A refresh without a durable health transition is not evidence that a
    /// newer live auth or entitlement block recovered. Explicit transitions
    /// use the ordinary sync method instead.
    pub fn sync_account_refresh_availability_with_quota(
        &self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        snapshot: &QuotaSnapshot,
        observed_at_ms: u64,
    ) -> bool {
        self.sync_account_availability_with_quota_inner(
            candidate_id,
            enabled,
            health,
            snapshot,
            observed_at_ms,
            true,
        )
    }

    fn sync_account_availability_with_quota_inner(
        &self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        snapshot: &QuotaSnapshot,
        observed_at_ms: u64,
        preserve_live_block: bool,
    ) -> bool {
        if !self.chatgpt_accounts.contains_key(candidate_id) {
            return false;
        }
        let mut quotas = crate::poison::mutex(&self.passive_quotas);
        let effective = quotas
            .get_mut(candidate_id)
            .map(|passive_quota_state| {
                reconcile_passive_quota_snapshot(passive_quota_state, snapshot, observed_at_ms)
            })
            .unwrap_or_else(|| snapshot.clone());
        let quota_state = CandidateQuotaState {
            quota: CandidateQuota::from_snapshot(
                &effective,
                observed_at_ms,
                self.quota_stale_after_ms,
            ),
            updated_at_ms: effective.updated_at_ms,
            reset_at_ms: effective.limiting_reset_at_ms(),
            provider_credits_micro_units: effective.available_credits_micro_units,
            provider_credits_unlimited: effective.provider_credits_unlimited,
        };
        let mut scheduler = self.lock_scheduler();
        let effective_health = if preserve_live_block && health.is_eligible() {
            scheduler
                .candidate(candidate_id)
                .filter(|candidate| !candidate.health.is_eligible())
                .map_or(health, |candidate| candidate.health)
        } else {
            health
        };
        let updated = scheduler.update_candidate_availability_with_quota_at(
            candidate_id,
            enabled,
            effective_health,
            quota_state,
        );
        drop(scheduler);
        drop(quotas);
        if updated {
            self.candidate_availability.notify_waiters();
        }
        updated
    }

    pub(crate) fn take_passive_quota_snapshot(
        &self,
        candidate_id: &str,
        now_ms: u64,
    ) -> Option<QuotaSnapshot> {
        let mut quotas = crate::poison::mutex(&self.passive_quotas);
        let passive_quota_state = quotas.get_mut(candidate_id)?;
        if !passive_quota_state.dirty
            || (!passive_quota_state.force_persist
                && now_ms.saturating_sub(passive_quota_state.last_persist_hint_ms)
                    < PASSIVE_QUOTA_PERSIST_DEBOUNCE_MS)
        {
            return None;
        }
        passive_quota_state.dirty = false;
        passive_quota_state.force_persist = false;
        passive_quota_state.last_persist_hint_ms = now_ms;
        Some(passive_quota_state.snapshot.clone())
    }

    pub(crate) fn apply_usage_event(&self, event: &UsageEvent, observed_at_ms: u64) {
        let Some(candidate_id) = event.candidate_id.as_deref() else {
            return;
        };
        if let Some(snapshot) = event.quota_snapshot.as_ref() {
            self.sync_account_quota_snapshot(candidate_id, snapshot, observed_at_ms);
        }
        if event.success {
            self.set_candidate_health(candidate_id, CandidateHealth::Healthy);
            return;
        }
        if event.is_basis_points_transport_failure() {
            return;
        }

        let category = event.error_category.as_deref().unwrap_or_default();
        let model_id = if category == error_codes::IMAGE_GENERATION_NOT_ENABLED {
            event.requested_model.as_deref()
        } else {
            event
                .resolved_model
                .as_deref()
                .or(event.requested_model.as_deref())
        }
        .unwrap_or("*");
        // A direct API source may advertise a model while its upstream is
        // being replaced or temporarily unable to serve it. The request path
        // already applies a model-scoped cooldown for that failure; turning it
        // into a permanent capability block makes every later retry look like
        // there is no route at all. Native account capabilities are stable
        // enough to retain the explicit block until their catalog is refreshed.
        if event.account_id.is_some() && super::models::is_model_capability_failure(category) {
            self.block_candidate_capability(candidate_id, model_id);
            return;
        }
        if event.account_id.is_none() {
            return;
        }

        match category {
            // A bare rejection is not a complete quota snapshot. Do not invent
            // windows or a permanent limit. An already-open primary window is
            // zeroed so rotation stops using the stale remainder. The secondary
            // window stays untouched: a false weekly zero can spend a reset credit.
            error_codes::UPSTREAM_QUOTA_EXHAUSTED => {
                if event.quota_snapshot.is_none() {
                    self.note_reported_primary_exhaustion(candidate_id, observed_at_ms);
                }
            }
            error_codes::UPSTREAM_UNAUTHORIZED | error_codes::ACCOUNT_AUTH => {
                self.set_candidate_health(candidate_id, CandidateHealth::ReauthRequired);
            }
            error_codes::UPSTREAM_ACCOUNT_DISABLED => {
                self.set_candidate_health(candidate_id, CandidateHealth::Blocked);
            }
            error_codes::UPSTREAM_ACCOUNT_VERIFICATION_REQUIRED => {
                self.set_candidate_health(candidate_id, CandidateHealth::Checkpoint);
            }
            _ => {}
        }
    }

    fn note_reported_primary_exhaustion(&self, candidate_id: &str, observed_at_ms: u64) -> bool {
        if !self.chatgpt_accounts.contains_key(candidate_id) {
            return false;
        }
        let mut quotas = crate::poison::mutex(&self.passive_quotas);
        let Some(passive_quota_state) = quotas.get_mut(candidate_id) else {
            return false;
        };
        if !passive_quota_state
            .snapshot
            .note_reported_window_exhaustion(observed_at_ms)
        {
            return false;
        }
        passive_quota_state.dirty = true;
        passive_quota_state.force_persist = true;
        let quota = CandidateQuota::from_snapshot(
            &passive_quota_state.snapshot,
            observed_at_ms,
            self.quota_stale_after_ms,
        );
        let updated = self.lock_scheduler().update_candidate_quota_at(
            candidate_id,
            quota,
            passive_quota_state.snapshot.updated_at_ms,
            passive_quota_state.snapshot.limiting_reset_at_ms(),
            passive_quota_state.snapshot.available_credits_micro_units,
            passive_quota_state.snapshot.provider_credits_unlimited,
        );
        drop(quotas);
        if updated {
            self.candidate_availability.notify_waiters();
        }
        updated
    }
}

/// Merge a persisted refresh into the passive in-memory snapshot without
/// allowing an older in-flight observation to win. A dirty snapshot with the
/// same timestamp is retained because it may contain response headers that
/// have not reached durable storage yet.
fn reconcile_passive_quota_snapshot(
    passive_quota_state: &mut super::super::PassiveQuotaState,
    incoming: &QuotaSnapshot,
    observed_at_ms: u64,
) -> QuotaSnapshot {
    let incoming_at = incoming.updated_at_ms.unwrap_or(observed_at_ms);
    let current_at = passive_quota_state
        .snapshot
        .updated_at_ms
        .unwrap_or_default();
    let incoming_wins = match (
        incoming.updated_at_ms,
        passive_quota_state.snapshot.updated_at_ms,
    ) {
        (Some(incoming_at), Some(current_at)) if incoming_at < current_at => false,
        (Some(incoming_at), Some(current_at)) if incoming_at == current_at => {
            !passive_quota_state.dirty
        }
        (None, Some(_)) => false,
        _ => incoming_at >= current_at,
    };
    if incoming_wins {
        passive_quota_state.snapshot = incoming.clone();
        passive_quota_state.dirty = false;
        passive_quota_state.force_persist = false;
        passive_quota_state.last_persist_hint_ms = incoming.updated_at_ms.unwrap_or(observed_at_ms);
    }
    passive_quota_state.snapshot.clone()
}
