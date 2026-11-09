use super::*;

/// Inspect native counters and identity before an adapter changes their representation.
pub(in crate::gateway) struct UpstreamUsage {
    pending: Vec<u8>,
    event: UsageEvent,
    usage_snapshot: Option<Value>,
    usage_revision: u64,
    expected_model: Option<String>,
    rejection: Option<&'static str>,
    finished: bool,
    timing: Option<(Instant, super::super::response::BufferedResponseTiming)>,
    observed_started: Option<Instant>,
}

impl UpstreamUsage {
    pub(in crate::gateway) fn new(event: UsageEvent, expected_model: Option<String>) -> Self {
        Self {
            pending: Vec::new(),
            event,
            usage_snapshot: None,
            usage_revision: 0,
            expected_model,
            rejection: None,
            finished: false,
            timing: None,
            observed_started: None,
        }
    }

    pub(super) fn with_timing(mut self, started: Instant) -> Self {
        self.observed_started = Some(started);
        self.timing = Some((
            started,
            super::super::response::BufferedResponseTiming::default(),
        ));
        self
    }

    pub(super) fn with_observations(mut self, started: Instant) -> Self {
        self.observed_started = Some(started);
        self
    }

    pub(super) fn reset_generation(&mut self) {
        self.finished = false;
        if let Some((_, timing)) = &mut self.timing {
            *timing = super::super::response::BufferedResponseTiming::default();
        }
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
            if let Some(timing) = routing
                .transport
                .as_mut()
                .and_then(|transport| transport.stream_response.as_mut())
            {
                timing.upstream_reasoning_first_ms = None;
                timing.upstream_text_first_ms = None;
                timing.upstream_tool_first_ms = None;
                timing.upstream_tool_first_complete_ms = None;
            }
            if let Some(cache) = routing.cache_context.as_mut() {
                cache.provider = None;
                cache.provider_response_id = None;
            }
        }
        self.event.tool_use.upstream_tool_call_count = None;
        self.event.tool_use.provider_usage = None;
    }

    pub(super) fn finish_json_generation(&mut self, bytes: &[u8]) {
        if let Ok(payload) = serde_json::from_slice::<Value>(bytes) {
            self.event.observe_response_metadata(&payload);
            self.event.tool_use.observe_provider_usage(&payload);
            self.event.tool_use.observe_upstream_response(&payload);
            self.event.applied_service_tier = response_service_tier(&payload);
            if let Some(usage) = find_usage(&payload) {
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

    pub(in crate::gateway) fn observe(&mut self, bytes: &[u8]) -> bool {
        let bytes = if self.pending.is_empty()
            && self.timing.is_none()
            && self.observed_started.is_none()
        {
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
        if let Some(delta) = super::fast_delta::fast_response_delta(frame) {
            if let Some(started) = self.observed_started
                && let Some(transport) = self
                    .event
                    .routing
                    .as_mut()
                    .and_then(|routing| routing.transport.as_mut())
            {
                transport
                    .stream_response
                    .get_or_insert_default()
                    .observe_delta(
                        delta.output_text,
                        delta.tool_arguments,
                        delta.nonempty_text,
                        started.elapsed().as_millis() as u64,
                    );
            }
            if delta.nonempty_text
                && let Some((started, timing)) = &mut self.timing
            {
                timing.observe_first_output(started.elapsed().as_millis() as u64);
            }
            return;
        }
        self.observe_full_frame(frame);
    }

    fn observe_full_frame(&mut self, frame: &[u8]) {
        let parsed = parse_sse_event(frame);
        if let Some(payload) = parsed.event_payload.as_ref() {
            self.event.observe_response_metadata(payload);
            self.event.tool_use.observe_provider_usage(payload);
            if let Some(started) = self.observed_started
                && let Some(transport) = self
                    .event
                    .routing
                    .as_mut()
                    .and_then(|routing| routing.transport.as_mut())
            {
                let timing = transport.stream_response.get_or_insert_default();
                let elapsed_ms = started.elapsed().as_millis() as u64;
                timing.observe_reasoning(payload, elapsed_ms);
                timing.observe_output(payload, elapsed_ms);
            }
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
            self.usage_revision = self.usage_revision.saturating_add(1);
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

    pub(in crate::gateway) fn reported_usage(&self) -> Option<(u64, &Value)> {
        self.usage_snapshot
            .as_ref()
            .map(|usage| (self.usage_revision, usage))
    }

    pub(super) fn apply_to(&self, event: &mut UsageEvent) {
        event
            .tool_use
            .provider_usage
            .clone_from(&self.event.tool_use.provider_usage);
        if let (Some(source), Some(target)) = (self.event.routing.as_ref(), event.routing.as_mut())
        {
            target
                .upstream_response_model
                .clone_from(&source.upstream_response_model);
            if self.observed_started.is_some()
                && let (Some(source), Some(target)) =
                    (source.transport.as_ref(), target.transport.as_mut())
                && (source.stream_response.is_some() || target.stream_response.is_some())
            {
                let source = source.stream_response.clone().unwrap_or_default();
                let target = target.stream_response.get_or_insert_default();
                target.upstream_reasoning_first_ms = source.upstream_reasoning_first_ms;
                target.upstream_text_first_ms = source.upstream_text_first_ms;
                target.upstream_tool_first_ms = source.upstream_tool_first_ms;
                target.upstream_tool_first_complete_ms = source.upstream_tool_first_complete_ms;
            }
            if let (Some(source), Some(target)) =
                (source.cache_context.as_ref(), target.cache_context.as_mut())
            {
                target.provider.clone_from(&source.provider);
                target
                    .provider_response_id
                    .clone_from(&source.provider_response_id);
            }
        }
        if let Some((_, timing)) = &self.timing {
            // This recorder owns the current upstream generation. Client output
            // may belong to an earlier lookup, even if this generation is empty.
            event.ttft_ms = None;
            event.generation_ms = None;
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

    mod benchmark;

    #[test]
    fn native_usage_snapshot_retains_reported_cache_writes() {
        let mut event = crate::gateway::test_support::test_usage_event();
        let mut capture = UpstreamUsage::new(event.clone(), None);
        assert!(capture.observe(
            b"data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":null},\"usage\":{\"input_tokens\":4,\"cache_creation_input_tokens\":6,\"output_tokens\":2,\"total_tokens\":12}}\n\n"
        ));
        capture.apply_to(&mut event);
        assert_eq!(event.input_tokens, Some(10));
        assert_eq!(event.cache_write_input_tokens, Some(6));
        assert_eq!(event.output_tokens, Some(2));
        assert_eq!(event.total_tokens, Some(12));
    }

    #[test]
    fn timed_fast_path_ignores_empty_output_and_keeps_cache_metadata() {
        let mut event = crate::gateway::test_support::test_usage_event();
        let mut capture = UpstreamUsage::new(event.clone(), None)
            .with_timing(Instant::now() - Duration::from_millis(100));
        assert!(
            capture.observe(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"\"}\n\n")
        );
        capture.apply_to(&mut event);
        assert_eq!(event.ttft_ms, None);
        assert!(capture.observe(
            b"data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"x\"}\n\n"
        ));
        capture.apply_to(&mut event);
        let first = event.ttft_ms.expect("nonempty reasoning output");
        assert!(first >= 100);
        assert!(capture.observe(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"y\",\"prompt_cache_options\":{\"ttl\":\"12h\"}}\n\n"));
        capture.apply_to(&mut event);
        assert_eq!(event.cache_write_ttl.as_deref(), Some("12h"));
        assert_eq!(event.ttft_ms, Some(first));
    }

    #[test]
    fn json_discovery_keeps_reported_usage_and_tier_through_response_envelopes() {
        for depth in 0..=2 {
            let mut event = crate::gateway::test_support::test_usage_event();
            let mut capture = UpstreamUsage::new(event.clone(), None);
            let mut response = json!({"status":"completed","service_tier":"flex","usage":{
                "input_tokens":20,"output_tokens":3,"total_tokens":23,
                "input_tokens_details":{"cached_tokens":5,"cache_write_tokens":7,
                    "cache_write_tokens_details":{"ttl_12h_tokens":7}},
                "output_tokens_details":{"reasoning_tokens":2}
            }});
            for _ in 0..depth {
                response = json!({"response":response});
            }
            capture.finish_json_generation(&serde_json::to_vec(&response).unwrap());
            capture.apply_to(&mut event);
            assert_eq!(event.input_tokens, Some(20), "depth={depth}");
            assert_eq!(event.output_tokens, Some(3));
            assert_eq!(event.total_tokens, Some(23));
            assert_eq!(event.cached_input_tokens, Some(5));
            assert_eq!(event.cache_write_input_tokens, Some(7));
            assert_eq!(event.cache_write_ttl.as_deref(), Some("12h"));
            assert_eq!(event.reasoning_tokens, Some(2));
            assert_eq!(event.applied_service_tier.as_deref(), Some("flex"));
            assert_eq!(
                event
                    .tool_use
                    .provider_usage
                    .as_ref()
                    .unwrap()
                    .cache_write_tokens_by_ttl
                    .get("12h"),
                Some(&7)
            );
            capture.reset_generation();
            capture.finish_json_generation(br#"{"status":"completed"}"#);
            capture.apply_to(&mut event);
            assert_eq!(event.input_tokens, None);
            assert_eq!(event.applied_service_tier, None);
            assert!(event.tool_use.provider_usage.is_none());
        }
    }

    #[test]
    fn structured_usage_survives_fragmented_stream_conversion_and_resets() {
        let mut event = crate::gateway::test_support::test_usage_event();
        event.wire_api = WireApi::Messages;
        let mut capture = UpstreamUsage::new(event.clone(), None);
        let frames = [
            json!({"type":"message_start","message":{"usage":{
                "server_tool_use":{"web_search_requests":0},
                "cache_creation":{"ephemeral_5m_input_tokens":12}}}}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{
                "server_tool_use":{"web_search_requests":2}}}),
            json!({"type":"message_stop"}),
        ];
        for payload in frames {
            for chunk in format!("data: {payload}\n\n").as_bytes().chunks(3) {
                assert!(capture.observe(chunk));
            }
        }
        capture.apply_to(&mut event);
        let details = event.tool_use.provider_usage.as_ref().unwrap();
        assert_eq!(details.server_tool_calls.get("web_search"), Some(&2));
        assert_eq!(details.cache_write_tokens_by_ttl.get("5m"), Some(&12));
        assert!(event.tool_use.has_evidence());
        capture.reset_generation();
        capture
            .finish_json_generation(br#"{"usage":{"server_tool_use":{"web_fetch_requests":0}}}"#);
        capture.apply_to(&mut event);
        let details = event.tool_use.provider_usage.as_ref().unwrap();
        assert_eq!(details.server_tool_calls.get("web_fetch"), Some(&0));
        assert!(!details.server_tool_calls.contains_key("web_search"));
        assert!(details.cache_write_tokens_by_ttl.is_empty());
        capture.reset_generation();
        capture.apply_to(&mut event);
        assert!(event.tool_use.provider_usage.is_none());
    }

    #[test]
    fn cache_write_lifetimes_survive_later_metadata_without_usage() {
        for (short, long, ttl) in [(0, 7, Some("12h")), (3, 7, Some("30m, 12h")), (0, 0, None)] {
            for outcome in ["in_progress", "completed", "incomplete", "failed"] {
                let mut event = crate::gateway::test_support::test_usage_event();
                let mut capture = UpstreamUsage::new(event.clone(), None);
                let usage = json!({"type":"response.in_progress","response":{"usage":{
                    "input_tokens":20,
                    "input_tokens_details":{"cache_write_tokens":short + long,
                        "cache_write_tokens_details":{"ttl_30m_tokens":short,"ttl_12h_tokens":long}}
                }}});
                assert!(capture.observe(format!("data: {usage}\n\n").as_bytes()));
                capture.apply_to(&mut event);
                assert_eq!(event.cache_write_ttl.as_deref(), ttl);

                let metadata = json!({"type":format!("response.{outcome}"),"response":{
                    "status":outcome,"prompt_cache_options":{"ttl":"30m"}
                }});
                assert!(capture.observe(format!("data: {metadata}\n\n").as_bytes()));
                capture.apply_to(&mut event);
                assert_eq!(
                    event.cache_write_ttl.as_deref(),
                    ttl.or(Some("30m")),
                    "{outcome}"
                );
                assert_eq!(event.input_tokens, Some(20));
                assert_eq!(event.cache_write_input_tokens, Some(short + long));
                let details = event.tool_use.provider_usage.as_ref().unwrap();
                assert_eq!(details.cache_write_tokens_by_ttl.get("12h"), Some(&long));
                assert_eq!(details.cache_write_tokens_by_ttl.get("30m"), Some(&short));

                capture.reset_generation();
                assert!(capture.observe(format!("data: {metadata}\n\n").as_bytes()));
                capture.apply_to(&mut event);
                assert_eq!(event.cache_write_ttl.as_deref(), Some("30m"));
                assert_eq!(event.cache_write_input_tokens, None);
                assert!(event.tool_use.provider_usage.is_none());
            }
        }
    }

    #[tokio::test]
    async fn native_stream_cache_writes_override_echoed_defaults() {
        for usage_on_terminal in [false, true] {
            let usage = json!({"input_tokens_details":{
                "cache_write_tokens_details":{"ttl_12h_tokens":7}
            }});
            let mut partial = json!({"type":"response.in_progress","response":{
                "status":"in_progress"
            }});
            let mut terminal = json!({"type":"response.completed","response":{
                "status":"completed","output":[],"prompt_cache_options":{"ttl":"30m"}
            }});
            if usage_on_terminal {
                terminal["response"]["usage"] = usage;
            } else {
                partial["response"]["usage"] = usage;
            }
            let events = Arc::new(Mutex::new(Vec::new()));
            let captured = events.clone();
            let input = [partial, terminal].map(|payload| {
                Ok::<_, std::convert::Infallible>(Bytes::from(format!("data: {payload}\n\n")))
            });
            let mut stream = UsageStream::new(
                stream::iter(input),
                Arc::new(move |event| captured.lock().unwrap().push(event)),
                crate::gateway::test_support::test_usage_event(),
                Instant::now(),
                Arc::new(|_, _, _| {}),
            );
            while let Some(chunk) = stream.next().await {
                assert!(chunk.is_ok());
            }
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 1);
            assert!(events[0].success);
            assert_eq!(events[0].cache_write_ttl.as_deref(), Some("12h"));
            assert_eq!(events[0].cache_write_input_tokens, None);
        }
    }

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
        event.observe_response_metadata(&json!({"model":"synthetic-client-alias"}));
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
            assert!(
                capture
                    .forward(Bytes::copy_from_slice(&frame[..split]))
                    .is_empty()
            );
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
    fn native_tool_boundaries_survive_fast_deltas_and_reset_without_client_readiness() {
        let mut initial = crate::gateway::test_support::test_usage_event();
        initial.routing = Some(
            serde_json::from_value(json!({
                "reason":"only_eligible", "eligibleCandidates":1,
                "inFlightBefore":0, "dispatchesBefore":0,
                "transport":{"upstream":"http", "mode":"basis_points"}
            }))
            .unwrap(),
        );
        let mut capture = UpstreamUsage::new(initial.clone(), None)
            .with_observations(Instant::now() - Duration::from_millis(100));
        capture.observe(
            b"data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\"}\n\n",
        );
        capture.observe(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\n",
        );
        capture.observe(b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_test\",\"call_id\":\"call_test\",\"arguments\":\"{}\",\"status\":\"completed\"}}\n\n");
        let mut event = initial;
        capture.apply_to(&mut event);
        let transport = event.routing.as_ref().unwrap().transport.as_ref().unwrap();
        let timing = transport.stream_response.as_ref().unwrap();
        assert!(timing.upstream_tool_first_ms.is_some_and(|ms| ms >= 100));
        assert!(timing.upstream_text_first_ms.is_some_and(|ms| ms >= 100));
        assert!(
            timing
                .upstream_tool_first_complete_ms
                .is_some_and(|complete| complete >= timing.upstream_tool_first_ms.unwrap())
        );
        assert_eq!(event.tool_use.tool_calls_ready_ms, None);
        assert_eq!(transport.client_first_output_ms, None);
        capture.reset_generation();
        capture.apply_to(&mut event);
        let transport = event.routing.as_ref().unwrap().transport.as_ref().unwrap();
        let timing = transport.stream_response.as_ref().unwrap();
        assert_eq!(timing.upstream_text_first_ms, None);
        assert_eq!(timing.upstream_tool_first_ms, None);
        assert_eq!(timing.upstream_tool_first_complete_ms, None);
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
    fn discovery_generations_do_not_inherit_first_output_or_completion_timing() {
        let started = Instant::now() - Duration::from_millis(100);
        let mut capture =
            UpstreamUsage::new(crate::gateway::test_support::test_usage_event(), None)
                .with_timing(started);
        capture.observe(b"data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n");
        let mut first = crate::gateway::test_support::test_usage_event();
        capture.apply_to(&mut first);
        assert!(first.ttft_ms.is_some_and(|first| first >= 100));

        capture.reset_generation();
        capture.finish_json_generation(br#"{"status":"completed","output":[]}"#);
        let mut second = crate::gateway::test_support::test_usage_event();
        capture.apply_to(&mut second);
        assert_eq!(
            second.ttft_ms, None,
            "a later snapshot has no first-token evidence"
        );
        assert_eq!(second.generation_ms, None);

        capture.reset_generation();
        capture.observe(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\n",
        );
        let mut partial = crate::gateway::test_support::test_usage_event();
        partial.latency_ms = 500;
        capture.apply_to(&mut partial);
        assert_eq!(
            partial.latency_ms, 500,
            "a prior terminal must not finish this attempt"
        );
        let first = partial.ttft_ms.expect("current generation's output");
        assert!(first >= 100);
        assert_eq!(partial.generation_ms, Some(500_u64.saturating_sub(first)));
    }

    #[test]
    fn unobserved_followup_does_not_inherit_discovery_output_timing() {
        let started = Instant::now() - Duration::from_millis(100);
        let mut event = crate::gateway::test_support::test_usage_event();
        let mut capture = UpstreamUsage::new(event.clone(), None).with_timing(started);
        assert!(capture.observe(b"data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n"));
        capture.apply_to(&mut event);
        assert!(event.ttft_ms.is_some());

        capture.reset_generation();
        assert!(capture.observe(
            b"data: {\"type\":\"response.created\",\"response\":{\"status\":\"in_progress\"}}\n\n"
        ));
        event.latency_ms = 500;
        capture.apply_to(&mut event);
        assert_eq!(event.ttft_ms, None);
        assert_eq!(event.generation_ms, None);
        assert_eq!(event.latency_ms, 500);
    }

    #[test]
    fn capture_without_raw_timing_preserves_normalized_output_timing() {
        let mut event = crate::gateway::test_support::test_usage_event();
        event.ttft_ms = Some(100);
        event.generation_ms = Some(400);
        event.latency_ms = 500;
        let mut capture = UpstreamUsage::new(event.clone(), None);
        capture.reset_generation();
        capture.apply_to(&mut event);
        assert_eq!(event.ttft_ms, Some(100));
        assert_eq!(event.generation_ms, Some(400));
        assert_eq!(event.latency_ms, 500);
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
        assert!(
            capture.observe(
                b"data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-"
            )
        );
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
