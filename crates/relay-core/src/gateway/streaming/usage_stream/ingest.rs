use super::super::fast_delta::{fast_response_delta, FastResponseDelta};
use super::*;

impl<S> UsageStream<S> {
    pub(super) fn finish(&mut self, success: Option<bool>, category: Option<&str>) {
        let Some(mut event) = self.event.take() else {
            return;
        };
        if let Some(success) = success {
            event.success = success;
        }
        if let Some(category) = category {
            event.error_category = Some(category.to_string());
        }
        if event.success {
            event.tool_use.finish();
        }
        if !event.success
            && event.http_status < 400
            && event.error_category.as_deref() != Some(error_codes::RESPONSE_INCOMPLETE)
        {
            event.http_status = event
                .error_category
                .as_deref()
                .filter(|category| *category != error_codes::CLIENT_CANCELLED)
                .map(upstream_failure_status)
                .unwrap_or(StatusCode::BAD_GATEWAY)
                .as_u16();
        }
        event.latency_ms = self.started.elapsed().as_millis() as u64;
        event.generation_ms = event
            .ttft_ms
            .map(|ttft_ms| event.latency_ms.saturating_sub(ttft_ms))
            .filter(|duration| *duration > 0);
        (self.completion)(&mut event, self.response_id.as_deref(), self.cooldown_hint);
        if let Some(runtime) = self.runtime.as_deref() {
            emit_usage(runtime, event);
        } else {
            emit_callback(&self.callback, event);
        }
    }

    fn queue_responses_failure(&mut self, category: &str) -> bool {
        let Some(event) = self.event.as_ref() else {
            return false;
        };
        if event.wire_api != WireApi::Responses || self.client_visible_output {
            return false;
        }
        let response_id = self.response_id.clone().unwrap_or_else(|| {
            let suffix = event
                .request_id
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>();
            format!("resp_{suffix}")
        });
        let message = match category {
            error_codes::STREAM_INVALID => "Upstream returned an invalid streaming event",
            error_codes::STREAM_EVENT_TOO_LARGE => {
                "Upstream streaming event exceeded the size limit"
            }
            error_codes::STREAM_INCOMPLETE => "Upstream stream ended before response.completed",
            _ => "Upstream stream disconnected before completion",
        };
        let payload = json!({
            "type": "response.failed",
            "response": {
                "id": response_id,
                "object": "response",
                "model": event.requested_model.clone().unwrap_or_default(),
                "status": "failed",
                "output": [],
                "error": {
                    "type": error_codes::STREAM_ERROR,
                    "code": category,
                    "message": message,
                    "zenith_relay": {
                        "origin": Self::stream_error_origin(event).as_str(),
                        "category": category,
                        "request_id": &event.request_id,
                    },
                }
            }
        });
        let Ok(payload) = serde_json::to_vec(&payload) else {
            return false;
        };
        let mut frame = Vec::with_capacity(payload.len() + 44);
        frame.extend_from_slice(b"event: response.failed\ndata: ");
        frame.extend_from_slice(&payload);
        frame.extend_from_slice(b"\n\n");
        self.output_pending.push_back(Bytes::from(frame));
        true
    }

    fn stream_error_origin(event: &UsageEvent) -> crate::ErrorOrigin {
        if event.account_id.is_some() {
            crate::ErrorOrigin::Account
        } else {
            crate::ErrorOrigin::Provider
        }
    }

    pub(super) fn fail_stream(&mut self, category: &str) -> bool {
        let framed = self.queue_responses_failure(category);
        self.finish(Some(false), Some(category));
        self.terminated = true;
        framed
    }

    fn accept_sse_bytes(&mut self, bytes: &[u8]) -> bool {
        if self.terminated {
            return false;
        }
        if self.sse_pending.len().saturating_add(bytes.len()) > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.fail_stream(error_codes::STREAM_EVENT_TOO_LARGE);
            return false;
        }
        self.sse_pending.extend_from_slice(bytes);
        true
    }

    fn consume_aligned_response_deltas<'a>(&mut self, bytes: &'a [u8]) -> Option<&'a [u8]> {
        let mut offset = 0;
        while offset < bytes.len() {
            let Some(end) = crate::protocol::sse_event_end(&bytes[offset..]) else {
                return Some(&bytes[offset..]);
            };
            let frame = &bytes[offset..offset + end];
            let Some(delta) = fast_response_delta(frame) else {
                return Some(&bytes[offset..]);
            };
            self.apply_fast_response_delta(delta);
            offset += end;
        }
        None
    }

    fn apply_fast_response_delta(&mut self, delta: FastResponseDelta) {
        if self.native_response.is_some() {
            self.native_replay_capture
                .observe_response_delta(delta.output_index);
        }
        if delta.nonempty_text
            && self
                .event
                .as_ref()
                .is_some_and(|event| event.ttft_ms.is_none())
        {
            if let Some(current) = self.event.as_mut() {
                current.ttft_ms = Some(self.started.elapsed().as_millis() as u64);
            }
        }
    }

    pub(in crate::gateway::streaming) fn ingest_sse(&mut self, bytes: &[u8]) -> (bool, usize) {
        if self.terminated {
            return (false, 0);
        }
        if self.sse_pending.len().saturating_add(bytes.len()) > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.fail_stream(error_codes::STREAM_EVENT_TOO_LARGE);
            return (false, 0);
        }
        let pending = if self.sse_pending.is_empty() {
            match self.consume_aligned_response_deltas(bytes) {
                None => return (true, bytes.len()),
                Some(rest) => rest,
            }
        } else {
            bytes
        };
        self.sse_pending.extend_from_slice(pending);
        while let Some(event) = crate::protocol::take_sse_event(&mut self.sse_pending) {
            if let Some(delta) = fast_response_delta(&event) {
                self.apply_fast_response_delta(delta);
                continue;
            }
            if event.len() > MAX_SSE_EVENT_BYTES {
                self.sse_pending.clear();
                self.fail_stream(error_codes::STREAM_EVENT_TOO_LARGE);
                return (false, 0);
            }
            let terminal = parse_sse_event(&event);
            if self.expected_model.as_deref().is_some_and(|expected| {
                terminal
                    .payload
                    .as_ref()
                    .is_some_and(|value| served_model_is_rejected(value, expected))
            }) {
                self.sse_pending.clear();
                self.fail_stream(error_codes::UPSTREAM_ROUTE_DEGRADED);
                return (false, 0);
            }
            if terminal.has_data && !terminal.valid {
                self.set_upstream_error(terminal.upstream_error);
                self.sse_pending.clear();
                self.fail_stream(error_codes::STREAM_INVALID);
                return (false, 0);
            }
            // A terminal marker from another wire protocol cannot prove this
            // request or its continuation state completed successfully.
            let valid_terminal = self
                .event
                .as_ref()
                .is_some_and(|event| match event.wire_api {
                    WireApi::Responses => terminal.payload.as_ref().is_some_and(|payload| {
                        matches!(
                            payload.get("type").and_then(Value::as_str),
                            Some("response.completed" | "response.done")
                        )
                    }),
                    WireApi::ChatCompletions => terminal.payload.is_none(),
                    WireApi::Messages => terminal.payload.as_ref().is_some_and(|payload| {
                        payload.get("type").and_then(Value::as_str) == Some("message_stop")
                    }),
                    WireApi::Gemini => false,
                });
            if terminal.outcome == Some(TerminalOutcome::Success) && !valid_terminal {
                self.sse_pending.clear();
                self.fail_stream(error_codes::STREAM_INCOMPLETE);
                return (false, 0);
            }
            if let Some(payload) = terminal.payload.as_ref() {
                if let Some(current) = self.event.as_mut() {
                    current.tool_use.observe_stream_payload(payload);
                }
                if self.native_response.is_some() {
                    self.native_replay_capture.observe(payload);
                }
            } else if terminal.is_compaction {
                self.native_replay_capture.mark_unmaterialized();
            }
            if terminal.has_output_delta
                && self
                    .event
                    .as_ref()
                    .is_some_and(|event| event.ttft_ms.is_none())
            {
                if let Some(current) = self.event.as_mut() {
                    current.ttft_ms = Some(self.started.elapsed().as_millis() as u64);
                }
            }
            if let Some(usage) = terminal.usage {
                if let Some(current) = self.event.as_mut() {
                    apply_usage(current, &usage);
                }
            }
            if let Some(service_tier) = terminal.applied_service_tier {
                if let Some(current) = self.event.as_mut() {
                    current.applied_service_tier = Some(service_tier);
                }
            }
            if terminal.response_id.is_some() {
                self.response_id = terminal.response_id;
            }
            match terminal.outcome {
                Some(TerminalOutcome::Success) => {
                    self.capture_native_response(terminal.response);
                    self.finish(None, None);
                    self.terminated = true;
                }
                Some(TerminalOutcome::Incomplete) => {
                    self.capture_native_response(terminal.response);
                    self.finish(
                        Some(false),
                        Some(
                            terminal
                                .error_category
                                .unwrap_or(error_codes::RESPONSE_INCOMPLETE),
                        ),
                    );
                    self.terminated = true;
                }
                Some(TerminalOutcome::Failure) => {
                    self.cooldown_hint = terminal.cooldown_hint;
                    self.set_upstream_error(terminal.upstream_error);
                    self.finish(
                        Some(false),
                        Some(
                            terminal
                                .error_category
                                .unwrap_or(error_codes::UPSTREAM_TERMINAL),
                        ),
                    );
                    self.terminated = true;
                }
                None => {}
            }
            if self.terminated {
                let forward_len = bytes.len().saturating_sub(self.sse_pending.len());
                self.sse_pending.clear();
                return (true, forward_len);
            }
        }
        if self.sse_pending.len() > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.fail_stream(error_codes::STREAM_EVENT_TOO_LARGE);
            return (false, 0);
        }
        (true, bytes.len())
    }

    /// Gemini's native SSE ends at EOF rather than with `response.completed`.
    /// Require a recognized final candidate or prompt block before treating
    /// that EOF as a completed request; keep the provider bytes untouched.
    pub(in crate::gateway::streaming) fn ingest_native_gemini(&mut self, bytes: &[u8]) -> bool {
        if !self.accept_sse_bytes(bytes) {
            return false;
        }
        while let Some(event) = crate::protocol::take_sse_event(&mut self.sse_pending) {
            let terminal = parse_sse_event(&event);
            if terminal.has_data && !terminal.valid {
                self.set_upstream_error(terminal.upstream_error);
                self.fail_stream(error_codes::STREAM_INVALID);
                self.terminated = true;
                return false;
            }
            if terminal.outcome == Some(TerminalOutcome::Failure) {
                self.cooldown_hint = terminal.cooldown_hint;
                self.set_upstream_error(terminal.upstream_error);
                self.finish(
                    Some(false),
                    Some(
                        terminal
                            .error_category
                            .unwrap_or(error_codes::UPSTREAM_TERMINAL),
                    ),
                );
                self.terminated = true;
                return true;
            }
            if terminal.outcome == Some(TerminalOutcome::Incomplete) {
                self.native_gemini_incomplete = true;
            }
            // Gemini has no generic [DONE] or Responses-style terminal event.
            // Only a candidate's own finish reason proves a completed generation.
            if terminal.payload.as_ref().is_some_and(|payload| {
                payload
                    .pointer("/candidates/0/finishReason")
                    .and_then(Value::as_str)
                    == Some("STOP")
            }) {
                self.native_gemini_finished = true;
            }
            if let Some(usage) = terminal.usage {
                if let Some(current) = self.event.as_mut() {
                    apply_usage(current, &usage);
                }
            }
            if terminal.has_output_delta
                && self
                    .event
                    .as_ref()
                    .is_some_and(|event| event.ttft_ms.is_none())
            {
                if let Some(current) = self.event.as_mut() {
                    current.ttft_ms = Some(self.started.elapsed().as_millis() as u64);
                }
            }
        }
        true
    }

    fn set_upstream_error(&mut self, details: Option<crate::usage::UpstreamErrorDetails>) {
        if let Some(current) = self.event.as_mut() {
            current.upstream_error = details.map(|mut details| {
                details.http_status = Some(current.http_status);
                details
            });
        }
    }

    fn capture_native_response(&mut self, response: Option<Value>) {
        let Some(shared) = self.native_response.as_ref() else {
            return;
        };
        let Some(response) = std::mem::take(&mut self.native_replay_capture)
            .finish(response, self.response_id.as_deref())
        else {
            return;
        };
        *crate::poison::mutex(shared) = Some(response);
    }
}
