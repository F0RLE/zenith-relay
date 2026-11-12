use super::*;

#[derive(Default, PartialEq, Eq)]
pub(in crate::gateway::websocket) enum ConnectPhase {
    #[default]
    Select,
    Finish,
}

/// Moves with one logical request across handshake, live rejection and deferred retry.
pub(in crate::gateway::websocket) struct ConnectRecovery {
    pub(super) tried: HashSet<String>,
    pub(super) repairs: AttemptRepairs,
    pub(super) confirmed_response_missing: bool,
    pub(super) last_failure: Option<GatewayFailure>,
    pub(super) http_fallback_origin: Option<ErrorOrigin>,
    pub(in crate::gateway::websocket) allow_previous_response_reset: bool,
    pub(in crate::gateway::websocket) phase: ConnectPhase,
}

impl ConnectRecovery {
    pub(in crate::gateway::websocket) fn new(allow_previous_response_reset: bool) -> Self {
        Self {
            tried: HashSet::new(),
            repairs: AttemptRepairs::default(),
            confirmed_response_missing: false,
            last_failure: None,
            http_fallback_origin: None,
            allow_previous_response_reset,
            phase: ConnectPhase::Select,
        }
    }

    pub(in crate::gateway::websocket) fn reused(candidate_id: &str) -> Self {
        let mut state = Self::new(true);
        state.tried.insert(candidate_id.to_owned());
        state
    }

    pub(super) fn into_http_recovery(
        self,
        request: &ClientRequest,
    ) -> super::super::super::execution::ContinuationRecovery {
        super::super::super::execution::ContinuationRecovery::new(
            super::super::super::continuation::ContinuationState {
                response_affinity_key: request.response_affinity_key.clone(),
                requires_affinity_owner: request.requires_affinity_owner,
                has_unpaired_tool_output: request.has_unpaired_tool_output,
            },
            self.repairs,
            self.confirmed_response_missing,
            self.allow_previous_response_reset,
        )
    }

    pub(in crate::gateway::websocket) fn progress(&mut self) -> ConnectProgress<'_> {
        ConnectProgress {
            tried: &mut self.tried,
            repairs: &mut self.repairs,
            confirmed_response_missing: &mut self.confirmed_response_missing,
            last_failure: &mut self.last_failure,
            http_fallback_origin: &mut self.http_fallback_origin,
        }
    }
}
