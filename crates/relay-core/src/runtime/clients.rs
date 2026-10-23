//! HTTP and WebSocket clients selected for one routed candidate.

use super::*;
use sha2::{Digest, Sha256};

const MAX_WEBSOCKET_CAPABILITY_ENTRIES: usize = 4096;

pub(super) struct WebsocketCapabilityObservation {
    observed_at_ms: u64,
    authorization: AuthorizationIncarnation,
}

impl GatewayRuntime {
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

    pub(crate) fn websocket_is_http_only(
        &self,
        capability_scope: &str,
        model: &str,
        authorization: &AuthorizationIncarnation,
        now_ms: u64,
    ) -> bool {
        let mut observations = crate::poison::mutex(&self.websocket_http_only);
        observations.retain(|_, observation| {
            now_ms.saturating_sub(observation.observed_at_ms) < WEBSOCKET_CAPABILITY_TTL_MS
        });
        observations
            .get(&(capability_scope.to_string(), model.to_string()))
            .is_some_and(|observation| observation.authorization == *authorization)
    }

    pub(crate) fn mark_websocket_supported(
        &self,
        capability_scope: &str,
        model: &str,
        authorization: &AuthorizationIncarnation,
    ) {
        let mut observations = crate::poison::mutex(&self.websocket_http_only);
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
        let mut observations = crate::poison::mutex(&self.websocket_http_only);
        observations.retain(|_, observation| {
            now_ms.saturating_sub(observation.observed_at_ms) < WEBSOCKET_CAPABILITY_TTL_MS
        });
        let key = (capability_scope.to_string(), model.to_string());
        if observations.len() >= MAX_WEBSOCKET_CAPABILITY_ENTRIES
            && !observations.contains_key(&key)
        {
            if let Some(oldest) = observations
                .iter()
                .min_by_key(|(_, observation)| observation.observed_at_ms)
                .map(|(key, _)| key.clone())
            {
                observations.remove(&oldest);
            }
        }
        observations.insert(
            key,
            WebsocketCapabilityObservation {
                observed_at_ms: now_ms,
                authorization: authorization.clone(),
            },
        );
    }

    /// The saved dispatch limit applies to the entire incoming request.
    pub(crate) fn request_dispatch_budget(&self) -> usize {
        self.max_retry_candidates.load(Ordering::Relaxed)
    }
}
