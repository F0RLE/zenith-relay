use super::admission::AdmissionRequest;
use super::*;
use crate::scheduler::rotation::{RotationOperation, SharedRequestBudget};
use crate::{scheduler::CooldownReason, Selection, SelectionRequest};

impl GatewayRuntime {
    pub(crate) fn configured_executor_routes(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        protocols: &[WireApi],
        stream: bool,
    ) -> Vec<ExecutorRoute> {
        let scope = key.scope_snapshot();
        let ids = self
            .lock_scheduler()
            .candidates()
            .filter(|candidate| candidate.is_configured(model, protocols, &scope))
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        ids.iter()
            .filter_map(|id| self.executor_route(id, model, &scope, protocols, stream))
            .collect()
    }

    #[cfg(test)]
    pub(crate) async fn select_and_reserve(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        affinity_keys: (Option<&str>, Option<&str>),
        now_ms: u64,
    ) -> Option<(Selection, CandidateLease)> {
        let (response_affinity_key, prompt_affinity_key) = affinity_keys;
        let budget = SharedRequestBudget::for_incoming_request(self.request_dispatch_budget());
        self.select_and_wait_for_capacity(
            key,
            model,
            allowed_protocols,
            tried,
            response_affinity_key,
            prompt_affinity_key,
            now_ms,
            RotationOperation::Text,
            &budget,
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Admission carries exact route, key scope, affinity and the shared request budget."
    )]
    pub(crate) async fn select_and_reserve_with_budget(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        affinity_keys: (Option<&str>, Option<&str>),
        now_ms: u64,
        budget: &SharedRequestBudget,
    ) -> Option<(Selection, CandidateLease)> {
        self.select_and_reserve_operation_with_budget(
            key,
            model,
            allowed_protocols,
            tried,
            affinity_keys,
            now_ms,
            RotationOperation::Text,
            budget,
        )
        .await
    }

    pub(crate) async fn select_and_reserve_image_with_budget(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        now_ms: u64,
        budget: &SharedRequestBudget,
    ) -> Option<(Selection, CandidateLease)> {
        self.select_and_reserve_operation_with_budget(
            key,
            model,
            allowed_protocols,
            tried,
            (None, None),
            now_ms,
            RotationOperation::Image,
            budget,
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Admission carries exact route, operation, key scope and the shared request budget."
    )]
    pub(crate) async fn select_and_reserve_operation_with_budget(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        affinity_keys: (Option<&str>, Option<&str>),
        now_ms: u64,
        operation: RotationOperation,
        budget: &SharedRequestBudget,
    ) -> Option<(Selection, CandidateLease)> {
        let (response_affinity_key, prompt_affinity_key) = affinity_keys;
        self.select_and_wait_for_capacity(
            key,
            model,
            allowed_protocols,
            tried,
            response_affinity_key,
            prompt_affinity_key,
            now_ms,
            operation,
            budget,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn select_and_wait_for_capacity(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        response_affinity_key: Option<&str>,
        prompt_affinity_key: Option<&str>,
        now_ms: u64,
        operation: RotationOperation,
        budget: &SharedRequestBudget,
    ) -> Option<(Selection, CandidateLease)> {
        if let (Some(key), Some(store)) =
            (response_affinity_key, self.response_affinity_store.as_ref())
        {
            let cached = self.lock_scheduler().has_response_affinity(key, now_ms);
            if !cached {
                if let Ok(Some(binding)) = store.find(key, now_ms) {
                    self.lock_scheduler().restore_response_affinity(
                        binding.key,
                        &binding.candidate_id,
                        binding.expires_at_ms,
                        now_ms,
                    );
                }
            }
        }
        if let (Some(key), Some(store)) =
            (prompt_affinity_key, self.response_affinity_store.as_ref())
        {
            let cached = self.lock_scheduler().has_prompt_affinity(key, now_ms);
            if !cached {
                if let Ok(Some(binding)) = store.find(key, now_ms) {
                    self.lock_scheduler().restore_prompt_affinity(
                        binding.key,
                        &binding.candidate_id,
                        binding.expires_at_ms,
                        now_ms,
                    );
                }
            }
        }
        let attempted_members = budget.attempted_members();
        let mut exclusions = self.lock_scheduler().routes_for_members(&attempted_members);
        exclusions.extend(tried.iter().cloned());
        self.admit(
            AdmissionRequest {
                key: key.clone(),
                model: model.into(),
                protocols: allowed_protocols.into(),
                tried: exclusions,
                response_affinity: response_affinity_key.map(str::to_owned),
                prompt_affinity: prompt_affinity_key.map(str::to_owned),
                operation,
                budget: budget.clone(),
            },
            now_ms,
        )
        .await
    }

    pub(super) fn try_reserve_admission(
        &self,
        request: &AdmissionRequest,
        now_ms: u64,
    ) -> (Option<(Selection, CandidateLease)>, bool) {
        let AdmissionRequest {
            key,
            model,
            protocols: allowed_protocols,
            tried,
            response_affinity,
            prompt_affinity,
            operation,
            budget,
        } = request;
        let operation = *operation;
        let response_affinity_key = response_affinity.as_deref();
        let prompt_affinity_key = prompt_affinity.as_deref();
        let lane = if operation == RotationOperation::Image {
            CandidateLeaseLane::Image
        } else {
            CandidateLeaseLane::Text
        };
        let request_id = budget.request_id();
        if !budget.can_dispatch() {
            return (None, false);
        }
        // Live principal scope stays locked through selection and reservation.
        let scope = key.scope_read();
        let mut scheduler = self.lock_scheduler();
        let selection = match (lane, operation) {
            (CandidateLeaseLane::Text, RotationOperation::Compaction) => scheduler
                .select_compaction(SelectionRequest {
                    model,
                    allowed_protocols,
                    scope: &scope,
                    tried,
                    response_affinity_key,
                    prompt_affinity_key,
                    now_ms,
                }),
            (CandidateLeaseLane::Text, _) => scheduler.select(SelectionRequest {
                model,
                allowed_protocols,
                scope: &scope,
                tried,
                response_affinity_key,
                prompt_affinity_key,
                now_ms,
            }),
            (CandidateLeaseLane::Image, _) => scheduler.select_image(SelectionRequest {
                model,
                allowed_protocols,
                scope: &scope,
                tried,
                response_affinity_key,
                prompt_affinity_key,
                now_ms,
            }),
        };
        let reserved = selection.and_then(|selection| {
            let reservation = scheduler.reserve_request_with_operation(
                &selection.candidate_id,
                model,
                now_ms,
                matches!(lane, CandidateLeaseLane::Image),
                operation,
                Some(request_id),
                Some(&selection.rotation_request),
            );
            reservation.map(|reservation_id| {
                let image_bridge_revision = (operation == RotationOperation::Image)
                    .then(|| self.chatgpt_accounts.get(&selection.candidate_id))
                    .flatten()
                    .map(|account| {
                        let revision = account.image_bridge_revision.clone();
                        let captured = revision.load(Ordering::Acquire);
                        (revision, captured)
                    });
                let lease = CandidateLease {
                    scheduler: self.scheduler.clone(),
                    availability: self.candidate_availability.clone(),
                    candidate_id: selection.candidate_id.clone(),
                    candidate_permission_revision: scheduler
                        .candidate_permission_revision(&selection.candidate_id),
                    member_key: scheduler
                        .member_key_for(&selection.candidate_id)
                        .expect("a reserved candidate has a physical member"),
                    reservation_id,
                    principal_scope: key.scope.clone(),
                    principal_scope_revision: (
                        key.scope_revision.clone(),
                        key.scope_revision.load(Ordering::Acquire),
                    ),
                    model: model.to_owned(),
                    allowed_protocols: allowed_protocols.to_vec(),
                    response_owner: response_affinity_key
                        .filter(|_| selection.rotation_request.owner.is_some())
                        .map(|key| {
                            let (owner, revision) = scheduler
                                .response_affinity_binding(key, now_ms)
                                .expect("reserved response owner is still bound");
                            debug_assert_eq!(owner, selection.candidate_id);
                            (key.to_owned(), revision)
                        }),
                    image_bridge_revision,
                    rotation_budget: budget.clone(),
                    rotation_started: AtomicBool::new(false),
                    rotation_settled: AtomicBool::new(false),
                    activity_callback: self.activity_callback.clone(),
                    activity_runtime_id: self.activity_runtime_id,
                    activity_revision: self.activity_revision.clone(),
                    released: AtomicBool::new(false),
                };
                (selection, lease)
            })
        });
        let capacity_blocked = reserved.is_none()
            && scheduler.capacity_blocked_for(
                SelectionRequest {
                    model,
                    allowed_protocols,
                    scope: &scope,
                    tried,
                    response_affinity_key,
                    prompt_affinity_key,
                    now_ms,
                },
                operation,
            );
        (reserved, capacity_blocked)
    }

    pub(super) fn admission_activity(
        &self,
        selection: &Selection,
        request: &AdmissionRequest,
        now_ms: u64,
    ) {
        let activity = {
            let scheduler = self.lock_scheduler();
            let (in_flight, active_request_count, active_models) =
                scheduler.runtime_activity_for(&selection.candidate_id);
            RuntimeActivitySnapshot {
                runtime_id: self.activity_runtime_id,
                revision: self.activity_revision.fetch_add(1, Ordering::AcqRel) + 1,
                candidate_id: selection.candidate_id.clone(),
                member_key: scheduler
                    .member_key_for(&selection.candidate_id)
                    .unwrap_or_default(),
                in_flight,
                active_request_count,
                active_models,
            }
        };
        self.emit_activity_changed(activity);
        if let Some(key) = &request.response_affinity {
            if selection.response_affinity_hit {
                self.persist_response_affinity(key, &selection.candidate_id, now_ms);
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Availability must use the same scoped operation as admission."
    )]
    pub(crate) fn recovery_retry_at(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        exclusions: &HashSet<String>,
        response_affinity_key: Option<&str>,
        now_ms: u64,
        operation: RotationOperation,
    ) -> Option<u64> {
        self.lock_scheduler().recovery_retry_at_for(
            SelectionRequest {
                model,
                allowed_protocols,
                scope: &key.scope_snapshot(),
                tried: exclusions,
                response_affinity_key,
                prompt_affinity_key: None,
                now_ms,
            },
            operation,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Availability must use the same scoped operation as admission."
    )]
    pub(crate) fn earliest_retry_at(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        response_affinity_key: Option<&str>,
        now_ms: u64,
        operation: RotationOperation,
    ) -> Option<u64> {
        let scope = key.scope_snapshot();
        self.lock_scheduler().earliest_retry_at_for(
            SelectionRequest {
                model,
                allowed_protocols,
                scope: &scope,
                tried,
                response_affinity_key,
                prompt_affinity_key: None,
                now_ms,
            },
            operation,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Availability must use the same scoped operation as admission."
    )]
    pub(crate) fn all_applicable_cooldown(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        response_affinity_key: Option<&str>,
        now_ms: u64,
        operation: RotationOperation,
    ) -> Option<(u64, CooldownReason)> {
        let scope = key.scope_snapshot();
        self.lock_scheduler().all_applicable_cooldown_for(
            SelectionRequest {
                model,
                allowed_protocols,
                scope: &scope,
                tried,
                response_affinity_key,
                prompt_affinity_key: None,
                now_ms,
            },
            operation,
        )
    }
}
