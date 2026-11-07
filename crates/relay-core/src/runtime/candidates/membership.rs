use super::super::{GatewayRuntime, runtime_now_ms};
use crate::{CandidateHealth, CandidateKind, CandidateScope};
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

impl GatewayRuntime {
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
                        .filter(|account_id| allowed_account_ids.contains(*account_id))
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
        self.invalidate_account_quota_reference_source(candidate_id);
        let candidate_ids = {
            let source_routes = self
                .source_state
                .candidate_bindings
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
            let mut manifests = crate::poison::mutex(&self.provider_metadata.codex_manifests);
            for route_id in &candidate_ids {
                manifests.remove(route_id);
            }
        }
        if !deferred.contains(candidate_id) {
            crate::poison::mutex(&self.account_state.passive_quotas).remove(candidate_id);
            if let Some(account) = self.account_state.executors.get(candidate_id) {
                account.active.store(false, Ordering::Release);
                *crate::poison::write(&account.agent_identity) = None;
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
        let projection = self.lock_scheduler().clone();
        let mut order = projection.into_runtime_order(
            &CandidateScope::default(),
            &crate::ModelRules::default(),
            &crate::WireApi::ALL_NATIVE,
            runtime_now_ms(),
        );
        let revision = self.activity_revision.load(Ordering::Acquire);
        for candidate in &mut order {
            candidate.runtime_id = self.activity_runtime_id;
            candidate.activity_revision = revision;
        }
        order
    }

    /// Read-only reserve policy: positive ordinary quota on a compatible pool
    /// member, independent of saved reset credits and paid-credit fallback.
    pub fn ordinary_quota_alternative_available(
        &self,
        account_id: &str,
        now_ms: u64,
    ) -> Option<bool> {
        let projection = self.lock_scheduler().clone();
        let target = projection.candidates().find(|candidate| {
            candidate.id == account_id
                && candidate.kind == CandidateKind::OAuthAccount
                && candidate.enabled
                && !candidate.draining
                && candidate.secret_available
                && candidate.health.is_eligible()
        })?;
        let models = crate::ModelRules {
            allowed: target
                .models
                .iter()
                .filter(|model| {
                    target.model_rules.allows(model) && !super::super::is_image_model_id(model)
                })
                .cloned()
                .collect(),
            ..Default::default()
        };
        if models.allowed.is_empty() {
            return None;
        }
        let protocol = target.protocol;
        let confirmed_credit_fallbacks = {
            let quotas = crate::poison::mutex(&self.account_state.passive_quotas);
            quotas
                .iter()
                .filter(|(_, state)| {
                    [
                        state.snapshot.primary.as_ref(),
                        state.snapshot.secondary.as_ref(),
                    ]
                    .into_iter()
                    .flatten()
                    .any(|window| {
                        window.available_basis_points == Some(0)
                            && now_ms
                                .checked_sub(window.observed_at_ms)
                                .is_some_and(|age| age <= crate::scheduler::QUOTA_STALE_AFTER_MS)
                    })
                })
                .map(|(id, _)| id.clone())
                .collect::<BTreeSet<_>>()
        };
        let order =
            projection.into_runtime_order(&CandidateScope::default(), &models, &[protocol], now_ms);
        let mut unknown = false;
        for candidate in order {
            if candidate.kind != CandidateKind::OAuthAccount
                || candidate.candidate_id == account_id
                || !(candidate.available || candidate.capacity_wait)
            {
                continue;
            }
            let Some(headroom) = candidate.resource_headroom else {
                unknown = true;
                continue;
            };
            if headroom
                .quota_remaining_basis_points
                .is_some_and(|remaining| remaining > 0)
            {
                return Some(true);
            }
            if headroom.quota_remaining_basis_points.is_none()
                && !confirmed_credit_fallbacks.contains(&candidate.candidate_id)
            {
                unknown = true;
            }
        }
        (!unknown).then_some(false)
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
        let scope = crate::poison::read(&key.scope).clone();
        let protocols = key.client_wire_apis.as_deref().map_or_else(
            super::super::all_native_wire_apis,
            super::super::client_wire_apis_to_native,
        );
        let (models, projection) = {
            let scheduler = self.lock_scheduler();
            let mut models = key.model_rules.clone();
            models.excluded.extend(
                scheduler
                    .candidates()
                    .flat_map(|candidate| &candidate.models)
                    .filter(|model| super::super::is_image_model_id(model))
                    .cloned(),
            );
            (models, scheduler.clone())
        };
        let mut order =
            projection.into_runtime_order(&scope, &models, &protocols, runtime_now_ms());
        let revision = self.activity_revision.load(Ordering::Acquire);
        for candidate in &mut order {
            candidate.runtime_id = self.activity_runtime_id;
            candidate.activity_revision = revision;
        }
        order
    }

    pub(crate) fn account_candidate_is_active(&self, candidate_id: &str) -> bool {
        self.account_state
            .executors
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
