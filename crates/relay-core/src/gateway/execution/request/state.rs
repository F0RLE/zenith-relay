use super::prelude::*;

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

pub(super) struct ContinuationState {
    pub(super) response_affinity_key: Option<String>,
    pub(super) requires_affinity_owner: bool,
    pub(super) has_unpaired_tool_output: bool,
    pub(super) confirmed_response_missing: bool,
    pub(super) repairs: AttemptRepairs,
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

impl AttemptState {
    pub(super) fn new(
        attempt_offset: u16,
        response_affinity_key: Option<String>,
        requires_affinity_owner: bool,
        request: &Value,
    ) -> Self {
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
            continuation: ContinuationState {
                response_affinity_key,
                requires_affinity_owner,
                has_unpaired_tool_output: !unpaired_tool_output_ids(request).is_empty(),
                confirmed_response_missing: false,
                repairs: AttemptRepairs::default(),
            },
        }
    }
}
