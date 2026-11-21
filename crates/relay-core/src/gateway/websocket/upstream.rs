mod connect;
mod drive;
mod handshake;
mod initial_terminal;
mod messages;
mod open;
mod recovery;
mod selection_gap;
mod session;
mod telemetry;
mod upgrade_reject;

use std::collections::HashSet;

use super::super::execution::AttemptRepairs;
use super::*;

pub(in crate::gateway::websocket) use initial_terminal::{TerminalAction, handle_initial_terminal};
#[cfg(test)]
pub(in crate::gateway::websocket) use messages::initial_payloads_are_empty_incomplete;
pub(in crate::gateway::websocket) use messages::{
    initial_message_state, initial_messages_are_empty_incomplete, message_serves_rejected_model,
};
pub(in crate::gateway::websocket) use recovery::{ConnectPhase, ConnectRecovery};
pub(in crate::gateway::websocket) use session::{
    await_while_client_connected, connect_transport_while_client_connected,
    connect_upstream_while_client_connected,
};
pub(in crate::gateway::websocket) use telemetry::record_connect_failure;

pub(super) enum ConnectedTransport {
    Websocket(Box<Connected>),
    Http(Box<HttpFallback>),
}

pub(super) struct HttpFallback {
    pub(super) request: ClientRequest,
    pub(super) reserved_candidate: Option<super::super::execution::ReservedCandidate>,
    pub(super) recovery: Option<super::super::execution::ContinuationRecovery>,
    pub(super) reason: crate::usage::TransportFallbackReason,
    pub(super) origin: ErrorOrigin,
}

impl HttpFallback {
    pub(super) fn new(
        request: ClientRequest,
        reason: crate::usage::TransportFallbackReason,
    ) -> Self {
        Self {
            request,
            reserved_candidate: None,
            recovery: None,
            reason,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn for_candidate(
        request: ClientRequest,
        key: &AuthenticatedKey,
        selection: crate::Selection,
        lease: CandidateLease,
        reason: crate::usage::TransportFallbackReason,
        origin: ErrorOrigin,
    ) -> Self {
        let reserved_candidate = super::super::execution::ReservedCandidate {
            local_key_id: key.id.clone(),
            model: request.resolved_model.clone(),
            selection,
            lease,
        };
        Self {
            reserved_candidate: Some(reserved_candidate),
            origin,
            ..Self::new(request, reason)
        }
    }

    pub(super) fn retain_recovery(&mut self, recovery: ConnectRecovery) {
        self.recovery = Some(recovery.into_http_recovery(&self.request));
    }
}

/// The candidate reserved for the WebSocket upgrade that is in progress.
pub(super) struct ConnectScope<'a> {
    pub(super) event: Option<&'a UsageEvent>,
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) request: &'a mut ClientRequest,
    pub(super) route: &'a ExecutorRoute,
    pub(super) lease: &'a CandidateLease,
    pub(super) attempt: u16,
    pub(super) started: Instant,
    pub(super) source_error_origin: ErrorOrigin,
    pub(super) response_affinity_hit: bool,
}

/// One WebSocket connect attempt, shared by its usage events.
pub(super) struct ConnectTrace<'a> {
    pub(super) event: Option<&'a UsageEvent>,
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) lease: &'a CandidateLease,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) route: &'a ExecutorRoute,
    pub(super) request: &'a ClientRequest,
    pub(super) attempt: u16,
    pub(super) started: Instant,
}

impl ConnectScope<'_> {
    pub(super) fn trace(&self) -> ConnectTrace<'_> {
        ConnectTrace {
            event: self.event,
            runtime: self.runtime,
            lease: self.lease,
            key: self.key,
            route: self.route,
            request: self.request,
            attempt: self.attempt,
            started: self.started,
        }
    }
}

/// Retry state that outlives a single candidate inside one connect loop.
pub(super) struct ConnectProgress<'a> {
    pub(super) tried: &'a mut HashSet<String>,
    pub(super) repairs: &'a mut AttemptRepairs,
    pub(super) confirmed_response_missing: &'a mut bool,
    pub(super) last_failure: &'a mut Option<GatewayFailure>,
    pub(super) http_fallback_origin: &'a mut Option<ErrorOrigin>,
}

pub(super) struct Connected {
    pub(super) credential_fingerprint: [u8; 32],
    pub(super) authorization_incarnation: AuthorizationIncarnation,
    pub(super) native_search_declared: Option<bool>,
    pub(super) upstream: UpstreamWebSocket,
    pub(super) recovery: ConnectRecovery,
    pub(super) response_affinity_hit: bool,
    pub(super) route: ExecutorRoute,
    pub(super) request: ClientRequest,
    pub(super) reasoning_effort: crate::usage::ReasoningEffortDiagnostics,
    pub(super) tool_use: crate::ToolUseDiagnostics,
    pub(super) lease: CandidateLease,
    pub(super) attempt: u16,
    pub(super) started: Instant,
}
