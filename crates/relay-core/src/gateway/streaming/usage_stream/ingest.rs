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
            error_codes::STREAM_INCOMPLETE => "Upstream stream ended before response.completed",
            _ => "Upstream stream disconnected before completion",
        };
        let origin = Self::stream_error_origin(event).for_category(category);
        let message = origin.prefix_message(message);
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
                        "origin": origin.as_str(),
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

    pub(super) fn stream_error_origin(event: &UsageEvent) -> crate::ErrorOrigin {
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
        self.sse_pending.extend_from_slice(bytes);
        true
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

    pub(super) fn forward_sse_bytes(
        &mut self,
        bytes: &[u8],
        origin: crate::ErrorOrigin,
    ) -> Vec<u8> {
        self.sse_forward_pending.extend_from_slice(bytes);
        let mut forwarded = Vec::with_capacity(bytes.len());
        loop {
            match self.sse_forward_mode {
                super::SseForwardMode::Normal => {
                    let Some(end) = crate::protocol::sse_event_end(&self.sse_forward_pending)
                    else {
                        forwarded.extend(self.sse_forward_pending.drain(..));
                        break;
                    };
                    let event = self.sse_forward_pending.drain(..end).collect::<Vec<_>>();
                    let terminal = parse_sse_event(&event);
                    let is_terminal = terminal.outcome.is_some();
                    forwarded.extend(prefix_stream_error_event(&event, &terminal, origin));
                    self.sse_forward_mode = super::SseForwardMode::Detect;
                    if is_terminal {
                        self.sse_forward_pending.clear();
                        break;
                    }
                }
                super::SseForwardMode::Error => {
                    let Some(end) = crate::protocol::sse_event_end(&self.sse_forward_pending)
                    else {
                        break;
                    };
                    let event = self.sse_forward_pending.drain(..end).collect::<Vec<_>>();
                    let terminal = parse_sse_event(&event);
                    let is_terminal = terminal.outcome.is_some();
                    forwarded.extend(prefix_stream_error_event(&event, &terminal, origin));
                    self.sse_forward_mode = super::SseForwardMode::Detect;
                    if is_terminal {
                        self.sse_forward_pending.clear();
                        break;
                    }
                }
                super::SseForwardMode::Detect => {
                    match classify_sse_prefix(&self.sse_forward_pending) {
                        Some(true) => {
                            self.sse_forward_mode = super::SseForwardMode::Error;
                        }
                        Some(false) => {
                            self.sse_forward_mode = super::SseForwardMode::Normal;
                        }
                        None => {
                            let end = crate::protocol::sse_event_end(&self.sse_forward_pending);
                            let Some(end) = end else {
                                if may_be_terminal_event_header_prefix(&self.sse_forward_pending) {
                                    break;
                                }
                                // Preserve low-latency forwarding when an
                                // ordinary data-only frame has not yet exposed
                                // a terminal marker. Standard terminal SSE
                                // events are held as soon as their `event:`
                                // name identifies them.
                                self.sse_forward_mode = super::SseForwardMode::Normal;
                                forwarded.extend(self.sse_forward_pending.drain(..));
                                break;
                            };
                            let event = self.sse_forward_pending.drain(..end).collect::<Vec<_>>();
                            let terminal = parse_sse_event(&event);
                            let is_terminal = terminal.outcome.is_some();
                            forwarded.extend(prefix_stream_error_event(&event, &terminal, origin));
                            if is_terminal {
                                self.sse_forward_pending.clear();
                                break;
                            }
                        }
                    }
                }
            }
        }
        forwarded
    }

    pub(in crate::gateway::streaming) fn ingest_sse(&mut self, bytes: &[u8]) -> bool {
        if self.terminated {
            return false;
        }
        self.sse_pending.extend_from_slice(bytes);
        while let Some(event) = crate::protocol::take_sse_event(&mut self.sse_pending) {
            if let Some(delta) = fast_response_delta(&event) {
                self.apply_fast_response_delta(delta);
                continue;
            }
            let terminal = parse_sse_event(&event);
            // Reported consumption remains real even when identity validation
            // rejects the response before it can reach the client.
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
            if self.expected_model.as_deref().is_some_and(|expected| {
                terminal
                    .payload
                    .as_ref()
                    .is_some_and(|value| served_model_is_rejected(value, expected))
            }) {
                self.sse_pending.clear();
                self.fail_stream(error_codes::UPSTREAM_ROUTE_DEGRADED);
                return false;
            }
            if terminal.has_data && !terminal.valid {
                self.set_upstream_error(terminal.upstream_error);
                self.sse_pending.clear();
                self.fail_stream(error_codes::STREAM_INVALID);
                return false;
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
                return false;
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
                self.sse_pending.clear();
                return true;
            }
        }
        true
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

fn classify_sse_prefix(bytes: &[u8]) -> Option<bool> {
    let mut offset = 0;
    let mut data = Vec::new();
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        let Some(end) = remaining
            .iter()
            .position(|byte| matches!(byte, b'\r' | b'\n'))
        else {
            let line = remaining;
            if line.starts_with(b":") {
                return Some(false);
            }
            if let Some(value) = line.strip_prefix(b"event:") {
                if is_terminal_sse_event(value.trim_ascii()) {
                    return Some(true);
                }
            }
            if let Some(value) = line.strip_prefix(b"data:") {
                if !data.is_empty() {
                    data.push(b'\n');
                }
                data.extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
                return classify_json_error_prefix(&data);
            }
            return None;
        };
        let line = &remaining[..end];
        let line_ending = if remaining[end] == b'\r' && remaining.get(end + 1) == Some(&b'\n') {
            2
        } else {
            1
        };
        offset += end + line_ending;

        if line.starts_with(b":") {
            return Some(false);
        }
        if line.is_empty() {
            return Some(false);
        }
        if let Some(value) = line.strip_prefix(b"event:") {
            if is_terminal_sse_event(value.trim_ascii()) {
                return Some(true);
            }
        } else if let Some(value) = line.strip_prefix(b"data:") {
            if !data.is_empty() {
                data.push(b'\n');
            }
            data.extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
            if let Some(is_error) = classify_json_error_prefix(&data) {
                return Some(is_error);
            }
        }
    }
    None
}

fn is_terminal_sse_event(event: &[u8]) -> bool {
    matches!(
        event,
        b"error"
            | b"response.failed"
            | b"response.incomplete"
            | b"response.cancelled"
            | b"response.canceled"
            | b"response.completed"
            | b"response.done"
    )
}

fn may_be_terminal_event_header_prefix(bytes: &[u8]) -> bool {
    let Some(line_end) = bytes.iter().position(|byte| matches!(byte, b'\r' | b'\n')) else {
        if b"event:".starts_with(bytes) {
            return true;
        }
        let Some(value) = bytes.strip_prefix(b"event:") else {
            return false;
        };
        let value = value.trim_ascii();
        return [
            b"error".as_slice(),
            b"response.failed".as_slice(),
            b"response.incomplete".as_slice(),
            b"response.cancelled".as_slice(),
            b"response.canceled".as_slice(),
            b"response.completed".as_slice(),
            b"response.done".as_slice(),
        ]
        .into_iter()
        .any(|event| event.starts_with(value));
    };
    bytes[..line_end]
        .strip_prefix(b"event:")
        .is_some_and(|value| is_terminal_sse_event(value.trim_ascii()))
}

fn classify_json_error_prefix(bytes: &[u8]) -> Option<bool> {
    let mut offset = skip_ascii_whitespace(bytes, 0);
    if offset == bytes.len() {
        return None;
    }
    if bytes[offset] != b'{' {
        return Some(false);
    }
    offset = skip_ascii_whitespace(bytes, offset + 1);
    if offset == bytes.len() {
        return None;
    }
    if bytes[offset] != b'"' {
        return Some(false);
    }
    let Some((key, consumed)) = read_json_string_prefix(&bytes[offset..]) else {
        return None;
    };
    let Some(key) = key else {
        return None;
    };
    offset = skip_ascii_whitespace(bytes, offset + consumed);
    if offset == bytes.len() {
        return None;
    }
    if bytes[offset] != b':' {
        return Some(false);
    }
    if key == b"error" || key == b"error_description" {
        return Some(true);
    }
    if key != b"type" {
        // Keep ambiguous objects buffered until the complete SSE event is
        // available. A later `type` field may identify a terminal error.
        return None;
    }
    offset = skip_ascii_whitespace(bytes, offset + 1);
    if offset == bytes.len() {
        return None;
    }
    if bytes[offset] != b'"' {
        return Some(false);
    }
    let Some((value, _)) = read_json_string_prefix(&bytes[offset..]) else {
        return None;
    };
    let Some(value) = value else {
        return Some(false);
    };
    Some(matches!(
        value,
        b"error"
            | b"response.failed"
            | b"response.incomplete"
            | b"response.cancelled"
            | b"response.canceled"
            | b"response.completed"
            | b"response.done"
    ))
}

fn read_json_string_prefix(bytes: &[u8]) -> Option<(Option<&[u8]>, usize)> {
    if bytes.first() != Some(&b'"') {
        return Some((None, 0));
    }
    let mut offset = 1;
    while offset < bytes.len() {
        match bytes[offset] {
            b'"' => return Some((Some(&bytes[1..offset]), offset + 1)),
            b'\\' => {
                // These protocol field names and event types are plain ASCII.
                // If a provider escapes them, wait for the complete frame and
                // let the full JSON parser decide its outcome.
                if offset + 1 >= bytes.len() {
                    return None;
                }
                return Some((None, 0));
            }
            byte if byte < 0x20 => return Some((None, 0)),
            _ => offset += 1,
        }
    }
    None
}

fn skip_ascii_whitespace(bytes: &[u8], mut offset: usize) -> usize {
    while bytes.get(offset).is_some_and(u8::is_ascii_whitespace) {
        offset += 1;
    }
    offset
}

fn prefix_stream_error_event(
    event: &[u8],
    terminal: &super::TerminalEvent,
    origin: crate::ErrorOrigin,
) -> Vec<u8> {
    if !matches!(
        terminal.outcome,
        Some(TerminalOutcome::Failure | TerminalOutcome::Incomplete)
    ) {
        return event.to_vec();
    }
    let Some(mut payload) = terminal.payload.clone() else {
        return event.to_vec();
    };
    let category = terminal
        .error_category
        .unwrap_or(error_codes::RESPONSE_INCOMPLETE);
    let origin = origin.for_category(category);
    if !super::super::super::errors::prefix_error_value(&mut payload, origin) {
        return event.to_vec();
    }
    let Ok(payload) = serde_json::to_vec(&payload) else {
        return event.to_vec();
    };
    let mut rewritten = Vec::with_capacity(event.len().saturating_add(origin.label().len() + 2));
    let mut wrote_data = false;
    for line in crate::protocol::sse_lines(&event) {
        if line.strip_prefix(b"data:").is_some() {
            if !wrote_data {
                rewritten.extend_from_slice(b"data: ");
                rewritten.extend_from_slice(&payload);
                rewritten.push(b'\n');
                wrote_data = true;
            }
        } else {
            rewritten.extend_from_slice(line);
            rewritten.push(b'\n');
        }
    }
    if wrote_data {
        rewritten
    } else {
        event.to_vec()
    }
}
