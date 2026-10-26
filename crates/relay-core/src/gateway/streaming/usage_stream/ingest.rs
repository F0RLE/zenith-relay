use super::super::fast_delta::{FastResponseDelta, fast_response_delta};
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
            && let Some(routing) = event.routing.as_mut()
        {
            routing.attempt_outcome = Some(
                if event.error_category.as_deref() == Some(error_codes::CLIENT_CANCELLED) {
                    crate::AttemptOutcome::ClientDisconnected
                } else {
                    crate::AttemptOutcome::ReturnedFailure
                },
            );
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
        if event.success
            && let Some(transport) = event
                .routing
                .as_mut()
                .and_then(|routing| routing.transport.as_mut())
        {
            transport.client_response_ready_ms = Some(self.started.elapsed().as_millis() as u64);
        }
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
        if event.wire_api != WireApi::Responses || self.client_frame_open {
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
                "Upstream streaming event exceeded Relay's size limit"
            }
            error_codes::STREAM_INCOMPLETE => "Upstream stream ended before response.completed",
            _ => "Upstream stream disconnected before completion",
        };
        let origin = Self::stream_error_origin(event).for_category(category);
        let message = origin.prefix_message(message);
        let failure_event = json!({
            "type": "response.failed",
            "sequence_number": self.next_sequence,
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
        let Ok(failure_event_bytes) = serde_json::to_vec(&failure_event) else {
            return false;
        };
        let mut frame = Vec::with_capacity(failure_event_bytes.len() + 44);
        frame.extend_from_slice(b"event: response.failed\ndata: ");
        frame.extend_from_slice(&failure_event_bytes);
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

    fn queue_interactions_failure(&mut self, category: &str) -> bool {
        let Some(event) = self
            .event
            .as_ref()
            .filter(|event| event.wire_api == WireApi::GeminiInteractions)
        else {
            return false;
        };
        if self.client_frame_open {
            return false;
        }
        let origin = Self::stream_error_origin(event).for_category(category);
        let payload = json!({"event_type":"error","error":{
            "code":category,
            "message":origin.prefix_message("Upstream interaction stream ended without a valid terminal event"),
            "zenith_relay":{"origin":origin.as_str(),"category":category,"request_id":event.request_id}
        }});
        self.output_pending
            .push_back(Bytes::from(format!("event: error\ndata: {payload}\n\n")));
        true
    }

    pub(super) fn fail_stream(&mut self, category: &str) -> bool {
        let framed =
            self.queue_responses_failure(category) || self.queue_interactions_failure(category);
        self.finish(Some(false), Some(category));
        self.terminated = true;
        framed
    }

    fn ingest_frames(&mut self, bytes: &[u8], handle: fn(&mut Self, &[u8]) -> bool) -> bool {
        if self.terminated {
            return false;
        }
        let mut valid = true;
        let pending = crate::protocol::push_pending_sse_frames(
            std::mem::take(&mut self.sse_pending),
            bytes,
            self,
            |stream| stream.terminated,
            |stream, frame| valid = handle(stream, frame),
        );
        match pending {
            Ok(pending) => self.sse_pending = pending,
            Err(()) => {
                self.fail_stream(error_codes::STREAM_EVENT_TOO_LARGE);
                return false;
            }
        }
        valid
    }

    fn apply_fast_response_delta(&mut self, delta: FastResponseDelta) {
        self.observe_sequence(delta.sequence_number);
        if self.native_response.is_some() {
            self.native_replay_capture
                .observe_response_delta(delta.output_index);
        }
        self.observe_first_output(delta.nonempty_text);
    }

    fn observe_sequence(&mut self, sequence: Option<u64>) {
        if let Some(sequence) = sequence {
            self.next_sequence = self.next_sequence.max(sequence.saturating_add(1));
        }
    }

    fn observe_first_output(&mut self, has_output: bool) {
        if !has_output {
            return;
        }
        if let Some(event) = self.event.as_mut().filter(|event| event.ttft_ms.is_none()) {
            let elapsed_ms = self.started.elapsed().as_millis() as u64;
            event.ttft_ms = Some(elapsed_ms);
            if let Some(transport) = event
                .routing
                .as_mut()
                .and_then(|routing| routing.transport.as_mut())
            {
                transport.client_first_output_ms.get_or_insert(elapsed_ms);
            }
        }
    }

    pub(super) fn forward_sse_bytes(
        &mut self,
        bytes: &[u8],
        origin: crate::ErrorOrigin,
    ) -> Vec<u8> {
        if self.native_gemini {
            return bytes.to_vec();
        }
        self.sse_forward_pending.extend_from_slice(bytes);
        let mut forwarded = Vec::with_capacity(bytes.len());
        loop {
            match self.sse_forward_mode {
                super::SseForwardMode::Passthrough | super::SseForwardMode::BufferedTerminal => {
                    let Some(end) = crate::protocol::sse_event_end(&self.sse_forward_pending)
                    else {
                        if self.sse_forward_mode == super::SseForwardMode::Passthrough {
                            forwarded.append(&mut self.sse_forward_pending);
                        }
                        break;
                    };
                    if self.forward_complete_sse_event(end, origin, &mut forwarded) {
                        break;
                    }
                }
                super::SseForwardMode::Detect => {
                    match classify_sse_prefix(&self.sse_forward_pending) {
                        Some(true) => {
                            self.sse_forward_mode = super::SseForwardMode::BufferedTerminal;
                        }
                        Some(false) => {
                            self.sse_forward_mode = super::SseForwardMode::Passthrough;
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
                                self.sse_forward_mode = super::SseForwardMode::Passthrough;
                                forwarded.append(&mut self.sse_forward_pending);
                                break;
                            };
                            if self.forward_complete_sse_event(end, origin, &mut forwarded) {
                                break;
                            }
                        }
                    }
                }
            }
        }
        forwarded
    }

    /// Returns whether this frame ended the upstream response. Terminal frames
    /// discard trailing bytes; ordinary frames resume prefix detection.
    fn forward_complete_sse_event(
        &mut self,
        end: usize,
        origin: crate::ErrorOrigin,
        forwarded: &mut Vec<u8>,
    ) -> bool {
        let frame = &self.sse_forward_pending[..end];
        let is_terminal = if fast_response_delta(frame).is_some() {
            forwarded.extend_from_slice(frame);
            false
        } else {
            let event = parse_sse_event(frame);
            forwarded.extend(prefix_stream_error_event(frame, &event, origin));
            event.outcome.is_some()
        };
        self.sse_forward_mode = super::SseForwardMode::Detect;
        if is_terminal {
            self.sse_forward_pending.clear();
        } else {
            self.sse_forward_pending.drain(..end);
        }
        is_terminal
    }

    pub(in crate::gateway::streaming) fn ingest_sse(&mut self, bytes: &[u8]) -> bool {
        self.ingest_frames(bytes, Self::ingest_sse_frame)
    }

    fn ingest_sse_frame(&mut self, event: &[u8]) -> bool {
        if let Some(delta) = fast_response_delta(event) {
            self.apply_fast_response_delta(delta);
            return true;
        }
        let mut terminal = parse_sse_event(event);
        self.observe_sequence(
            terminal
                .event_payload
                .as_ref()
                .and_then(|payload| payload.get("sequence_number"))
                .and_then(Value::as_u64),
        );
        // Reported consumption remains real even when identity validation
        // rejects the response before it can reach the client.
        if let Some(usage) = terminal.usage {
            self.merge_usage_snapshot(&usage);
        }
        if let Some(service_tier) = terminal.applied_service_tier
            && let Some(usage_event) = self.event.as_mut()
        {
            usage_event.applied_service_tier = Some(service_tier);
        }
        if self.expected_model.as_deref().is_some_and(|expected| {
            terminal
                .event_payload
                .as_ref()
                .is_some_and(|served_model| served_model_is_rejected(served_model, expected))
        }) {
            self.fail_stream(error_codes::UPSTREAM_ROUTE_DEGRADED);
            return false;
        }
        if terminal.has_data && !terminal.valid {
            self.set_upstream_error(terminal.upstream_error);
            self.fail_stream(error_codes::STREAM_INVALID);
            return false;
        }
        if self
            .event
            .as_ref()
            .is_some_and(|event| event.wire_api == WireApi::GeminiInteractions)
            && terminal.outcome == Some(TerminalOutcome::Failure)
            && terminal.error_category == Some(error_codes::STREAM_INVALID)
            && terminal.event_payload.as_ref().is_some_and(|payload| {
                payload.get("event_type").and_then(Value::as_str) != Some("error")
            })
        {
            self.fail_stream(error_codes::STREAM_INVALID);
            return false;
        }
        if !matches!(terminal.outcome, Some(TerminalOutcome::Failure)) {
            let completion = match self.event.as_ref().map(|event| event.wire_api) {
                Some(WireApi::GeminiInteractions) => match terminal.event_payload.as_ref() {
                    Some(payload) => Ok(self.interactions_completion.observe(payload)),
                    None if terminal.has_data => Err(()),
                    None => Ok(None),
                },
                Some(WireApi::ChatCompletions) => match terminal.event_payload.as_ref() {
                    Some(payload) => self.chat_completion.observe(payload).map(|()| None),
                    None if terminal.has_data => Ok(Some(self.chat_completion.outcome())),
                    None => Ok(None),
                },
                Some(WireApi::Messages) => match terminal.event_payload.as_ref() {
                    Some(payload) => self.messages_completion.observe(payload),
                    None if terminal.has_data => Err(()),
                    None => Ok(None),
                },
                Some(WireApi::Responses)
                    if terminal.has_data
                        && terminal.event_payload.is_none()
                        && !terminal.is_compaction =>
                {
                    // The transport ended without a Responses document.
                    // Keep this separate from a provider's explicit
                    // response.incomplete, which can contain useful output.
                    self.fail_stream(error_codes::STREAM_INCOMPLETE);
                    return false;
                }
                _ => Ok(None),
            };
            match completion {
                Ok(Some(ResponseCompletion::Completed)) => {
                    terminal.outcome = Some(TerminalOutcome::Success)
                }
                Ok(Some(ResponseCompletion::Incomplete)) => {
                    terminal.outcome = Some(TerminalOutcome::Incomplete);
                    terminal.error_category = Some(error_codes::RESPONSE_INCOMPLETE);
                }
                Ok(Some(ResponseCompletion::Invalid)) | Err(()) => {
                    self.fail_stream(error_codes::STREAM_INVALID);
                    return false;
                }
                Ok(None) => {
                    let protocol_terminal = self.event.as_ref().is_some_and(|event| {
                        event.wire_api == WireApi::Responses
                            && terminal
                                .event_payload
                                .as_ref()
                                .and_then(ResponseCompletion::from_responses_event)
                                .is_some()
                    });
                    if terminal.outcome.is_some() && !protocol_terminal {
                        self.fail_stream(error_codes::STREAM_INVALID);
                        return false;
                    }
                }
            }
        }
        if let Some(terminal_event_payload) = terminal.event_payload.as_ref() {
            if let (Some(capture), Some(event)) =
                (self.tool_call_capture.as_ref(), self.event.as_ref())
            {
                crate::poison::mutex(capture).observe(terminal_event_payload, event.wire_api);
            }
            if let Some(usage_event) = self.event.as_mut() {
                usage_event.observe_response_metadata(terminal_event_payload);
                usage_event
                    .tool_use
                    .observe_stream_payload(terminal_event_payload);
                usage_event
                    .tool_use
                    .observe_tool_calls_ready(self.started.elapsed().as_millis() as u64);
            }
            self.capture_native_event(terminal_event_payload);
        } else if terminal.is_compaction {
            self.native_replay_capture.mark_unmaterialized();
        }
        self.observe_first_output(terminal.has_output_delta);
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
        true
    }

    /// Gemini's native SSE ends at EOF rather than with `response.completed`.
    /// Require a recognized final candidate or prompt block before treating
    /// that EOF as a completed request; keep the provider bytes untouched.
    pub(in crate::gateway::streaming) fn ingest_native_gemini(&mut self, bytes: &[u8]) -> bool {
        self.ingest_frames(bytes, Self::ingest_native_gemini_frame)
    }

    fn ingest_native_gemini_frame(&mut self, event: &[u8]) -> bool {
        let terminal = parse_sse_event(event);
        if let Some(usage) = terminal.usage.as_ref() {
            self.merge_usage_snapshot(usage);
        }
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
        if terminal.has_data {
            if let (Some(capture), Some(payload)) = (
                self.tool_call_capture.as_ref(),
                terminal.event_payload.as_ref(),
            ) {
                crate::poison::mutex(capture).observe(payload, WireApi::Gemini);
            }
            if terminal
                .event_payload
                .as_ref()
                .is_some_and(|payload| self.native_gemini_completion.observe(payload).is_err())
            {
                self.fail_stream(error_codes::STREAM_INVALID);
                return false;
            }
            if terminal.event_payload.is_none() {
                let completion = self.native_gemini_completion.outcome();
                if completion == ResponseCompletion::Invalid {
                    self.fail_stream(error_codes::STREAM_INCOMPLETE);
                    return false;
                }
                let complete = completion == ResponseCompletion::Completed;
                self.finish(
                    Some(complete),
                    (!complete).then_some(error_codes::RESPONSE_INCOMPLETE),
                );
                self.terminated = true;
                return true;
            }
            if let Some(payload) = terminal.event_payload.as_ref() {
                self.capture_native_event(payload);
            }
        }
        self.observe_first_output(terminal.has_output_delta);
        true
    }

    fn merge_usage_snapshot(&mut self, usage: &Value) {
        crate::protocol::merge_usage(&mut self.usage_snapshot, usage);
        if let (Some(event), Some(snapshot)) = (self.event.as_mut(), self.usage_snapshot.as_ref()) {
            apply_usage(event, snapshot);
        }
    }

    pub(super) fn record_transport_error(&mut self, error: &(dyn std::error::Error + 'static)) {
        self.set_upstream_error(Some(super::super::diagnostics::transport_error(error)));
    }

    fn set_upstream_error(&mut self, details: Option<crate::usage::UpstreamErrorDetails>) {
        if let Some(usage_event) = self.event.as_mut() {
            usage_event.upstream_error = details.map(|mut details| {
                details.http_status = Some(usage_event.http_status);
                details
            });
        }
    }

    fn capture_native_event(&mut self, payload: &Value) {
        let Some(shared) = &self.native_response else {
            return;
        };
        if self
            .event
            .as_ref()
            .is_some_and(|event| event.wire_api == WireApi::GeminiInteractions)
        {
            if crate::protocol::interactions::terminal_event(payload).is_some() {
                *crate::poison::mutex(shared) = Some(payload.clone());
            }
            return;
        }
        if self
            .event
            .as_ref()
            .is_some_and(|event| event.wire_api == WireApi::Gemini)
        {
            for candidate in payload
                .get("candidates")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|candidate| candidate["finishReason"] == "CONTINUATION")
            {
                let mut checkpoint = crate::poison::mutex(shared);
                let checkpoint =
                    checkpoint.get_or_insert_with(|| serde_json::json!({"candidates":[]}));
                checkpoint["candidates"].as_array_mut().expect("native candidate checkpoint").push(serde_json::json!({
                    "index":candidate.get("index").and_then(Value::as_u64).unwrap_or(0),
                    "finishReason":"CONTINUATION","continuationToken":candidate["continuationToken"]
                }));
            }
        } else {
            self.native_replay_capture.observe(payload);
        }
    }

    fn capture_native_response(&mut self, completed_response: Option<Value>) {
        if self.event.as_ref().is_some_and(|event| {
            matches!(
                event.wire_api,
                crate::WireApi::Gemini | crate::WireApi::GeminiInteractions
            )
        }) {
            return;
        }
        let Some(shared) = self.native_response.as_ref() else {
            return;
        };
        let Some(replayed_response) = std::mem::take(&mut self.native_replay_capture)
            .finish(completed_response, self.response_id.as_deref())
        else {
            return;
        };
        *crate::poison::mutex(shared) = Some(replayed_response);
    }
}

fn classify_sse_prefix(bytes: &[u8]) -> Option<bool> {
    let mut offset = 0;
    let mut data_bytes = Vec::new();
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
            if let Some(event_name) = line.strip_prefix(b"event:")
                && is_terminal_sse_event(event_name.trim_ascii())
            {
                return Some(true);
            }
            if let Some(data_line) = line.strip_prefix(b"data:") {
                if !data_bytes.is_empty() {
                    data_bytes.push(b'\n');
                }
                data_bytes.extend_from_slice(data_line.strip_prefix(b" ").unwrap_or(data_line));
                return classify_json_error_prefix(&data_bytes);
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
        if let Some(event_name) = line.strip_prefix(b"event:") {
            if is_terminal_sse_event(event_name.trim_ascii()) {
                return Some(true);
            }
        } else if let Some(data_line) = line.strip_prefix(b"data:") {
            if !data_bytes.is_empty() {
                data_bytes.push(b'\n');
            }
            data_bytes.extend_from_slice(data_line.strip_prefix(b" ").unwrap_or(data_line));
            if let Some(is_error) = classify_json_error_prefix(&data_bytes) {
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
        let Some(event_name) = bytes.strip_prefix(b"event:") else {
            return false;
        };
        let event_name = event_name.trim_ascii();
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
        .any(|event| event.starts_with(event_name));
    };
    bytes[..line_end]
        .strip_prefix(b"event:")
        .is_some_and(|event_name| is_terminal_sse_event(event_name.trim_ascii()))
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
    let (key, consumed) = read_json_string_prefix(&bytes[offset..])?;
    let key = key?;
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
    let (event_type_bytes, _) = read_json_string_prefix(&bytes[offset..])?;
    let Some(event_type_bytes) = event_type_bytes else {
        return Some(false);
    };
    Some(is_terminal_sse_event(event_type_bytes))
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
    let Some(mut terminal_event_payload) = terminal.event_payload.clone() else {
        return event.to_vec();
    };
    let category = terminal
        .error_category
        .unwrap_or(error_codes::RESPONSE_INCOMPLETE);
    let origin = origin.for_category(category);
    if !super::super::super::errors::prefix_error_value(&mut terminal_event_payload, origin) {
        return event.to_vec();
    }
    let Ok(encoded_terminal_event) = serde_json::to_vec(&terminal_event_payload) else {
        return event.to_vec();
    };
    let mut rewritten = Vec::with_capacity(event.len().saturating_add(origin.label().len() + 2));
    let mut wrote_data = false;
    for line in crate::protocol::sse_lines(event) {
        if line.strip_prefix(b"data:").is_some() {
            if !wrote_data {
                rewritten.extend_from_slice(b"data: ");
                rewritten.extend_from_slice(&encoded_terminal_event);
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
