//! Operator controls for one running gateway: tools, routing, tiers, and account transport.

use super::*;

impl GatewayRuntime {
    /// Installs a lightweight observer for request start/end activity.
    /// The callback carries only routing identifiers and live counts; request
    /// data and provider responses never cross the host boundary.
    pub fn set_activity_callback(
        &self,
        callback: impl Fn(RuntimeActivitySnapshot) + Send + Sync + 'static,
    ) {
        if let Ok(mut callback_slot) = self.activity_callback.lock() {
            *callback_slot = Arc::new(callback);
        }
    }

    /// Installs the host-specific persistence hook for Team breaker siblings.
    /// The callback receives only candidate ids; local and server pools keep
    /// their own account stores and may persist the block independently.
    pub fn set_chatgpt_team_breaker_callback(
        &self,
        callback: impl Fn(Vec<String>) + Send + Sync + 'static,
    ) {
        if let Ok(mut callback_slot) = self.chatgpt_team_breaker_callback.lock() {
            *callback_slot = Arc::new(callback);
        }
    }

    pub(crate) fn emit_activity_changed(&self, activity: RuntimeActivitySnapshot) {
        let callback = self
            .activity_callback
            .lock()
            .ok()
            .map(|callback| callback.clone());
        if let Some(callback) = callback {
            callback(activity);
        }
    }

    pub fn codex_background_tasks_enabled(&self) -> bool {
        self.control.codex_background_tasks_enabled()
    }

    /// Requests snapshot this value once. Hot updates never rebuild the
    /// listener or change an already admitted request's policy during retry.
    pub fn tool_policy(&self) -> crate::ToolPolicy {
        crate::poison::read(&self.tool_policy).clone()
    }

    pub fn set_tool_policy(&self, policy: crate::ToolPolicy) -> Result<()> {
        let policy = policy
            .normalized()
            .map_err(|message| Error::Validation(message.to_string()))?;
        *crate::poison::write(&self.tool_policy) = policy;
        Ok(())
    }

    pub fn set_codex_background_tasks_enabled(&self, enabled: bool) {
        self.control.set_codex_background_tasks_enabled(enabled);
    }

    pub fn codex_websockets_enabled(&self) -> bool {
        self.control.codex_websockets_enabled()
    }

    pub fn set_codex_websockets_enabled(&self, enabled: bool) {
        self.control.set_codex_websockets_enabled(enabled);
    }

    pub fn block_degraded_routes_enabled(&self) -> bool {
        self.control.block_degraded_routes_enabled()
    }

    pub fn set_block_degraded_routes_enabled(&self, enabled: bool) {
        self.control.set_block_degraded_routes_enabled(enabled);
    }

    /// Internal downgrade ids stay ordinary missing models while this is off.
    pub(crate) fn effective_upstream_category<'a>(&self, category: &'a str) -> &'a str {
        if category == crate::error_codes::UPSTREAM_ROUTE_DEGRADED
            && !self.block_degraded_routes_enabled()
        {
            crate::error_codes::UPSTREAM_MODEL_NOT_FOUND
        } else {
            category
        }
    }

    pub(crate) fn degraded_route_blocked(&self, model: &str) -> bool {
        self.block_degraded_routes_enabled() && crate::is_degraded_route_model(model)
    }

    pub fn route_recovery_enabled(&self) -> bool {
        self.control.route_recovery_enabled()
    }

    pub fn set_route_recovery_enabled(&self, enabled: bool) {
        self.control.set_route_recovery_enabled(enabled);
        self.candidate_availability.notify_waiters();
    }

    /// The bounded retry window for gateway requests without persistent route
    /// recovery. It starts at the first replay-safe rejection, not dispatch.
    pub fn route_recovery_window_ms(&self) -> u64 {
        self.control.route_recovery_window_ms()
    }

    pub fn set_route_recovery_window_ms(&self, window_ms: u64) {
        self.control.set_route_recovery_window_ms(window_ms);
        self.candidate_availability.notify_waiters();
    }

    pub(crate) fn mark_request_origin(&self, request_id: &str, origin: &'static str) {
        self.control.mark_request_origin(request_id, origin);
    }

    pub(crate) fn request_origin(&self, request_id: &str) -> Option<&'static str> {
        self.control.request_origin(request_id)
    }

    pub(crate) fn blocked_codex_background_event(
        &self,
        request_id: &str,
        local_key_id: &str,
        requested_model: &str,
        wire_api: WireApi,
        transport: crate::UsageTransport,
        origin: &'static str,
    ) {
        self.control.blocked_codex_background_event(
            &self.usage,
            request_id,
            local_key_id,
            requested_model,
            wire_api,
            transport,
            origin,
        );
    }

    pub fn set_pool_routing_policy(
        &self,
        policy: crate::PoolRoutingPolicy,
        max_retry_candidates: u8,
    ) -> Result<()> {
        self.set_pool_routing_policy_with_key_scopes(policy, max_retry_candidates, &[])?;
        Ok(())
    }

    /// Apply host membership and the corresponding internal key scopes as one
    /// routing transaction. A final dispatch holds scope -> scheduler locks;
    /// taking them in the same order here prevents a send in the gap between
    /// replacing the policy and revoking a removed member's key permission.
    /// Missing keys abort without changing either the policy or any scope.
    pub fn set_pool_routing_policy_with_key_scopes(
        &self,
        policy: crate::PoolRoutingPolicy,
        max_retry_candidates: u8,
        key_scopes: &[(String, CandidateScope)],
    ) -> Result<bool> {
        policy
            .validate_activation()
            .map_err(|message| Error::Validation(message.into()))?;
        if !crate::protocol::max_retry_candidates_in_range(max_retry_candidates) {
            return Err(Error::Validation(
                "max retry candidates must be between 1 and 8".into(),
            ));
        }
        let mut updates = key_scopes.iter().collect::<Vec<_>>();
        updates.sort_by(|(left, _), (right, _)| left.cmp(right));
        let mut seen = BTreeSet::new();
        let mut keys = Vec::with_capacity(updates.len());
        for (id, scope) in updates {
            if !seen.insert(id) {
                return Err(Error::Validation(
                    "duplicate gateway key scope update".into(),
                ));
            }
            let Some(key) = self.keys.iter().find(|key| key.enabled && key.id == *id) else {
                return Ok(false);
            };
            keys.push((key, scope));
        }
        let mut locked = Vec::with_capacity(keys.len());
        for (key, scope) in keys {
            locked.push((key, scope, crate::poison::write(&key.scope)));
        }
        let mut scheduler = self.lock_scheduler();
        scheduler.set_pool_routing(policy)?;
        self.max_retry_candidates
            .store(usize::from(max_retry_candidates), Ordering::Relaxed);
        for (key, scope, mut existing_scope) in locked {
            if *existing_scope != *scope {
                *existing_scope = scope.clone();
                key.scope_revision.fetch_add(1, Ordering::AcqRel);
            }
        }
        drop(scheduler);
        self.candidate_availability.notify_waiters();
        Ok(true)
    }

    pub(crate) fn source_recovery_delay_ms(&self, candidate_id: &str) -> Option<u64> {
        crate::poison::mutex(&self.source_recovery_delays_ms)
            .get(candidate_id)
            .copied()
    }

    pub fn set_default_service_tier(&self, tier: DefaultServiceTier) {
        self.default_service_tier_value
            .store(tier.atomic_value(), Ordering::Relaxed);
    }

    pub(crate) fn default_service_tier(&self) -> DefaultServiceTier {
        DefaultServiceTier::from_atomic_value(
            self.default_service_tier_value.load(Ordering::Relaxed),
        )
    }

    /// Applies the operator-selected speed policy. Client-owned API requests
    /// retain an explicit tier at the gateway boundary.
    pub fn set_model_service_tier_overrides(
        &self,
        overrides: BTreeMap<String, DefaultServiceTier>,
    ) -> Result<()> {
        let overrides = normalize_model_service_tier_overrides(overrides)
            .map_err(|message| Error::Validation(message.to_string()))?;
        *crate::poison::mutex(&self.model_service_tier_overrides) = overrides;
        Ok(())
    }

    pub(crate) fn model_effective_service_tier(&self, model: &str) -> DefaultServiceTier {
        let requested = crate::poison::mutex(&self.model_service_tier_overrides)
            .get(&crate::model_id_key(model))
            .copied()
            .unwrap_or_else(|| self.default_service_tier());
        self.project_service_tier_for_model(model, requested)
    }

    pub fn set_model_display_order(&self, models: Vec<String>) {
        *crate::poison::mutex(&self.model_display_order) = crate::normalize_model_ids(models);
    }

    /// Switches the explicitly labelled Excel/Basis Points transport for OAuth
    /// accounts without rebuilding the scheduler. Agent Identity accounts
    /// cannot use this transport. The account candidate, quota state and
    /// concurrency reservation remain unchanged.
    pub fn set_basis_points_enabled(&self, enabled: bool) {
        for account in self.chatgpt_accounts.values() {
            let oauth = crate::poison::read(&account.agent_identity).is_none();
            account
                .basis_points_enabled
                .store(enabled && oauth, Ordering::Relaxed);
        }
    }

    /// A host replaces this runtime's routing graph without waiting for
    /// already-served streams to finish. Serialize retirement with final rotation
    /// dispatch, then wake admissions so they do not wait on dead capacity.
    pub fn retire_for_replacement(&self) {
        self.lock_scheduler().retire_for_replacement();
        self.candidate_availability.notify_waiters();
        self.admission_changed.notify_waiters();
    }
}
