use super::*;

/// Inspect native counters and identity before an adapter changes their representation.
pub(super) struct UpstreamUsage {
    pending: Vec<u8>,
    event: UsageEvent,
    usage_snapshot: Option<Value>,
    expected_model: Option<String>,
    rejection: Option<&'static str>,
    finished: bool,
    timing: Option<(Instant, super::super::response::BufferedResponseTiming)>,
}

impl UpstreamUsage {
    pub(super) fn new(event: UsageEvent, expected_model: Option<String>) -> Self {
        Self {
            pending: Vec::new(),
            event,
            usage_snapshot: None,
            expected_model,
            rejection: None,
            finished: false,
            timing: None,
        }
    }

    pub(super) fn with_timing(mut self, started: Instant) -> Self {
        self.timing = Some((
            started,
            super::super::response::BufferedResponseTiming::default(),
        ));
        self
    }

    pub(super) fn reset_generation(&mut self) {
        self.finished = false;
        self.pending.clear();
        self.usage_snapshot = None;
        self.event.input_tokens = None;
        self.event.output_tokens = None;
        self.event.total_tokens = None;
        self.event.cached_input_tokens = None;
        self.event.cache_write_input_tokens = None;
        self.event.cache_write_ttl = None;
        self.event.reasoning_tokens = None;
        self.event.applied_service_tier = None;
        if let Some(routing) = self.event.routing.as_mut() {
            routing.upstream_response_model = None;
        }
        self.event.tool_use.upstream_tool_call_count = None;
    }

    pub(super) fn finish_json_generation(&mut self, bytes: &[u8]) {
        if let Ok(payload) = serde_json::from_slice::<Value>(bytes) {
            self.event.observe_response_model(&payload);
            self.event.tool_use.observe_upstream_response(&payload);
            if let Some(usage) = payload.get("usage") {
                apply_usage(&mut self.event, usage);
            }
        }
        // JSON gives a real terminal arrival, not an incremental first token.
        if let Some((started, timing)) = &mut self.timing {
            timing.finish(started.elapsed().as_millis() as u64);
        }
    }

    pub(super) fn forward(&mut self, bytes: Bytes) -> Bytes {
        if self.expected_model.is_none() {
            if self.observe(&bytes) {
                return bytes;
            }
        } else {
            // Identity validation and forwarding share one incomplete-frame buffer.
            let mut forwarded = Vec::new();
            if self.observe_frames(&bytes, |frame| forwarded.extend_from_slice(frame)) {
                return Bytes::from(forwarded);
            }
        }
        let category = self.rejection.unwrap_or(error_codes::STREAM_INVALID);
        let error = json!({"error": {
            "type": error_codes::STREAM_ERROR,
            "code": category,
            "message": if category == error_codes::UPSTREAM_ROUTE_DEGRADED {
                "Upstream served a different or internally degraded model"
            } else if category == error_codes::STREAM_EVENT_TOO_LARGE {
                "Upstream streaming event exceeded Relay's size limit"
            } else {
                "Upstream stream event is invalid"
            }
        }});
        Bytes::from(format!("data: {error}\n\n"))
    }

    pub(super) fn observe(&mut self, bytes: &[u8]) -> bool {
        let bytes = if self.pending.is_empty() && self.timing.is_none() {
            skip_aligned_response_deltas(bytes)
        } else {
            bytes
        };
        self.observe_frames(bytes, |_| {})
    }

    fn observe_frames(&mut self, bytes: &[u8], mut forward: impl FnMut(&[u8])) -> bool {
        if self.rejection.is_some() {
            return false;
        }
        if self.finished || bytes.is_empty() {
            return true;
        }
        let pending = crate::protocol::push_pending_sse_frames(
            std::mem::take(&mut self.pending),
            bytes,
            self,
            |capture| capture.rejection.is_some() || capture.finished,
            |capture, frame| {
                capture.observe_frame(frame);
                forward(frame);
            },
        );
        match pending {
            Ok(pending) => self.pending = pending,
            Err(()) => self.rejection = Some(error_codes::STREAM_EVENT_TOO_LARGE),
        }
        self.rejection.is_none()
    }

    fn observe_frame(&mut self, frame: &[u8]) {
        if self.finished {
            return;
        }
        if self.timing.is_none() && super::fast_delta::fast_response_delta(frame).is_some() {
            return;
        }
        let parsed = parse_sse_event(frame);
        if let Some(payload) = parsed.event_payload.as_ref() {
            self.event.observe_response_model(payload);
        }
        if parsed.outcome.is_some() {
            // Gemini can report final candidate status before a separate usage chunk.
            self.finished = !parsed
                .event_payload
                .as_ref()
                .is_some_and(crate::protocol::gemini_incomplete);
        }
        if let Some((started, timing)) = self.timing.as_mut() {
            let elapsed = started.elapsed().as_millis() as u64;
            timing.observe(&parsed, elapsed);
            if parsed.outcome.is_some() {
                timing.finish(elapsed);
                if let Some(payload) = parsed.event_payload.as_ref() {
                    self.event.tool_use.observe_upstream_response(payload);
                }
            }
        }
        if let Some(usage) = &parsed.usage {
            crate::protocol::merge_usage(&mut self.usage_snapshot, usage);
            if let Some(snapshot) = &self.usage_snapshot {
                apply_usage(&mut self.event, snapshot);
            }
        }
        if parsed.applied_service_tier.is_some() {
            self.event.applied_service_tier = parsed.applied_service_tier;
        }
        if self.expected_model.as_deref().is_some_and(|expected| {
            parsed
                .event_payload
                .as_ref()
                .is_some_and(|served_model| served_model_is_rejected(served_model, expected))
        }) {
            self.rejection = Some(error_codes::UPSTREAM_ROUTE_DEGRADED);
        }
    }

    pub(super) fn apply_to(&self, event: &mut UsageEvent) {
        if let (Some(source), Some(target)) = (self.event.routing.as_ref(), event.routing.as_mut())
        {
            target
                .upstream_response_model
                .clone_from(&source.upstream_response_model);
        }
        if let Some((_, timing)) = &self.timing {
            timing.apply(event);
            event.tool_use.upstream_tool_call_count = self.event.tool_use.upstream_tool_call_count;
        }
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
        if end > crate::protocol::sse::MAX_EVENT_BYTES
            || super::fast_delta::fast_response_delta(&bytes[offset..offset + end]).is_none()
        {
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
    fn raw_model_declarations_survive_conversion_and_reset_between_generations() {
        let mut event = crate::gateway::test_support::test_usage_event();
        event.routing = Some(
            serde_json::from_value(json!({
                "reason":"only_eligible", "eligibleCandidates":1,
                "quotaRemainingBasisPoints":null, "inFlightBefore":0, "dispatchesBefore":0
            }))
            .unwrap(),
        );
        let mut capture = UpstreamUsage::new(event.clone(), None);
        let frames = b"data: {\"type\":\"response.created\",\"response\":{\"model\":\"synthetic-upstream\"}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":2}}}\n\n";
        for chunk in frames.chunks(3) {
            assert!(capture.observe(chunk));
        }
        event.observe_response_model(&json!({"model":"synthetic-client-alias"}));
        capture.apply_to(&mut event);
        assert_eq!(
            event
                .routing
                .as_ref()
                .unwrap()
                .upstream_response_model
                .as_deref(),
            Some("synthetic-upstream")
        );
        assert_eq!(event.output_tokens, Some(2));
        capture.reset_generation();
        capture.finish_json_generation(br#"{"model":"synthetic-next","usage":{"input_tokens":3}}"#);
        capture.apply_to(&mut event);
        assert_eq!(
            event
                .routing
                .as_ref()
                .unwrap()
                .upstream_response_model
                .as_deref(),
            Some("synthetic-next")
        );
        capture.reset_generation();
        capture.apply_to(&mut event);
        assert!(event.routing.unwrap().upstream_response_model.is_none());
    }

    #[test]
    fn oversized_events_reject_forwarding_with_or_without_identity_checks() {
        for expected in [None, Some("synthetic-model".to_string())] {
            let mut capture =
                UpstreamUsage::new(crate::gateway::test_support::test_usage_event(), expected);
            assert!(
                capture.observe(b"data: {\"usage\":{\"input_tokens\":7,\"output_tokens\":3}}\n\n")
            );
            capture
                .pending
                .resize(crate::protocol::sse::MAX_EVENT_BYTES, b'x');
            let failure = capture.forward(Bytes::from_static(b"x"));
            assert_eq!(
                parse_sse_event(&failure).error_category,
                Some(error_codes::STREAM_EVENT_TOO_LARGE)
            );
            assert!(capture.pending.is_empty());
            assert!(!capture.observe(b"data: [DONE]\n\n"));
            let mut event = crate::gateway::test_support::test_usage_event();
            capture.apply_to(&mut event);
            assert_eq!(event.input_tokens, Some(7));
            assert_eq!(event.output_tokens, Some(3));
        }
    }

    #[test]
    fn incomplete_identity_frames_are_buffered_once_until_validated() {
        let frame = b"data: {\"type\":\"response.created\",\"response\":{\"model\":\"synthetic-model\"}}\n\n";
        for split in 1..frame.len() {
            let mut capture = UpstreamUsage::new(
                crate::gateway::test_support::test_usage_event(),
                Some("synthetic-model".into()),
            );
            assert!(capture
                .forward(Bytes::copy_from_slice(&frame[..split]))
                .is_empty());
            assert_eq!(capture.pending, frame[..split]);
            assert_eq!(
                capture.forward(Bytes::copy_from_slice(&frame[split..])),
                frame.as_slice()
            );
            assert!(capture.pending.is_empty());
        }
    }

    #[test]
    fn completed_generations_ignore_trailing_usage_tier_and_model_changes() {
        for terminal in [
            "data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-6-astra\",\"status\":\"completed\",\"output\":[]}}\n\n",
            "data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"output\":[]}}\n\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"synthetic failure\"}}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
            "data: [DONE]\n\n",
        ] {
            let mut capture = UpstreamUsage::new(
                crate::gateway::test_support::test_usage_event(),
                Some("gpt-6-astra".into()),
            );
            let usage = "data: {\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}\n\n";
            let trailing = "data: {\"model\":\"gpt-5.6-luna\",\"service_tier\":\"priority\",\"usage\":{\"input_tokens\":999,\"output_tokens\":999}}\n\n";
            assert!(capture.observe(format!("{usage}{terminal}{trailing}").as_bytes()));
            assert!(capture.observe(trailing.as_bytes()));
            let mut event = crate::gateway::test_support::test_usage_event();
            capture.apply_to(&mut event);
            assert_eq!(event.input_tokens, Some(3));
            assert_eq!(event.output_tokens, Some(2));
            assert_eq!(event.applied_service_tier, None);
            capture.reset_generation();
            assert!(capture.observe(b"data: {\"usage\":{\"input_tokens\":7}}\n\n"));
            capture.apply_to(&mut event);
            assert_eq!(event.input_tokens, Some(7));
        }
    }

    #[test]
    fn gemini_late_usage_survives_both_complete_and_incomplete_candidates() {
        for reason in ["STOP", "MAX_TOKENS"] {
            let mut capture =
                UpstreamUsage::new(crate::gateway::test_support::test_usage_event(), None);
            let candidate = json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"Hi"}]},"finishReason":reason}]});
            assert!(capture.observe(format!("data: {candidate}\n\n").as_bytes()));
            assert!(capture.observe(b"data: {\"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":2,\"totalTokenCount\":5}}\n\n"));
            let mut event = crate::gateway::test_support::test_usage_event();
            capture.apply_to(&mut event);
            assert_eq!(event.input_tokens, Some(3));
            assert_eq!(event.output_tokens, Some(2));
            assert_eq!(event.total_tokens, Some(5));
        }
    }

    #[test]
    fn discovery_generation_reset_does_not_reuse_prior_usage() {
        let mut capture =
            UpstreamUsage::new(crate::gateway::test_support::test_usage_event(), None);
        capture.observe(b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":100,\"output_tokens\":10}}}\n\n");
        capture.reset_generation();
        capture.observe(
            b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
        );
        let mut event = crate::gateway::test_support::test_usage_event();
        capture.apply_to(&mut event);
        assert_eq!(event.input_tokens, None);
        assert_eq!(event.output_tokens, None);
        assert_eq!(event.total_tokens, None);
    }

    #[test]
    fn raw_tool_timing_and_partial_usage_survive_without_a_terminal() {
        let started = Instant::now() - Duration::from_millis(100);
        let mut capture =
            UpstreamUsage::new(crate::gateway::test_support::test_usage_event(), None)
                .with_timing(started);
        capture.observe(b"data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\",\"usage\":{\"input_tokens\":7}}\n\n");
        let mut event = crate::gateway::test_support::test_usage_event();
        event.latency_ms = 500;
        capture.apply_to(&mut event);
        assert!(event.ttft_ms.is_some_and(|first| first >= 100));
        assert_eq!(
            event.generation_ms,
            event.ttft_ms.map(|first| 500_u64.saturating_sub(first))
        );
        assert_eq!(event.input_tokens, Some(7));
        assert_eq!(event.output_tokens, None);
        assert_eq!(event.tool_use.upstream_tool_call_count, None);
        assert_eq!(event.tool_use.tool_calls_ready_ms, None);
    }

    #[test]
    fn bps_native_call_count_resets_between_discovery_generations() {
        let mut initial = crate::gateway::test_support::test_usage_event();
        initial.tool_use.client_context = Some(Default::default());
        let mut capture = UpstreamUsage::new(initial, None).with_timing(Instant::now());
        capture.observe(b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"function_call\"}]}}\n\n");
        let mut event = crate::gateway::test_support::test_usage_event();
        capture.apply_to(&mut event);
        assert_eq!(event.tool_use.upstream_tool_call_count, Some(1));
        capture.reset_generation();
        capture.apply_to(&mut event);
        assert_eq!(event.tool_use.upstream_tool_call_count, None);
        capture.finish_json_generation(br#"{"status":"completed","output":[]}"#);
        capture.apply_to(&mut event);
        assert_eq!(event.tool_use.upstream_tool_call_count, Some(0));
        assert_eq!(event.tool_use.tool_calls_ready_ms, None);
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
    fn forwarded_frames_preserve_usage_timing_and_discard_terminal_tail_at_every_split() {
        let frames = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"model\":\"gpt-6-astra\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\\\"text\\\":\\\"hi\\nthere\\\"}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-6-astra\",\"service_tier\":\"default\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"total_tokens\":5}}}\n\n",
        );
        let tail = b"data: {\"type\":\"response.";
        let input = [frames.as_bytes(), tail].concat();
        for timed in [false, true] {
            for chunk_size in 1..=input.len() {
                let mut capture = UpstreamUsage::new(
                    crate::gateway::test_support::test_usage_event(),
                    Some("gpt-6-astra".into()),
                );
                if timed {
                    capture = capture.with_timing(Instant::now() - Duration::from_millis(100));
                }
                let mut forwarded = Vec::new();
                for chunk in input.chunks(chunk_size) {
                    forwarded.extend_from_slice(&capture.forward(Bytes::copy_from_slice(chunk)));
                }
                assert_eq!(forwarded, frames.as_bytes(), "chunk size {chunk_size}");
                assert!(capture.pending.is_empty(), "chunk size {chunk_size}");
                let mut event = crate::gateway::test_support::test_usage_event();
                capture.apply_to(&mut event);
                assert_eq!(event.input_tokens, Some(3));
                assert_eq!(event.output_tokens, Some(2));
                assert_eq!(event.total_tokens, Some(5));
                assert_eq!(event.applied_service_tier.as_deref(), Some("default"));
                if timed {
                    assert!(event.ttft_ms.is_some_and(|first| first >= 100));
                    assert!(event.generation_ms.is_some());
                }
            }
        }
    }

    #[test]
    fn coalesced_rejection_discards_later_usage_and_partial_events() {
        let mut capture = UpstreamUsage::new(
            crate::gateway::test_support::test_usage_event(),
            Some("gpt-6-astra".into()),
        );
        let frames = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"model\":\"gpt-6-astra\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-luna\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"total_tokens\":5}}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":999}}}\n\n",
            "data: {\"type\":\"response.",
        );
        let rejected = capture.forward(Bytes::from_static(frames.as_bytes()));
        assert_eq!(
            parse_sse_event(&rejected).error_category,
            Some(error_codes::UPSTREAM_ROUTE_DEGRADED)
        );
        assert!(capture.pending.is_empty());
        assert!(capture.pending.is_empty());
        assert!(!capture.observe(b"data: [DONE]\n\n"));
        let mut event = crate::gateway::test_support::test_usage_event();
        capture.apply_to(&mut event);
        assert_eq!(event.input_tokens, Some(3));
        assert_eq!(event.output_tokens, Some(2));
        assert_eq!(event.total_tokens, Some(5));
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
    fn split_messages_usage_keeps_actual_cache_ttl_and_derives_total() {
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
        assert_eq!(event.total_tokens, Some(64));
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
