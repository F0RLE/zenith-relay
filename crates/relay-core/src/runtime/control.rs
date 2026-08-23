use crate::{DefaultServiceTier, UsageCallback, UsageEvent, WireApi};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

const MAX_TRACKED_REQUEST_ORIGINS: usize = 4096;

/// Mutable runtime controls that are changed by management commands while the
/// routing graph itself remains immutable for the lifetime of a runtime.
pub(crate) struct RuntimeControl {
    codex_background_tasks_enabled: AtomicBool,
    codex_websockets_enabled: AtomicBool,
    request_origins: Mutex<BTreeMap<String, &'static str>>,
}

impl Default for RuntimeControl {
    fn default() -> Self {
        Self {
            codex_background_tasks_enabled: AtomicBool::new(true),
            codex_websockets_enabled: AtomicBool::new(true),
            request_origins: Mutex::new(BTreeMap::new()),
        }
    }
}

impl RuntimeControl {
    pub(crate) fn codex_background_tasks_enabled(&self) -> bool {
        self.codex_background_tasks_enabled.load(Ordering::Acquire)
    }

    pub(crate) fn set_codex_background_tasks_enabled(&self, enabled: bool) {
        self.codex_background_tasks_enabled
            .store(enabled, Ordering::Release);
    }

    pub(crate) fn codex_websockets_enabled(&self) -> bool {
        self.codex_websockets_enabled.load(Ordering::Acquire)
    }

    pub(crate) fn set_codex_websockets_enabled(&self, enabled: bool) {
        self.codex_websockets_enabled
            .store(enabled, Ordering::Release);
    }

    pub(crate) fn mark_request_origin(&self, request_id: &str, origin: &'static str) {
        let mut origins = self
            .request_origins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        origins.insert(request_id.to_string(), origin);
        if origins.len() > MAX_TRACKED_REQUEST_ORIGINS {
            let excess = origins.len() - MAX_TRACKED_REQUEST_ORIGINS;
            let old = origins.keys().take(excess).cloned().collect::<Vec<_>>();
            for request_id in old {
                origins.remove(&request_id);
            }
        }
    }

    pub(crate) fn request_origin(&self, request_id: &str) -> Option<&'static str> {
        self.request_origins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(request_id)
            .copied()
    }

    pub(crate) fn blocked_codex_background_event(
        &self,
        usage: &UsageCallback,
        request_id: &str,
        local_key_id: &str,
        requested_model: &str,
        wire_api: WireApi,
        origin: &'static str,
    ) {
        let event = UsageEvent {
            request_id: request_id.to_string(),
            attempt: 1,
            local_key_id: local_key_id.to_string(),
            source_id: "relay".to_string(),
            candidate_id: None,
            account_id: None,
            client_context_id: None,
            routing: None,
            requested_model: Some(requested_model.to_string()),
            resolved_model: None,
            requested_reasoning_effort: None,
            effective_reasoning_effort: None,
            wire_api,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: true,
            http_status: 200,
            error_category: Some(format!("codex_background_blocked_{origin}")),
            tool_use: Default::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: None,
            latency_ms: 0,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: Some(0),
            cached_input_tokens: Some(0),
            cache_write_input_tokens: Some(0),
            cache_write_ttl: None,
            reasoning_tokens: Some(0),
            output_tokens: Some(0),
            total_tokens: Some(0),
            quota_snapshot: None,
        };
        usage(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_default_to_enabled() {
        let control = RuntimeControl::default();
        assert!(control.codex_background_tasks_enabled());
        assert!(control.codex_websockets_enabled());
    }

    #[test]
    fn request_origin_registry_is_bounded() {
        let control = RuntimeControl::default();
        for index in 0..=MAX_TRACKED_REQUEST_ORIGINS {
            control.mark_request_origin(&index.to_string(), "test");
        }
        assert_eq!(control.request_origin("0"), None);
        assert_eq!(control.request_origin("4096"), Some("test"));
    }
}
