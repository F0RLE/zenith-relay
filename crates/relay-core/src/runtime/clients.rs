//! HTTP and WebSocket clients selected for one routed candidate.

use super::*;
use sha2::{Digest, Sha256};

const MAX_WEBSOCKET_OBSERVATIONS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WebsocketTransportPreference {
    Native,
    HttpOnly,
    SlowHandshake,
}

#[derive(Clone, Copy)]
enum WebsocketObservationKind {
    HttpOnly,
    SlowHandshakes(u8),
}

pub(super) struct WebsocketTransportObservation {
    observed_at_ms: u64,
    authorization: AuthorizationIncarnation,
    kind: WebsocketObservationKind,
}

impl WebsocketTransportObservation {
    fn is_current(&self, now_ms: u64) -> bool {
        let ttl_ms = match self.kind {
            WebsocketObservationKind::HttpOnly => WEBSOCKET_CAPABILITY_TTL_MS,
            WebsocketObservationKind::SlowHandshakes(_) => WEBSOCKET_FAST_PATH_BACKOFF_MS,
        };
        now_ms.saturating_sub(self.observed_at_ms) < ttl_ms
    }
}

impl GatewayRuntime {
    pub(super) fn direct_http_route(&self, candidate_id: &str) -> bool {
        self.chatgpt_accounts.get(candidate_id).map_or_else(
            || {
                self.source_candidate_bindings.contains_key(candidate_id)
                    && !self.source_http_clients.contains_key(candidate_id)
                    && self.clients.direct
            },
            |account| account.clients.direct,
        )
    }

    pub(crate) fn request_client(&self, candidate_id: &str) -> &reqwest::Client {
        if let Some(account) = self.chatgpt_accounts.get(candidate_id) {
            return &account.clients.http;
        }
        // Source candidates may have a per-source proxy; fall back to the
        // shared global client when no source-specific client exists.
        self.source_http_clients
            .get(candidate_id)
            .unwrap_or(&self.clients.http)
    }

    pub(crate) fn websocket_client(&self, candidate_id: &str) -> &reqwest::Client {
        self.chatgpt_accounts
            .get(candidate_id)
            .map(|account| &account.clients.websocket)
            .or_else(|| {
                // Source candidates with a proxy use a proxy-aware websocket
                // client; others share the global websocket client.
                self.source_websocket_clients.get(candidate_id)
            })
            .unwrap_or(&self.clients.websocket)
    }

    /// Capability observations belong to one endpoint and authorization incarnation.
    /// Clients and their proxy configuration are immutable for this runtime.
    pub(crate) fn websocket_capability_scope(
        route: &ExecutorRoute,
        prepared: &PreparedAuthorization,
    ) -> String {
        let mut digest = Sha256::new();
        for part in [
            route.candidate_id.as_bytes(),
            route.upstream_url.as_str().as_bytes(),
            route.source_model.as_bytes(),
            route.service_tier.as_str().as_bytes(),
        ] {
            digest.update((part.len() as u64).to_be_bytes());
            digest.update(part);
        }
        digest.update(prepared.credential_fingerprint());
        digest.update(prepared.token_generation.unwrap_or(0).to_be_bytes());
        digest.update(prepared.agent_identity_revision.unwrap_or(0).to_be_bytes());
        let mut headers: Vec<_> = route.upstream_headers.iter().collect();
        headers.sort_unstable_by(|left, right| {
            left.0
                .as_str()
                .cmp(right.0.as_str())
                .then(left.1.as_bytes().cmp(right.1.as_bytes()))
        });
        for (name, value) in headers {
            for part in [name.as_str().as_bytes(), value.as_bytes()] {
                digest.update((part.len() as u64).to_be_bytes());
                digest.update(part);
            }
        }
        hex::encode(digest.finalize())
    }

    pub(crate) fn websocket_transport_preference(
        &self,
        capability_scope: &str,
        model: &str,
        authorization: &AuthorizationIncarnation,
        now_ms: u64,
    ) -> WebsocketTransportPreference {
        let mut observations = crate::poison::mutex(&self.websocket_transport_observations);
        observations.retain(|_, observation| observation.is_current(now_ms));
        match observations
            .get(&(capability_scope.to_string(), model.to_string()))
            .filter(|observation| observation.authorization == *authorization)
            .map(|observation| observation.kind)
        {
            Some(WebsocketObservationKind::HttpOnly) => WebsocketTransportPreference::HttpOnly,
            // One slow handshake may recover immediately. Only repeated misses
            // defer optional probes; continuations still use their native owner.
            Some(WebsocketObservationKind::SlowHandshakes(2..)) => {
                WebsocketTransportPreference::SlowHandshake
            }
            _ => WebsocketTransportPreference::Native,
        }
    }

    pub(crate) fn mark_websocket_supported(
        &self,
        capability_scope: &str,
        model: &str,
        authorization: &AuthorizationIncarnation,
    ) {
        let mut observations = crate::poison::mutex(&self.websocket_transport_observations);
        let key = (capability_scope.to_string(), model.to_string());
        if observations
            .get(&key)
            .is_some_and(|observation| observation.authorization == *authorization)
        {
            observations.remove(&key);
        }
    }

    pub(crate) fn mark_websocket_http_only(
        &self,
        capability_scope: &str,
        model: &str,
        authorization: &AuthorizationIncarnation,
        now_ms: u64,
    ) {
        self.observe_websocket_transport(
            capability_scope,
            model,
            authorization,
            WebsocketObservationKind::HttpOnly,
            now_ms,
        );
    }

    pub(crate) fn observe_slow_websocket_handshake(
        &self,
        capability_scope: &str,
        model: &str,
        authorization: &AuthorizationIncarnation,
        now_ms: u64,
    ) {
        self.observe_websocket_transport(
            capability_scope,
            model,
            authorization,
            WebsocketObservationKind::SlowHandshakes(1),
            now_ms,
        );
    }

    fn observe_websocket_transport(
        &self,
        capability_scope: &str,
        model: &str,
        authorization: &AuthorizationIncarnation,
        mut kind: WebsocketObservationKind,
        now_ms: u64,
    ) {
        let mut observations = crate::poison::mutex(&self.websocket_transport_observations);
        observations.retain(|_, observation| observation.is_current(now_ms));
        let key = (capability_scope.to_string(), model.to_string());
        if matches!(kind, WebsocketObservationKind::SlowHandshakes(_))
            && let Some(previous) = observations
                .get(&key)
                .filter(|previous| previous.authorization == *authorization)
        {
            kind = match previous.kind {
                // A delayed timeout must not replace a confirmed rejection.
                WebsocketObservationKind::HttpOnly => return,
                WebsocketObservationKind::SlowHandshakes(count) => {
                    WebsocketObservationKind::SlowHandshakes(count.saturating_add(1))
                }
            };
        }
        if observations.len() >= MAX_WEBSOCKET_OBSERVATIONS
            && !observations.contains_key(&key)
            && let Some(oldest) = observations
                .iter()
                .min_by_key(|(_, observation)| observation.observed_at_ms)
                .map(|(key, _)| key.clone())
        {
            observations.remove(&oldest);
        }
        observations.insert(
            key,
            WebsocketTransportObservation {
                observed_at_ms: now_ms,
                authorization: authorization.clone(),
                kind,
            },
        );
    }

    /// The saved dispatch limit applies to the entire incoming request.
    pub(crate) fn request_dispatch_budget(&self) -> usize {
        self.max_retry_candidates.load(Ordering::Relaxed)
    }
}
