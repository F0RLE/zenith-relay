//! HTTP and WebSocket clients selected for one routed candidate.

use super::*;

impl GatewayRuntime {
    pub(crate) fn request_client(&self, candidate_id: &str) -> &reqwest::Client {
        if let Some(account) = self.chatgpt_accounts.get(candidate_id) {
            return &account.clients.http;
        }
        &self.clients.http
    }

    pub(crate) fn websocket_client(&self, candidate_id: &str) -> &reqwest::Client {
        self.chatgpt_accounts
            .get(candidate_id)
            .map(|account| &account.clients.websocket)
            .unwrap_or(&self.clients.websocket)
    }

    pub(crate) fn websocket_is_http_only(
        &self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
    ) -> bool {
        self.websocket_http_only
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(candidate_id.to_string(), model.to_string()))
            .is_some_and(|observed_at| {
                now_ms.saturating_sub(*observed_at) < WEBSOCKET_CAPABILITY_TTL_MS
            })
    }

    pub(crate) fn mark_websocket_supported(&self, candidate_id: &str, model: &str) {
        self.websocket_http_only
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(candidate_id.to_string(), model.to_string()));
    }

    pub(crate) fn mark_websocket_http_only(&self, candidate_id: &str, model: &str, now_ms: u64) {
        self.websocket_http_only
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((candidate_id.to_string(), model.to_string()), now_ms);
    }

    /// The saved dispatch limit applies to the entire incoming request.
    pub(crate) fn request_dispatch_budget(&self) -> usize {
        self.max_retry_candidates.load(Ordering::Relaxed)
    }
}
