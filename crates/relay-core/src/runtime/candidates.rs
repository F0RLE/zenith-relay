use super::{
    apply_candidate_policy, images::select_image_main_model_with_catalog, model_rules,
    normalized_set, runtime_now_ms, AccountModelInventory, ExecutionFence, GatewayRuntime,
    RuntimeCandidatePolicy, RuntimeSourcePolicyUpdate, IMAGE_API_MODEL,
};
use crate::error_codes;
use crate::quota::QuotaSnapshot;
use crate::{
    CandidateHealth, CandidateKind, CandidateQuota, CandidateQuotaState, CandidateScope, UsageEvent,
};
use reqwest::{header::HeaderMap, StatusCode};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};

const PASSIVE_QUOTA_PERSIST_DEBOUNCE_MS: u64 = 5_000;

impl GatewayRuntime {
    /// Reconcile only the discovered OAuth model inventory. Keep this runtime's
    /// scheduler and executor identity: an inventory read is not fresh auth,
    /// quota or health evidence and must not discard live physical leases.
    /// The scheduler lock also serializes this edit with final dispatch and
    /// model-list projection; old unstarted leases are rechecked there.
    pub fn update_account_models(&self, account_id: &str, models: &[String]) -> bool {
        let Some(account) = self.chatgpt_accounts.get(account_id) else {
            return false;
        };
        let configured_models = normalized_set(models.iter());
        let image_main_model = select_image_main_model_with_catalog(
            &configured_models,
            self.image_base_model.as_deref(),
            self.image_pricing_catalog.as_deref(),
        );
        let mut candidate_models = configured_models.clone();
        let mut published_models = models.to_vec();
        if image_main_model.is_some() {
            candidate_models.insert(IMAGE_API_MODEL.to_string());
            published_models.push(IMAGE_API_MODEL.to_string());
        }
        let mut scheduler = self.lock_scheduler();
        if scheduler.is_retired() {
            return false;
        }
        let Some(mut candidate) = scheduler.candidate(account_id).cloned() else {
            return false;
        };
        if candidate.kind != CandidateKind::OAuthAccount
            || candidate.account_id.as_deref() != Some(account_id)
        {
            return false;
        }
        candidate.models = candidate_models;
        // Never publish a model that the executor cannot resolve. Hold the
        // scheduler lock until all three views name the same inventory.
        let mut inventory = account
            .model_inventory
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let changed = inventory.configured_models != configured_models;
        let image_bridge_changed = inventory.image_main_model != image_main_model;
        *inventory = AccountModelInventory {
            configured_models,
            image_main_model,
        };
        drop(inventory);
        if image_bridge_changed {
            // A virtual image route retains its public model id even when its
            // underlying Responses model changes. Revoke pending old leases.
            account.image_bridge_revision.fetch_add(1, Ordering::AcqRel);
        }
        self.registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(account_id, published_models.iter());
        scheduler.upsert(candidate);
        if changed {
            // Transport cards and Lite support describe the old inventory.
            // A removed and later reintroduced slug needs fresh evidence.
            self.model_metadata
                .codex_manifests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(account_id);
            self.codex_responses_lite_models
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|(id, _)| id != account_id);
        }
        drop(scheduler);
        self.candidate_availability.notify_waiters();
        self.admission_changed.notify_waiters();
        true
    }

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

    /// Hold while a host commits a credential or permission edit and applies
    /// it to this runtime. Acquire before the durable edit; release only after
    /// the new runtime state is published or the old state is restored.
    pub fn fence_candidate_dispatch(&self, candidate_id: &str) -> Option<ExecutionFence> {
        let epoch = self.lock_scheduler().begin_execution_fence(candidate_id)?;
        self.candidate_availability.notify_waiters();
        Some(ExecutionFence {
            scheduler: self.scheduler.clone(),
            availability: self.candidate_availability.clone(),
            candidate_id: candidate_id.to_string(),
            epoch,
            released: AtomicBool::new(false),
        })
    }

    /// Fence every physical protocol route of one API source while its host
    /// changes the source's credential, endpoint or permission policy.
    pub fn fence_source_dispatch(&self, source_id: &str) -> Vec<ExecutionFence> {
        self.source_candidate_bindings
            .iter()
            .filter(|(_, binding)| binding.source_id == source_id)
            .filter_map(|(candidate_id, _)| self.fence_candidate_dispatch(candidate_id))
            .collect()
    }

    pub(crate) fn fence_execution(&self, candidate_id: &str) -> Option<ExecutionFence> {
        self.fence_candidate_dispatch(candidate_id)
    }

    pub(crate) fn block_candidate_capability(&self, candidate_id: &str, model: &str) -> bool {
        let changed = self.lock_scheduler().block_capability(candidate_id, model);
        if changed {
            self.candidate_availability.notify_waiters();
        }
        changed
    }

    pub(crate) fn clear_candidate_capability_blocks(&self, candidate_id: &str) -> bool {
        let changed = self.lock_scheduler().clear_capability_blocks(candidate_id);
        if changed {
            self.candidate_availability.notify_waiters();
        }
        changed
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
        let updated = self.lock_scheduler().update_candidate_quota_at(
            candidate_id,
            quota,
            state.snapshot.updated_at_ms,
            state.snapshot.limiting_reset_at_ms(),
            state.snapshot.available_credits_micro_units,
            state.snapshot.provider_credits_unlimited,
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
        let mut quotas = self
            .passive_quotas
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let effective = quotas
            .get_mut(candidate_id)
            .map(|state| reconcile_passive_quota_snapshot(state, snapshot, observed_at_ms))
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
        let mut quotas = self
            .passive_quotas
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let effective = quotas
            .get_mut(candidate_id)
            .map(|state| reconcile_passive_quota_snapshot(state, snapshot, observed_at_ms))
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
            self.sync_account_quota_snapshot(candidate_id, snapshot, observed_at_ms);
        }
        if event.success {
            self.set_candidate_health(candidate_id, CandidateHealth::Healthy);
            return;
        }

        let category = event.error_category.as_deref().unwrap_or_default();
        let model = if category == error_codes::IMAGE_GENERATION_NOT_ENABLED {
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
            return;
        }

        match category {
            // The gateway already applied a candidate-scoped cooldown before
            // emitting this event. A bare 429 is not a durable quota snapshot:
            // treating it as `Exhausted` keeps an otherwise healthy slot out
            // of rotation until a separate refresh happens to run. Only an
            // actual quota snapshot above may mark the candidate exhausted.
            error_codes::UPSTREAM_QUOTA_EXHAUSTED => {}
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

    pub fn update_candidate_availability(
        &self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        quota: CandidateQuota,
    ) -> bool {
        let updated = self.lock_scheduler().update_candidate_availability(
            candidate_id,
            enabled,
            health,
            quota,
        );
        if updated {
            self.candidate_availability.notify_waiters();
        }
        updated
    }

    pub fn update_candidate_availability_at(
        &self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        quota: CandidateQuota,
        quota_updated_at_ms: Option<u64>,
    ) -> bool {
        let updated = self.lock_scheduler().update_candidate_availability_at(
            candidate_id,
            enabled,
            health,
            quota,
            quota_updated_at_ms,
        );
        if updated {
            self.candidate_availability.notify_waiters();
        }
        updated
    }

    /// Applies the complete refreshed account state without rebuilding the
    /// gateway. Quota windows and provider credits originate from one provider
    /// response and must reach the scheduler together.
    pub fn update_candidate_availability_with_quota_at(
        &self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        quota_state: CandidateQuotaState,
    ) -> bool {
        let updated = self
            .lock_scheduler()
            .update_candidate_availability_with_quota_at(
                candidate_id,
                enabled,
                health,
                quota_state,
            );
        if updated {
            self.candidate_availability.notify_waiters();
        }
        updated
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
        // The scope write lock makes this revision atomic with the permission
        // edit from the perspective of both reservation and final dispatch.
        key.scope_revision.fetch_add(1, Ordering::AcqRel);
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
        let updated = self
            .lock_scheduler()
            .set_candidate_health(candidate_id, health);
        if updated {
            self.candidate_availability.notify_waiters();
        }
        updated
    }

    pub fn remove_candidate(&self, candidate_id: &str) -> bool {
        let candidate_ids = {
            let source_routes = self
                .source_candidate_bindings
                .iter()
                .filter(|(_, binding)| binding.source_id == candidate_id)
                .map(|(route_id, _)| route_id.clone())
                .collect::<Vec<_>>();
            if source_routes.is_empty() {
                vec![candidate_id.to_string()]
            } else {
                source_routes
            }
        };
        // Scheduler removal is graceful when a lease is active: keep the
        // executor alive until the request reaches its terminal outcome.
        let (removed, deferred) = {
            let mut scheduler = self.lock_scheduler();
            let mut removed = false;
            let mut deferred = BTreeSet::new();
            for route_id in &candidate_ids {
                removed |= scheduler.remove(route_id).is_some();
                if scheduler.candidate(route_id).is_some() {
                    deferred.insert(route_id.clone());
                }
            }
            (removed, deferred)
        };
        {
            let mut manifests = self
                .model_metadata
                .codex_manifests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for route_id in &candidate_ids {
                manifests.remove(route_id);
            }
        }
        if !deferred.contains(candidate_id) {
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
        }
        if let Some(store) = self.response_affinity_store.as_ref() {
            for route_id in &candidate_ids {
                let _ = store.delete_candidate(route_id);
            }
        }
        if removed {
            self.candidate_availability.notify_waiters();
        }
        removed
    }

    /// Refresh cadence follows physical members, not protocol-route aliases.
    /// This read does not build the more expensive routing-preview projection.
    pub fn active_member_keys(&self, now_ms: u64, recent_ms: u64) -> BTreeSet<String> {
        self.lock_scheduler().active_member_keys(now_ms, recent_ms)
    }

    pub fn candidate_runtime_order(&self) -> Vec<crate::CandidateRuntimeSnapshot> {
        let scheduler = self.lock_scheduler();
        let mut order = scheduler.runtime_order(runtime_now_ms());
        let revision = self.activity_revision.load(Ordering::Acquire);
        for candidate in &mut order {
            candidate.runtime_id = self.activity_runtime_id;
            candidate.activity_revision = revision;
        }
        order
    }

    pub fn candidate_runtime_order_for_key(
        &self,
        key_id: &str,
    ) -> Vec<crate::CandidateRuntimeSnapshot> {
        let Some(key) = self.keys.iter().find(|key| key.enabled && key.id == key_id) else {
            let mut order = self.candidate_runtime_order();
            for candidate in &mut order {
                candidate.next_for_new_request = false;
            }
            return order;
        };
        let scope = key
            .scope
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let protocols = key.client_wire_apis.as_deref().map_or_else(
            super::all_native_wire_apis,
            super::client_wire_apis_to_native,
        );
        let scheduler = self.lock_scheduler();
        let mut models = key.model_rules.clone();
        models.excluded.extend(
            scheduler
                .candidates()
                .flat_map(|candidate| &candidate.models)
                .filter(|model| super::is_image_model_id(model))
                .cloned(),
        );
        let mut order = scheduler.runtime_order_for(&scope, &models, &protocols, runtime_now_ms());
        let revision = self.activity_revision.load(Ordering::Acquire);
        for candidate in &mut order {
            candidate.runtime_id = self.activity_runtime_id;
            candidate.activity_revision = revision;
        }
        order
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
        let changed = self
            .lock_scheduler()
            .set_protected_candidate(candidate_id, reserve_basis_points);
        if changed {
            self.candidate_availability.notify_waiters();
        }
        changed
    }

    pub fn clear_candidate_cooldown(&self, candidate_id: &str, model: &str) -> bool {
        let updated = self.lock_scheduler().clear_cooldown(candidate_id, model);
        if updated {
            self.candidate_availability.notify_waiters();
        }
        updated
    }

    pub fn set_candidate_cooldown(
        &self,
        candidate_id: &str,
        model: &str,
        retry_at_ms: u64,
    ) -> bool {
        let updated = self
            .lock_scheduler()
            .set_cooldown(candidate_id, model, retry_at_ms);
        if updated {
            self.candidate_availability.notify_waiters();
        }
        updated
    }
}

fn is_model_capability_failure(category: &str) -> bool {
    matches!(
        category,
        error_codes::UPSTREAM_MODEL_NOT_FOUND
            | error_codes::UPSTREAM_MODEL_UNSUPPORTED
            | error_codes::UPSTREAM_USAGE_NOT_INCLUDED
            | error_codes::IMAGE_GENERATION_NOT_ENABLED
    )
}

/// Merge a persisted refresh into the passive in-memory snapshot without
/// allowing an older in-flight observation to win. A dirty snapshot with the
/// same timestamp is retained because it may contain response headers that
/// have not reached durable storage yet.
fn reconcile_passive_quota_snapshot(
    state: &mut super::PassiveQuotaState,
    incoming: &QuotaSnapshot,
    observed_at_ms: u64,
) -> QuotaSnapshot {
    let incoming_at = incoming.updated_at_ms.unwrap_or(observed_at_ms);
    let current_at = state.snapshot.updated_at_ms.unwrap_or_default();
    let incoming_wins = match (incoming.updated_at_ms, state.snapshot.updated_at_ms) {
        (Some(incoming_at), Some(current_at)) if incoming_at < current_at => false,
        (Some(incoming_at), Some(current_at)) if incoming_at == current_at => !state.dirty,
        (None, Some(_)) => false,
        _ => incoming_at >= current_at,
    };
    if incoming_wins {
        state.snapshot = incoming.clone();
        state.dirty = false;
        state.force_persist = false;
        state.last_persist_hint_ms = incoming.updated_at_ms.unwrap_or(observed_at_ms);
    }
    state.snapshot.clone()
}
