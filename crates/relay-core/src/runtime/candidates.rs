use super::{
    apply_candidate_policy, confirmed_source_reasoning_levels, model_rules, runtime_now_ms,
    GatewayRuntime, RuntimeCandidatePolicy, RuntimeSourcePolicyUpdate,
};
use crate::{CandidateHealth, CandidateKind, CandidateQuota, CandidateScope};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;

impl GatewayRuntime {
    pub fn update_candidate_availability(
        &self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        quota: CandidateQuota,
    ) -> bool {
        self.lock_scheduler()
            .update_candidate_availability(candidate_id, enabled, health, quota)
    }

    pub fn update_candidate_availability_at(
        &self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        quota: CandidateQuota,
        quota_updated_at_ms: Option<u64>,
    ) -> bool {
        self.lock_scheduler().update_candidate_availability_at(
            candidate_id,
            enabled,
            health,
            quota,
            quota_updated_at_ms,
        )
    }

    /// Applies source routing rules without rebuilding its HTTP executor.
    ///
    /// A source can have more than one protocol binding, so every matching
    /// candidate must receive the same policy atomically from the scheduler's
    /// point of view.
    pub fn update_source_policy(
        &self,
        source_id: &str,
        policy: RuntimeCandidatePolicy,
        recovery_delay_seconds: u64,
    ) -> bool {
        self.update_source_policies(&[RuntimeSourcePolicyUpdate {
            source_id: source_id.to_string(),
            policy,
            recovery_delay_seconds,
        }])
    }

    /// Applies several source policies as one scheduler update. This keeps a
    /// reordered fallback group consistent even when its sources expose
    /// multiple protocol bindings.
    pub fn update_source_policies(&self, updates: &[RuntimeSourcePolicyUpdate]) -> bool {
        if updates
            .iter()
            .any(|update| update.policy.weight == 0 || update.recovery_delay_seconds > 24 * 60 * 60)
        {
            return false;
        }
        let mut seen = BTreeSet::new();
        if updates
            .iter()
            .any(|update| !seen.insert(update.source_id.as_str()))
        {
            return false;
        }

        let mut scheduler = self.lock_scheduler();
        let mut candidates = Vec::new();
        let mut recovery_updates = Vec::new();
        for update in updates {
            let rules = model_rules(
                &update.policy.allowed_models,
                &update.policy.excluded_models,
            );
            let mut matched = false;
            for (candidate_id, binding) in &self.source_candidate_bindings {
                if binding.source_id != update.source_id {
                    continue;
                }
                matched = true;
                let Some(mut candidate) = scheduler.candidate(candidate_id).cloned() else {
                    return false;
                };
                if candidate.kind != CandidateKind::ApiSource
                    || candidate.source_id != update.source_id
                {
                    return false;
                }
                apply_candidate_policy(&mut candidate, &update.policy, &rules);
                recovery_updates.push((candidate_id.clone(), update.recovery_delay_seconds));
                candidates.push(candidate);
            }
            if !matched {
                return false;
            }
        }
        for candidate in candidates {
            scheduler.upsert(candidate);
        }
        drop(scheduler);

        let mut recovery_delays = self
            .source_recovery_delays_ms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (candidate_id, recovery_delay_seconds) in recovery_updates {
            if recovery_delay_seconds == 0 {
                recovery_delays.remove(&candidate_id);
            } else {
                recovery_delays.insert(candidate_id, recovery_delay_seconds.saturating_mul(1_000));
            }
        }
        drop(recovery_delays);
        self.candidate_availability.notify_waiters();
        true
    }

    /// Applies an account's scheduling policy without replacing its OAuth
    /// executor or interrupting in-flight streams.
    pub fn update_account_policy(&self, account_id: &str, policy: RuntimeCandidatePolicy) -> bool {
        if policy.weight == 0 {
            return false;
        }
        let rules = model_rules(&policy.allowed_models, &policy.excluded_models);
        let mut scheduler = self.lock_scheduler();
        let Some(mut candidate) = scheduler.candidate(account_id).cloned() else {
            return false;
        };
        if candidate.kind != CandidateKind::OAuthAccount
            || candidate.account_id.as_deref() != Some(account_id)
        {
            return false;
        }
        apply_candidate_policy(&mut candidate, &policy, &rules);
        scheduler.upsert(candidate);
        drop(scheduler);
        self.candidate_availability.notify_waiters();
        true
    }

    pub fn update_key_scope(&self, key_id: &str, scope: CandidateScope) -> bool {
        let Some(key) = self.keys.iter().find(|key| key.enabled && key.id == key_id) else {
            return false;
        };
        let mut current = key
            .scope
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *current == scope {
            return true;
        }
        *current = scope;
        drop(current);
        self.candidate_availability.notify_waiters();
        true
    }

    pub fn set_candidate_health(&self, candidate_id: &str, health: CandidateHealth) -> bool {
        self.lock_scheduler()
            .set_candidate_health(candidate_id, health)
    }

    pub fn remove_candidate(&self, candidate_id: &str) -> bool {
        let removed = self.lock_scheduler().remove(candidate_id).is_some();
        self.model_metadata
            .codex_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(candidate_id);
        self.model_metadata
            .source_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(candidate_id);
        let previous_levels = self
            .model_metadata
            .confirmed_reasoning_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let confirmed_levels = {
            let mut efforts = self
                .model_metadata
                .confirmed_reasoning_efforts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for routes in efforts.values_mut() {
                routes.remove(candidate_id);
            }
            efforts.retain(|_, routes| !routes.is_empty());
            confirmed_source_reasoning_levels(&efforts, &previous_levels, &BTreeMap::new())
        };
        *self
            .model_metadata
            .confirmed_reasoning_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = confirmed_levels;
        self.passive_quotas
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(candidate_id);
        if let Some(account) = self.chatgpt_accounts.get(candidate_id) {
            account.active.store(false, Ordering::Release);
            *account
                .agent_identity
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
        if let Some(store) = self.response_affinity_store.as_ref() {
            let _ = store.delete_candidate(candidate_id);
        }
        removed
    }

    pub fn candidate_runtime_order(&self) -> Vec<crate::CandidateRuntimeSnapshot> {
        self.lock_scheduler().runtime_order(runtime_now_ms())
    }

    pub(crate) fn account_candidate_is_active(&self, candidate_id: &str) -> bool {
        self.chatgpt_accounts
            .get(candidate_id)
            .is_some_and(|account| account.active.load(Ordering::Acquire))
    }

    pub fn set_protected_candidate(
        &self,
        candidate_id: Option<&str>,
        reserve_basis_points: u64,
    ) -> bool {
        self.lock_scheduler()
            .set_protected_candidate(candidate_id, reserve_basis_points)
    }

    pub fn clear_candidate_cooldown(&self, candidate_id: &str, model: &str) -> bool {
        self.lock_scheduler().clear_cooldown(candidate_id, model)
    }

    pub fn set_candidate_cooldown(
        &self,
        candidate_id: &str,
        model: &str,
        retry_at_ms: u64,
    ) -> bool {
        self.lock_scheduler()
            .set_cooldown(candidate_id, model, retry_at_ms)
    }

    pub fn reset_candidate_failures(&self, candidate_id: &str) -> bool {
        self.lock_scheduler().reset_failures(candidate_id)
    }
}
