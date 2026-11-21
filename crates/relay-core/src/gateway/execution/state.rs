use super::super::continuation::{
    ContinuationState as PreparedContinuation, clear_materialized_continuation,
};
use super::super::errors::{AttemptFailure, PreservedUpstreamError};
use super::super::request::unpaired_tool_output_ids;
use super::{
    AttemptRepairs, detach_encrypted_context_repair_owner, release_encrypted_context_repair_owner,
};
use crate::protocol::AdapterError;
use crate::{ErrorOrigin, GatewayRuntime};
use serde_json::Value;
use std::collections::HashSet;

/// Mutable facts owned by one logical request across its candidate attempts.
/// The request identity, shared budget and candidate lease retain their existing
/// owners; this state neither duplicates them nor grants another dispatch.
pub(super) struct AttemptState {
    pub(super) retry: RetryState,
    pub(super) continuation: ContinuationState,
}

pub(super) struct RetryState {
    pub(super) attempt: u16,
    pub(super) tried: HashSet<String>,
    // Local preparation failures are request-local, not provider health.
    // Cooldown recovery must not reopen a route that cannot prepare this input.
    pub(super) preparation_exclusions: HashSet<String>,
    pub(super) last_failure: Option<AttemptFailure>,
    pub(super) last_failure_origin: ErrorOrigin,
    pub(super) last_adapter_error: Option<AdapterError>,
    pub(super) last_preserved_upstream_error: Option<PreservedUpstreamError>,
    pub(super) window_expired: bool,
}

#[derive(Clone)]
pub(super) struct ContinuationState {
    pub(super) response_affinity_key: Option<String>,
    pub(super) requires_affinity_owner: bool,
    pub(super) has_unpaired_tool_output: bool,
    pub(super) confirmed_response_missing: bool,
    pub(super) repairs: AttemptRepairs,
}

/// Recovery facts travel with a repaired request across an internal transport
/// handoff. Candidate exclusions stay transport-local; physical attempts remain
/// recorded under the request's shared budget and identity.
#[derive(Clone)]
pub(in crate::gateway) struct ContinuationRecovery {
    continuation: ContinuationState,
    allow_previous_response_reset: bool,
}

impl ContinuationRecovery {
    pub(in crate::gateway) fn new(
        prepared: PreparedContinuation,
        repairs: AttemptRepairs,
        confirmed_response_missing: bool,
        allow_previous_response_reset: bool,
    ) -> Self {
        Self {
            continuation: ContinuationState {
                repairs,
                confirmed_response_missing,
                ..prepared.into()
            },
            allow_previous_response_reset,
        }
    }

    pub(super) fn into_state(self, attempt: u16) -> (AttemptState, bool) {
        (
            AttemptState::with_continuation(attempt, self.continuation),
            self.allow_previous_response_reset,
        )
    }
}

impl ContinuationState {
    pub(super) fn clear_materialized(&mut self) {
        clear_materialized_continuation(
            &mut self.response_affinity_key,
            &mut self.requires_affinity_owner,
            &mut self.has_unpaired_tool_output,
        );
    }

    pub(super) fn release_repair_owner(&mut self, runtime: &GatewayRuntime) {
        release_encrypted_context_repair_owner(
            &mut self.repairs,
            &mut self.response_affinity_key,
            &mut self.requires_affinity_owner,
            runtime,
        );
    }

    pub(super) fn detach_repair_owner(&mut self) {
        detach_encrypted_context_repair_owner(
            &self.repairs,
            &mut self.response_affinity_key,
            &mut self.requires_affinity_owner,
        );
    }
}

impl From<PreparedContinuation> for ContinuationState {
    fn from(prepared: PreparedContinuation) -> Self {
        Self {
            response_affinity_key: prepared.response_affinity_key,
            requires_affinity_owner: prepared.requires_affinity_owner,
            has_unpaired_tool_output: prepared.has_unpaired_tool_output,
            confirmed_response_missing: false,
            repairs: AttemptRepairs::default(),
        }
    }
}

impl AttemptState {
    pub(super) fn new(
        attempt_offset: u16,
        response_affinity_key: Option<String>,
        requires_affinity_owner: bool,
        request: &Value,
    ) -> Self {
        Self::with_continuation(
            attempt_offset,
            ContinuationState {
                response_affinity_key,
                requires_affinity_owner,
                has_unpaired_tool_output: !unpaired_tool_output_ids(request).is_empty(),
                confirmed_response_missing: false,
                repairs: AttemptRepairs::default(),
            },
        )
    }

    pub(super) fn from_prepared(continuation: PreparedContinuation) -> Self {
        Self::with_continuation(0, continuation.into())
    }

    fn with_continuation(attempt_offset: u16, continuation: ContinuationState) -> Self {
        Self {
            retry: RetryState {
                attempt: attempt_offset,
                tried: HashSet::new(),
                preparation_exclusions: HashSet::new(),
                last_failure: None,
                last_failure_origin: ErrorOrigin::Relay,
                last_adapter_error: None,
                last_preserved_upstream_error: None,
                window_expired: false,
            },
            continuation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_handoff_retains_consumed_repairs_and_continuation_owner() {
        let repairs = AttemptRepairs {
            native_replay: true,
            function_item_id: true,
            custom_tool_item_id: true,
            tool_search_item_id: true,
            message_item_id: true,
            legacy_call_id: true,
            model_switch_reset: true,
            stale_tool_history: true,
            quota_yield: true,
            encrypted_context: true,
            encrypted_context_repair_key: Some("synthetic-repair-owner".into()),
        };
        let recovery = ContinuationRecovery::new(
            PreparedContinuation {
                response_affinity_key: Some("synthetic-response-owner".into()),
                requires_affinity_owner: true,
                has_unpaired_tool_output: true,
            },
            repairs,
            true,
            false,
        );
        let (state, allow_reset) = recovery.into_state(2);
        assert!(!allow_reset);
        assert_eq!(state.retry.attempt, 2);
        assert!(
            state.retry.tried.is_empty(),
            "transport exclusions do not exclude HTTP routes"
        );
        let continuation = state.continuation;
        assert_eq!(
            continuation.response_affinity_key.as_deref(),
            Some("synthetic-response-owner")
        );
        assert!(continuation.requires_affinity_owner);
        assert!(continuation.has_unpaired_tool_output);
        assert!(continuation.confirmed_response_missing);
        let repairs = continuation.repairs;
        assert!(repairs.native_replay);
        assert!(repairs.function_item_id);
        assert!(repairs.custom_tool_item_id);
        assert!(repairs.tool_search_item_id);
        assert!(repairs.message_item_id);
        assert!(repairs.legacy_call_id);
        assert!(repairs.model_switch_reset);
        assert!(repairs.stale_tool_history);
        assert!(repairs.quota_yield);
        assert!(repairs.encrypted_context);
        assert_eq!(
            repairs.encrypted_context_repair_key.as_deref(),
            Some("synthetic-repair-owner")
        );
    }

    #[test]
    fn prepared_continuation_keeps_its_facts_and_initializes_one_retry_owner() {
        for (affinity, requires_affinity_owner, has_unpaired_tool_output) in [
            (None, false, false),
            (Some("response-owner"), true, false),
            (Some("response-owner"), true, true),
        ] {
            let state = AttemptState::from_prepared(PreparedContinuation {
                response_affinity_key: affinity.map(str::to_owned),
                requires_affinity_owner,
                has_unpaired_tool_output,
            });
            assert_eq!(
                state.continuation.response_affinity_key.as_deref(),
                affinity
            );
            assert_eq!(
                state.continuation.requires_affinity_owner,
                requires_affinity_owner
            );
            assert_eq!(
                state.continuation.has_unpaired_tool_output,
                has_unpaired_tool_output
            );
            assert!(!state.continuation.confirmed_response_missing);
            assert_eq!(state.retry.attempt, 0);
            assert!(state.retry.tried.is_empty());
            assert!(state.retry.preparation_exclusions.is_empty());
            assert!(state.retry.last_failure.is_none());
            assert!(state.retry.last_adapter_error.is_none());
            assert!(state.retry.last_preserved_upstream_error.is_none());
            assert_eq!(state.retry.last_failure_origin, ErrorOrigin::Relay);
            assert!(!state.retry.window_expired);
            assert!(!state.continuation.repairs.legacy_call_id);
            assert!(!state.continuation.repairs.encrypted_context);
            assert!(
                state
                    .continuation
                    .repairs
                    .encrypted_context_repair_key
                    .is_none()
            );
        }
    }

    #[test]
    fn clearing_continuation_preserves_consumed_repairs_and_detached_lease_owner() {
        let mut state = AttemptState::from_prepared(PreparedContinuation {
            response_affinity_key: Some("repair-owner".into()),
            requires_affinity_owner: true,
            has_unpaired_tool_output: true,
        });
        state.continuation.repairs.legacy_call_id = true;
        state.continuation.repairs.encrypted_context = true;
        state.continuation.repairs.encrypted_context_repair_key = Some("repair-owner".into());

        state.continuation.detach_repair_owner();
        assert!(state.continuation.response_affinity_key.is_none());
        assert!(!state.continuation.requires_affinity_owner);
        assert!(state.continuation.has_unpaired_tool_output);
        state.continuation.clear_materialized();
        assert!(!state.continuation.has_unpaired_tool_output);
        assert!(state.continuation.repairs.legacy_call_id);
        assert!(state.continuation.repairs.encrypted_context);
        assert_eq!(
            state
                .continuation
                .repairs
                .encrypted_context_repair_key
                .as_deref(),
            Some("repair-owner")
        );
    }
}
