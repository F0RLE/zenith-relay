use super::{
    apply_candidate_policy, declared_source_reasoning_levels, model_rules, runtime_now_ms,
    ExecutionFence, GatewayRuntime, RuntimeCandidatePolicy, RuntimeSourcePolicyUpdate,
};
use crate::quota::QuotaSnapshot;
use crate::{CandidateHealth, CandidateKind, CandidateQuota, CandidateScope, UsageEvent};
use reqwest::{header::HeaderMap, StatusCode};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};

const PASSIVE_QUOTA_PERSIST_DEBOUNCE_MS: u64 = 5_000;

impl GatewayRuntime {
    /// Immediately blocks sibling OAuth candidates that share the same
    /// ChatGPT Team/workspace identity. This is intentionally an in-memory
    /// circuit breaker; the owning local/server store persists the triggering
    /// request through the normal usage callback.
    pub(crate) fn trip_chatgpt_team_breaker(&self, candidate_id: &str, now_ms: u64) -> bool {
        let team_key = self
            .chatgpt_team_members
            .iter()
            .find_map(|(team, members)| members.contains(candidate_id).then_some(team.clone()));
        let Some(team_key) = team_key else {
            return false;
        };
        {
            let mut recent = self
                .chatgpt_team_breaker_recent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if recent.get(&team_key).is_some_and(|until| *until > now_ms) {
                return false;
            }
            recent.retain(|_, until| *until > now_ms);
            recent.insert(
                team_key.clone(),
                now_ms.saturating_add(super::CHATGPT_TEAM_BREAKER_DEDUP_MS),
            );
        }
        let siblings = self
            .chatgpt_team_members
            .get(&team_key)
            .into_iter()
            .flat_map(|members| members.iter())
            .filter(|member| member.as_str() != candidate_id)
            .cloned()
            .collect::<Vec<_>>();
        let mut changed = false;
        for sibling in &siblings {
            changed |= self.set_candidate_health(sibling, CandidateHealth::Blocked);
        }
        if let Ok(callback) = self.chatgpt_team_breaker_callback.lock() {
            callback(siblings.clone());
        }
        changed
    }

    pub(crate) fn fence_execution(&self, candidate_id: &str) -> Option<ExecutionFence> {
        self.lock_scheduler()
            .set_execution_fence(candidate_id, true)
            .then(|| ExecutionFence {
                scheduler: self.scheduler.clone(),
                candidate_id: candidate_id.to_string(),
                released: AtomicBool::new(false),
            })
    }

    pub(crate) fn block_candidate_capability(&self, candidate_id: &str, model: &str) -> bool {
        self.lock_scheduler().block_capability(candidate_id, model)
    }

    pub(crate) fn clear_candidate_capability_blocks(&self, candidate_id: &str) -> bool {
        self.lock_scheduler().clear_capability_blocks(candidate_id)
    }

    pub(crate) fn record_provider_rate_limit(
        &self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
    ) -> bool {
        self.lock_scheduler()
            .record_provider_rate_limit(candidate_id, model, now_ms)
    }

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
        let mut quotas = self
            .passive_quotas
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = quotas.get_mut(candidate_id) else {
            return false;
        };
        let Some(merged) = crate::providers::chatgpt::merge_codex_quota_headers(
            &state.snapshot,
            headers,
            observed_at_ms,
        ) else {
            return false;
        };
        if merged == state.snapshot {
            return false;
        }
        let previous_quota = CandidateQuota::from_snapshot(
            &state.snapshot,
            observed_at_ms,
            self.quota_stale_after_ms,
        );
        let quota =
            CandidateQuota::from_snapshot(&merged, observed_at_ms, self.quota_stale_after_ms);
        state.force_persist |= previous_quota != quota
            && matches!(
                (previous_quota, quota),
                (CandidateQuota::Exhausted, _) | (_, CandidateQuota::Exhausted)
            );
        state.snapshot = merged;
        state.dirty = true;
        self.lock_scheduler().update_candidate_quota_at(
            candidate_id,
            quota,
            state.snapshot.updated_at_ms,
            state.snapshot.limiting_reset_at_ms(),
        )
    }

    pub(crate) fn take_passive_quota_snapshot(
        &self,
        candidate_id: &str,
        now_ms: u64,
    ) -> Option<QuotaSnapshot> {
        let mut quotas = self
            .passive_quotas
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = quotas.get_mut(candidate_id)?;
        if !state.dirty
            || (!state.force_persist
                && now_ms.saturating_sub(state.last_persist_hint_ms)
                    < PASSIVE_QUOTA_PERSIST_DEBOUNCE_MS)
        {
            return None;
        }
        state.dirty = false;
        state.force_persist = false;
        state.last_persist_hint_ms = now_ms;
        Some(state.snapshot.clone())
    }

    pub(crate) fn apply_usage_event(&self, event: &UsageEvent, observed_at_ms: u64) {
        let Some(candidate_id) = event.candidate_id.as_deref() else {
            return;
        };
        if let Some(snapshot) = event.quota_snapshot.as_ref() {
            self.lock_scheduler().update_candidate_quota_at(
                candidate_id,
                CandidateQuota::from_snapshot(snapshot, observed_at_ms, self.quota_stale_after_ms),
                snapshot.updated_at_ms,
                snapshot.limiting_reset_at_ms(),
            );
        }
        if event.success {
            self.set_candidate_health(candidate_id, CandidateHealth::Healthy);
            return;
        }

        let category = event.error_category.as_deref().unwrap_or_default();
        let model = if category == "image_generation_not_enabled" {
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
        if event.account_id.is_some() && is_model_capability_failure(category) {
            self.block_candidate_capability(candidate_id, model);
            return;
        }
        if event.account_id.is_none() {
            if event.http_status == StatusCode::TOO_MANY_REQUESTS.as_u16() {
                self.record_provider_rate_limit(candidate_id, model, observed_at_ms);
            }
            return;
        }

        match category {
            // The gateway already applied a candidate-scoped cooldown before
            // emitting this event. A bare 429 is not a durable quota snapshot:
            // treating it as `Exhausted` keeps an otherwise healthy slot out
            // of rotation until a separate refresh happens to run. Only an
            // actual quota snapshot above may mark the candidate exhausted.
            "upstream_quota_exhausted" => {}
            "upstream_unauthorized" | "account_auth" => {
                self.set_candidate_health(candidate_id, CandidateHealth::ReauthRequired);
            }
            "upstream_account_disabled" => {
                self.set_candidate_health(candidate_id, CandidateHealth::Blocked);
            }
            "upstream_account_verification_required" => {
                self.set_candidate_health(candidate_id, CandidateHealth::Checkpoint);
            }
            _ => {}
        }
    }

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

    /// Builds a scope from all healthy pool protocols without reopening secrets
    /// or replacing a runtime that owns active streams. Request admission still
    /// filters candidates by the caller's selected wire API.
    pub fn active_pool_scope(
        &self,
        allowed_source_ids: &BTreeSet<String>,
        allowed_account_ids: &BTreeSet<String>,
    ) -> CandidateScope {
        let mut source_ids = BTreeSet::new();
        let mut account_ids = BTreeSet::new();
        for candidate in self.lock_scheduler().candidates() {
            if !candidate.enabled || candidate.draining || !candidate.secret_available {
                continue;
            }
            match candidate.kind {
                CandidateKind::ApiSource if allowed_source_ids.contains(&candidate.source_id) => {
                    source_ids.insert(candidate.source_id.clone());
                }
                CandidateKind::OAuthAccount => {
                    if let Some(account_id) = candidate
                        .account_id
                        .as_ref()
                        .filter(|id| allowed_account_ids.contains(*id))
                    {
                        account_ids.insert(account_id.clone());
                    }
                }
                _ => {}
            }
        }
        CandidateScope {
            source_ids: Some(source_ids),
            account_ids: Some(account_ids),
            model_rules: Default::default(),
        }
    }

    /// Backward-compatible name for callers that historically built a pool
    /// scope for the Responses-only desktop profile.
    pub fn active_responses_scope(
        &self,
        allowed_source_ids: &BTreeSet<String>,
        allowed_account_ids: &BTreeSet<String>,
    ) -> CandidateScope {
        self.active_pool_scope(allowed_source_ids, allowed_account_ids)
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
        {
            let mut declared = self
                .model_metadata
                .declared_reasoning
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for routes in declared.efforts.values_mut() {
                routes.remove(candidate_id);
            }
            declared.efforts.retain(|_, routes| !routes.is_empty());
            for routes in declared.empty_routes.values_mut() {
                routes.remove(candidate_id);
            }
            declared.empty_routes.retain(|_, routes| !routes.is_empty());
            let previous_levels = declared.levels.clone();
            declared.levels = declared_source_reasoning_levels(
                &declared.efforts,
                &previous_levels,
                &BTreeMap::new(),
            );
        }
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

fn is_model_capability_failure(category: &str) -> bool {
    matches!(
        category,
        "upstream_model_not_found"
            | "upstream_model_unsupported"
            | "upstream_usage_not_included"
            | "image_generation_not_enabled"
    )
}
