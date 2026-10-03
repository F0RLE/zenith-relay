use super::*;

/// Inspect native counters and identity before an adapter changes their representation.
pub(super) struct UpstreamUsage {
    pending: Vec<u8>,
    event: UsageEvent,
    expected_model: Option<String>,
    rejection: Option<&'static str>,
    forward_pending: Vec<u8>,
}

impl UpstreamUsage {
    pub(super) fn new(event: UsageEvent, expected_model: Option<String>) -> Self {
        Self {
            pending: Vec::new(),
            event,
            expected_model,
            rejection: None,
            forward_pending: Vec::new(),
        }
    }

    pub(super) fn forward(&mut self, bytes: Bytes) -> Bytes {
        if self.expected_model.is_none() {
            self.observe(&bytes);
            return bytes;
        }
        self.forward_pending.extend_from_slice(&bytes);
        if !self.observe(&bytes) {
            self.forward_pending.clear();
            let category = self.rejection.unwrap_or(error_codes::STREAM_INVALID);
            let error = json!({"error": {
                "code": category,
                "message": if category == error_codes::UPSTREAM_ROUTE_DEGRADED {
                    "Upstream served a different or internally degraded model"
                } else {
                    "Upstream stream event exceeds the inspection limit"
                }
            }});
            return Bytes::from(format!("data: {error}\n\n"));
        }
        // An adapter must never receive half a model-bearing event: otherwise
        // its next chunk could join that half to our rejection and hide the cause.
        let complete = self
            .forward_pending
            .len()
            .saturating_sub(self.pending.len());
        let tail = self.forward_pending.split_off(complete);
        Bytes::from(std::mem::replace(&mut self.forward_pending, tail))
    }

    pub(super) fn observe(&mut self, bytes: &[u8]) -> bool {
        if self.rejection.is_some() {
            return false;
        }
        if self.pending.len().saturating_add(bytes.len()) > MAX_SSE_EVENT_BYTES {
            self.pending.clear();
            if self.expected_model.is_some() {
                self.rejection = Some(error_codes::STREAM_EVENT_TOO_LARGE);
                return false;
            }
            return true;
        }
        let bytes = if self.pending.is_empty() {
            skip_aligned_response_deltas(bytes)
        } else {
            bytes
        };
        if bytes.is_empty() {
            return true;
        }
        self.pending.extend_from_slice(bytes);
        while let Some(frame) = crate::protocol::take_sse_event(&mut self.pending) {
            if super::fast_delta::fast_response_delta(&frame).is_some() {
                continue;
            }
            let parsed = parse_sse_event(&frame);
            if let Some(usage) = &parsed.usage {
                apply_usage(&mut self.event, usage);
            }
            if parsed.applied_service_tier.is_some() {
                self.event.applied_service_tier = parsed.applied_service_tier;
            }
            if self.expected_model.as_deref().is_some_and(|expected| {
                parsed
                    .payload
                    .as_ref()
                    .is_some_and(|value| served_model_is_rejected(value, expected))
            }) {
                self.pending.clear();
                self.rejection = Some(error_codes::UPSTREAM_ROUTE_DEGRADED);
                return false;
            }
        }
        true
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

fn skip_aligned_response_deltas(bytes: &[u8]) -> &[u8] {
    let mut offset = 0;
    while offset < bytes.len() {
        let Some(end) = crate::protocol::sse_event_end(&bytes[offset..]) else {
            break;
        };
        if super::fast_delta::fast_response_delta(&bytes[offset..offset + end]).is_none() {
            break;
        }
        offset += end;
    }
    &bytes[offset..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_partial_frame_cannot_bypass_identity_inspection() {
        let mut capture = UpstreamUsage::new(
            crate::gateway::test_support::test_usage_event(),
            Some("gpt-6-astra".into()),
        );
        assert!(capture
            .forward(Bytes::from_static(b"data: {\"model\":\""))
            .is_empty());
        let rejected = capture.forward(Bytes::from(vec![b'x'; MAX_SSE_EVENT_BYTES]));
        let event = parse_sse_event(&rejected);
        assert!(event.valid);
        assert_eq!(
            event.payload.unwrap()["error"]["code"],
            error_codes::STREAM_EVENT_TOO_LARGE
        );
        assert!(capture.pending.is_empty());
        assert!(capture.forward_pending.is_empty());
        assert!(!capture.observe(b"data: [DONE]\n\n"));
    }

    #[test]
    fn mismatched_terminal_retains_reported_usage() {
        let mut capture = UpstreamUsage::new(
            crate::gateway::test_support::test_usage_event(),
            Some("gpt-6-astra".into()),
        );
        assert!(!capture.observe(b"data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-luna\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"total_tokens\":5}}}\n\n"));
        let mut event = crate::gateway::test_support::test_usage_event();
        capture.apply_to(&mut event);
        assert_eq!(event.input_tokens, Some(3));
        assert_eq!(event.output_tokens, Some(2));
        assert_eq!(event.total_tokens, Some(5));
    }

    #[test]
    fn split_rejection_reaches_the_adapter_as_one_valid_error() {
        let mut capture = UpstreamUsage::new(
            crate::gateway::test_support::test_usage_event(),
            Some("gpt-6-astra".into()),
        );
        let delta = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n";
        let partial = b"data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-";
        assert_eq!(
            capture
                .forward(Bytes::from([delta.as_slice(), partial.as_slice()].concat()))
                .as_ref(),
            delta
        );
        let rejected = capture.forward(Bytes::from_static(b"luna\"}}\n\n"));
        let event = parse_sse_event(&rejected);
        assert!(event.valid);
        assert_eq!(
            event.error_category,
            Some(error_codes::UPSTREAM_ROUTE_DEGRADED)
        );
    }

    #[test]
    fn native_identity_is_checked_across_chunks_before_translation() {
        let mut capture = UpstreamUsage::new(
            crate::gateway::test_support::test_usage_event(),
            Some("gpt-6-astra".into()),
        );
        assert!(capture.observe(
            b"data: {\"type\":\"response.created\",\"response\":{\"model\":\"gpt-6-astra\"}}\n\n"
        ));
        assert!(capture
            .observe(b"data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-"));
        assert!(!capture.observe(b"luna\"}}\n\n"));
        assert!(!capture.observe(b"data: [DONE]\n\n"));
    }

    #[test]
    fn split_messages_usage_keeps_actual_cache_ttl_and_unknown_totals() {
        let mut capture =
            UpstreamUsage::new(crate::gateway::test_support::test_usage_event(), None);
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
        let mut capture =
            UpstreamUsage::new(crate::gateway::test_support::test_usage_event(), None);
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

    #[test]
    fn response_deltas_keep_terminal_usage_and_explicit_delta_usage() {
        let mut capture =
            UpstreamUsage::new(crate::gateway::test_support::test_usage_event(), None);
        let frames = b"data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"total_tokens\":5}}}\n\n";
        for chunk in frames.chunks(4) {
            capture.observe(chunk);
        }
        let mut event = crate::gateway::test_support::test_usage_event();
        capture.apply_to(&mut event);
        assert_eq!(event.input_tokens, Some(3));
        assert_eq!(event.output_tokens, Some(2));
        assert_eq!(event.total_tokens, Some(5));

        let mut capture =
            UpstreamUsage::new(crate::gateway::test_support::test_usage_event(), None);
        capture.observe(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\",\"usage\":{\"input_tokens\":9,\"output_tokens\":1,\"total_tokens\":10}}\n\n");
        let mut event = crate::gateway::test_support::test_usage_event();
        capture.apply_to(&mut event);
        assert_eq!(event.input_tokens, Some(9));
        assert_eq!(event.output_tokens, Some(1));
    }
}
