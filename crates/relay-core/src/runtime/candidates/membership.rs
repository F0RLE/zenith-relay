use super::super::{runtime_now_ms, GatewayRuntime};
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
            super::super::all_native_wire_apis,
            super::super::client_wire_apis_to_native,
        );
        let scheduler = self.lock_scheduler();
        let mut models = key.model_rules.clone();
        models.excluded.extend(
            scheduler
                .candidates()
                .flat_map(|candidate| &candidate.models)
                .filter(|model| super::super::is_image_model_id(model))
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
