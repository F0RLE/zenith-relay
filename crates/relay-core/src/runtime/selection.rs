use super::admission::AdmissionRequest;
use super::*;
use crate::scheduler::rotation::{RotationOperation, SharedRequestBudget};
use crate::{scheduler::CooldownReason, Selection, SelectionRequest};

mod capacity;
mod route;

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

    pub(crate) fn automatic_response_owner_should_yield(
        &self,
        key: &AuthenticatedKey,
        affinity_key: &str,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        now_ms: u64,
    ) -> bool {
        let scope = key.scope_snapshot();
        self.lock_scheduler().automatic_response_owner_should_yield(
            affinity_key,
            model,
            allowed_protocols,
            &scope,
            tried,
            now_ms,
        )
    }
}
