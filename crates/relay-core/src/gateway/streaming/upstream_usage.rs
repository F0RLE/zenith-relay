use super::*;

/// Capture native counters before an adapter changes their wire representation.
pub(super) struct UpstreamUsage {
    pending: Vec<u8>,
    event: UsageEvent,
}

impl UpstreamUsage {
    pub(super) fn new(event: UsageEvent) -> Self {
        Self {
            pending: Vec::new(),
            event,
        }
    }

    pub(super) fn observe(&mut self, bytes: &[u8]) {
        if self.pending.len().saturating_add(bytes.len()) > MAX_SSE_EVENT_BYTES {
            self.pending.clear();
            return;
        }
        self.pending.extend_from_slice(bytes);
        while let Some(end) = sse_event_end(&self.pending) {
            let frame = self.pending.drain(..end).collect::<Vec<_>>();
            let parsed = parse_sse_event(&frame);
            if let Some(usage) = parsed.usage {
                apply_usage(&mut self.event, &usage);
            }
            if parsed.applied_service_tier.is_some() {
                self.event.applied_service_tier = parsed.applied_service_tier;
            }
        }
    }

    pub(super) fn apply_to(&self, event: &mut UsageEvent) {
        event.input_tokens = self.event.input_tokens;
        event.output_tokens = self.event.output_tokens;
        event.total_tokens = self.event.total_tokens;
        event.cached_input_tokens = self.event.cached_input_tokens;
        event.cache_write_input_tokens = self.event.cache_write_input_tokens;
        event
            .cache_write_ttl
            .clone_from(&self.event.cache_write_ttl);
        event.reasoning_tokens = self.event.reasoning_tokens;
        event
            .applied_service_tier
            .clone_from(&self.event.applied_service_tier);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_messages_usage_keeps_actual_cache_ttl_and_unknown_totals() {
        let mut capture = UpstreamUsage::new(crate::gateway::test_support::test_usage_event());
        let frames = b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":20,\"cache_creation_input_tokens\":30,\"cache_creation\":{\"ephemeral_1h_input_tokens\":30}}}}\n\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":4}}\n\n";
        for chunk in frames.chunks(5) {
            capture.observe(chunk);
        }
        let mut event = crate::gateway::test_support::test_usage_event();
        event.total_tokens = Some(0);
        capture.apply_to(&mut event);
        assert_eq!(event.input_tokens, Some(60));
        assert_eq!(event.output_tokens, Some(4));
        assert_eq!(event.cache_write_input_tokens, Some(30));
        assert_eq!(event.cache_write_ttl.as_deref(), Some("1h"));
        assert_eq!(event.total_tokens, None);
    }

    #[test]
    fn gemini_usage_preserves_reasoning_and_unknown_cache_counters() {
        let mut capture = UpstreamUsage::new(crate::gateway::test_support::test_usage_event());
        let frame = b"data: {\"candidates\":[{\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":4,\"thoughtsTokenCount\":7,\"totalTokenCount\":21}}\n\n";
        for chunk in frame.chunks(3) {
            capture.observe(chunk);
        }
        let mut event = crate::gateway::test_support::test_usage_event();
        capture.apply_to(&mut event);
        assert_eq!(event.input_tokens, Some(10));
        assert_eq!(event.output_tokens, Some(11));
        assert_eq!(event.reasoning_tokens, Some(7));
        assert_eq!(event.total_tokens, Some(21));
        assert_eq!(event.cached_input_tokens, None);
        assert_eq!(event.cache_write_input_tokens, None);
        assert_eq!(event.cache_write_ttl, None);
    }
}
