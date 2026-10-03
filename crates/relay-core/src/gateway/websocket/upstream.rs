mod connect;
mod drive;
mod handshake;
mod initial_terminal;
mod messages;
mod open;
mod selection_gap;
mod session;
mod telemetry;
mod upgrade_reject;

use std::collections::HashSet;

use super::super::execution::AttemptRepairs;
use super::*;

#[cfg(test)]
pub(in crate::gateway::websocket) use messages::initial_payloads_are_empty_incomplete;
pub(in crate::gateway::websocket) use messages::{
    first_message_terminal, message_serves_rejected_model,
};
pub(in crate::gateway::websocket) use session::{
    await_while_client_connected, connect_upstream_while_client_connected,
};

/// The candidate reserved for the WebSocket upgrade that is in progress.
pub(super) struct ConnectScope<'a> {
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
    pub(super) upstream: UpstreamWebSocket,
    pub(super) initial_messages: Vec<UpstreamMessage>,
    pub(super) route: ExecutorRoute,
    pub(super) request: ClientRequest,
    pub(super) lease: CandidateLease,
    pub(super) attempt: u16,
    pub(super) started: Instant,
}
