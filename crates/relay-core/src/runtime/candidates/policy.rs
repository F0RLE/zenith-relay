use super::super::{
    apply_candidate_policy, model_rules, GatewayRuntime, RuntimeCandidatePolicy,
    RuntimeSourcePolicyUpdate,
};
use crate::{CandidateKind, CandidateScope};
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

impl GatewayRuntime {
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
        if updates.iter().any(|update| {
            update.policy.weight == 0
                || update.recovery_delay_seconds > crate::MAX_SOURCE_RECOVERY_DELAY_SECONDS
        }) {
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

        let mut recovery_delays = crate::poison::mutex(&self.source_recovery_delays_ms);
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
        let mut current = crate::poison::write(&key.scope);
        if *current == scope {
            return true;
        }
        *current = scope;
        // The scope write lock makes this revision atomic with the permission
        // edit from the perspective of both reservation and final dispatch.
        key.scope_revision.fetch_add(1, Ordering::AcqRel);
        drop(current);
        self.candidate_availability.notify_waiters();
        true
    }
}
