use super::errors::{
    canonical_upstream_status, rate_limit_body_hint_value, upstream_event_failure_category,
    upstream_failure_status, upstream_status_from_value, AttemptFailure, RateLimitBodyHint,
};
use super::response::{
    apply_usage, emit_callback, emit_usage, find_usage, response_id, response_service_tier,
    CompletionCallback,
};
use crate::runtime::DefaultServiceTier;
use crate::{GatewayRuntime, UsageEvent, WireApi};
use axum::body::Bytes;
use axum::http::StatusCode;
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime};
use tokio::time::{sleep, Instant as TokioInstant, Sleep};

const MAX_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;

const SSE_PRE_OUTPUT_RETRY_GRACE: Duration = Duration::from_secs(30);

const SSE_FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(30);

const SSE_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

const SSE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

const SSE_HEARTBEAT: &[u8] = b": keep-alive\n\n";

type UpstreamStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

pub(super) async fn bootstrap_stream(
    upstream: reqwest::Response,
) -> Result<(reqwest::header::HeaderMap, Bytes, UpstreamStream), AttemptFailure> {
    let headers = upstream.headers().clone();
    let mut stream: UpstreamStream = Box::pin(upstream.bytes_stream());
    let mut buffer = Vec::new();
    let deadline = Instant::now() + SSE_PRE_OUTPUT_RETRY_GRACE;
    loop {
        let next = if buffer.is_empty() {
            match tokio::time::timeout(SSE_FIRST_BYTE_TIMEOUT, stream.next()).await {
                Ok(next) => next,
                Err(_) => return Err(AttemptFailure::stream("stream_first_byte_timeout")),
            }
        } else {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok((headers, Bytes::from(buffer), stream));
            }
            match tokio::time::timeout(remaining, stream.next()).await {
                Ok(next) => next,
                Err(_) => return Ok((headers, Bytes::from(buffer), stream)),
            }
        };
        match next {
            Some(Ok(chunk)) => {
                if buffer.len().saturating_add(chunk.len()) > MAX_SSE_EVENT_BYTES {
                    return Err(AttemptFailure::stream("stream_event_too_large"));
                }
                buffer.extend_from_slice(&chunk);
                let mut inspected = 0;
                while let Some(end) = sse_event_end(&buffer[inspected..]) {
                    let absolute_end = inspected + end;
                    let event = parse_sse_event(&buffer[inspected..absolute_end]);
                    if event.has_data && !event.valid {
                        return Err(AttemptFailure::stream("stream_invalid"));
                    }
                    if event.outcome == Some(TerminalOutcome::Failure) {
                        let category = event.error_category.unwrap_or("upstream_terminal");
                        return Err(AttemptFailure::classified_with_hint(
                            event
                                .error_status
                                .unwrap_or_else(|| upstream_failure_status(category)),
                            category,
                            event.cooldown_hint,
                        ));
                    }
                    if event.has_output_delta || event.outcome == Some(TerminalOutcome::Success) {
                        return Ok((headers, Bytes::from(buffer), stream));
                    }
                    inspected = absolute_end;
                }
                if Instant::now() >= deadline {
                    return Ok((headers, Bytes::from(buffer), stream));
                }
            }
            Some(Err(error)) => return Err(AttemptFailure::transport(&error)),
            None => return Err(AttemptFailure::stream("stream_incomplete")),
        }
    }
}

pub(super) struct UsageStream<S> {
    pub(super) inner: Pin<Box<S>>,
    pub(super) runtime: Option<Arc<GatewayRuntime>>,
    pub(super) callback: crate::UsageCallback,
    pub(super) completion: CompletionCallback,
    pub(super) event: Option<UsageEvent>,
    pub(super) response_id: Option<String>,
    pub(super) cooldown_hint: RateLimitBodyHint,
    pub(super) started: Instant,
    pub(super) sse_pending: Vec<u8>,
    pub(super) output_pending: VecDeque<Bytes>,
    pub(super) heartbeat: Pin<Box<Sleep>>,
    pub(super) idle_watchdog: Pin<Box<Sleep>>,
    pub(super) terminated: bool,
}

impl<S> UsageStream<S> {
    #[cfg(test)]
    pub(super) fn new(
        stream: S,
        callback: crate::UsageCallback,
        event: UsageEvent,
        started: Instant,
        completion: CompletionCallback,
    ) -> Self {
        Self {
            inner: Box::pin(stream),
            runtime: None,
            callback,
            completion,
            event: Some(event),
            response_id: None,
            cooldown_hint: RateLimitBodyHint::default(),
            started,
            sse_pending: Vec::new(),
            output_pending: VecDeque::new(),
            heartbeat: Box::pin(sleep(SSE_HEARTBEAT_INTERVAL)),
            idle_watchdog: Box::pin(sleep(SSE_IDLE_TIMEOUT)),
            terminated: false,
        }
    }

    pub(super) fn with_runtime(
        stream: S,
        runtime: Arc<GatewayRuntime>,
        event: UsageEvent,
        started: Instant,
        completion: CompletionCallback,
    ) -> Self {
        let callback = runtime.usage.clone();
        Self {
            inner: Box::pin(stream),
            runtime: Some(runtime),
            callback,
            completion,
            event: Some(event),
            response_id: None,
            cooldown_hint: RateLimitBodyHint::default(),
            started,
            sse_pending: Vec::new(),
            output_pending: VecDeque::new(),
            heartbeat: Box::pin(sleep(SSE_HEARTBEAT_INTERVAL)),
            idle_watchdog: Box::pin(sleep(SSE_IDLE_TIMEOUT)),
            terminated: false,
        }
    }

    fn finish(&mut self, success: Option<bool>, category: Option<&str>) {
        let Some(mut event) = self.event.take() else {
            return;
        };
        if let Some(success) = success {
            event.success = success;
        }
        if let Some(category) = category {
            event.error_category = Some(category.to_string());
        }
        if !event.success && event.http_status < 400 {
            event.http_status = event
                .error_category
                .as_deref()
                .filter(|category| *category != "client_cancelled")
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
        if event.wire_api != WireApi::Responses {
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
            "stream_invalid" => "Upstream returned an invalid streaming event",
            "stream_event_too_large" => "Upstream streaming event exceeded the size limit",
            "stream_incomplete" => "Upstream stream ended before response.completed",
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
                    "type": "stream_error",
                    "code": category,
                    "message": message,
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

    fn fail_stream(&mut self, category: &str) -> bool {
        let framed = self.queue_responses_failure(category);
        self.finish(Some(false), Some(category));
        self.terminated = true;
        framed
    }

    fn ingest_sse(&mut self, bytes: &[u8]) {
        if self.terminated {
            return;
        }
        if self.sse_pending.len().saturating_add(bytes.len()) > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.fail_stream("stream_event_too_large");
            return;
        }
        self.sse_pending.extend_from_slice(bytes);
        while let Some(end) = sse_event_end(&self.sse_pending) {
            if end > MAX_SSE_EVENT_BYTES {
                self.sse_pending.clear();
                self.fail_stream("stream_event_too_large");
                return;
            }
            let event = self.sse_pending.drain(..end).collect::<Vec<_>>();
            let terminal = parse_sse_event(&event);
            if terminal.has_data && !terminal.valid {
                self.sse_pending.clear();
                self.fail_stream("stream_invalid");
                return;
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
                    self.finish(None, None);
                    self.terminated = true;
                    return;
                }
                Some(TerminalOutcome::Failure) => {
                    self.cooldown_hint = terminal.cooldown_hint;
                    self.finish(
                        Some(false),
                        Some(terminal.error_category.unwrap_or("upstream_terminal")),
                    );
                    self.terminated = true;
                    return;
                }
                None => {}
            }
        }
        if self.sse_pending.len() > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.fail_stream("stream_event_too_large");
        }
    }
}

impl<S, E> Stream for UsageStream<S>
where
    S: Stream<Item = std::result::Result<Bytes, E>>,
{
    type Item = std::result::Result<Bytes, E>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        loop {
            if let Some(bytes) = this.output_pending.pop_front() {
                return Poll::Ready(Some(Ok(bytes)));
            }
            if this.terminated {
                return Poll::Ready(None);
            }
            match this.inner.as_mut().poll_next(context) {
                Poll::Ready(Some(Ok(bytes))) => {
                    let now = TokioInstant::now();
                    this.heartbeat.as_mut().reset(now + SSE_HEARTBEAT_INTERVAL);
                    this.idle_watchdog.as_mut().reset(now + SSE_IDLE_TIMEOUT);
                    this.ingest_sse(&bytes);
                    if let Some(failure) = this.output_pending.pop_front() {
                        return Poll::Ready(Some(Ok(failure)));
                    }
                    return Poll::Ready(Some(Ok(bytes)));
                }
                Poll::Ready(Some(Err(error))) => {
                    if this.fail_stream("upstream_stream") {
                        continue;
                    }
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    if this.event.as_ref().is_some_and(|event| event.success) {
                        if this.fail_stream("stream_incomplete") {
                            continue;
                        }
                    } else {
                        this.finish(None, None);
                    }
                    this.sse_pending.clear();
                    this.terminated = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    if this.idle_watchdog.as_mut().poll(context).is_ready() {
                        if this.fail_stream("stream_idle_timeout") {
                            continue;
                        }
                        return Poll::Ready(None);
                    }
                    if this.heartbeat.as_mut().poll(context).is_ready() {
                        this.heartbeat
                            .as_mut()
                            .reset(TokioInstant::now() + SSE_HEARTBEAT_INTERVAL);
                        return Poll::Ready(Some(Ok(Bytes::from_static(SSE_HEARTBEAT))));
                    }
                    return Poll::Pending;
                }
            }
        }
    }
}

#[derive(Default)]
pub(super) struct TerminalEvent {
    pub(super) has_data: bool,
    pub(super) valid: bool,
    pub(super) has_output_delta: bool,
    pub(super) outcome: Option<TerminalOutcome>,
    pub(super) error_status: Option<StatusCode>,
    pub(super) error_category: Option<&'static str>,
    pub(super) cooldown_hint: RateLimitBodyHint,
    pub(super) usage: Option<Value>,
    pub(super) applied_service_tier: Option<DefaultServiceTier>,
    pub(super) response_id: Option<String>,
    pub(super) response: Option<Value>,
    pub(super) output_item: Option<Value>,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum TerminalOutcome {
    Success,
    Failure,
}

pub(super) fn sse_event_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|position| position + 2)
        .or_else(|| {
            bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
        })
}

pub(super) fn parse_sse_event(event: &[u8]) -> TerminalEvent {
    let mut data = Vec::new();
    for line in event.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(value) = line.strip_prefix(b"data:") else {
            continue;
        };
        if !data.is_empty() {
            data.push(b'\n');
        }
        data.extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
    }
    if data.is_empty() {
        return TerminalEvent::default();
    }
    if data == b"[DONE]" {
        return TerminalEvent {
            has_data: true,
            valid: true,
            has_output_delta: false,
            outcome: Some(TerminalOutcome::Success),
            error_status: None,
            error_category: None,
            cooldown_hint: RateLimitBodyHint::default(),
            usage: None,
            applied_service_tier: None,
            response_id: None,
            response: None,
            output_item: None,
        };
    }
    let Ok(value) = serde_json::from_slice::<Value>(&data) else {
        return TerminalEvent {
            has_data: true,
            ..TerminalEvent::default()
        };
    };
    let event_type = value.get("type").and_then(Value::as_str);
    let outcome = match event_type {
        Some("response.completed" | "response.done" | "message_stop") => {
            Some(TerminalOutcome::Success)
        }
        Some(
            "response.failed"
            | "response.incomplete"
            | "response.cancelled"
            | "response.canceled"
            | "error",
        ) => Some(TerminalOutcome::Failure),
        _ => None,
    };
    let error_category = upstream_event_failure_category(event_type, &value);
    let error_status = error_category.map(|category| {
        let status = upstream_status_from_value(&value)
            .filter(|status| !status.is_success())
            .unwrap_or_else(|| upstream_failure_status(category));
        canonical_upstream_status(status, category)
    });
    let cooldown_hint = rate_limit_body_hint_value(&value, SystemTime::now());
    let has_output_delta = has_output_delta(&value, event_type);
    let usage = find_usage(&value).cloned();
    let applied_service_tier = response_service_tier(&value);
    let response_id = response_id(&value).map(str::to_string);
    let response = value.get("response").cloned();
    let output_item = (value.get("type").and_then(Value::as_str)
        == Some("response.output_item.done"))
    .then(|| value.get("item").cloned())
    .flatten();
    TerminalEvent {
        has_data: true,
        valid: true,
        has_output_delta,
        outcome,
        error_status,
        error_category,
        cooldown_hint,
        usage,
        applied_service_tier,
        response_id,
        response,
        output_item,
    }
}

pub(super) fn has_output_delta(value: &Value, event_type: Option<&str>) -> bool {
    if matches!(
        event_type,
        Some(
            "response.output_text.delta"
                | "response.refusal.delta"
                | "response.function_call_arguments.delta"
                | "response.custom_tool_call_input.delta"
                | "response.mcp_call_arguments.delta"
                | "response.code_interpreter_call_code.delta"
        )
    ) && value
        .get("delta")
        .and_then(Value::as_str)
        .is_some_and(|delta| !delta.is_empty())
    {
        return true;
    }
    if event_type == Some("content_block_delta")
        && value.get("delta").is_some_and(|delta| {
            ["text", "partial_json"].into_iter().any(|key| {
                delta
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
            })
        })
    {
        return true;
    }
    value
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| choices.iter().any(chat_choice_has_output_delta))
}

fn chat_choice_has_output_delta(choice: &Value) -> bool {
    let Some(delta) = choice.get("delta") else {
        return false;
    };
    ["content", "refusal"].into_iter().any(|key| {
        delta
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    }) || delta
        .get("function_call")
        .is_some_and(function_delta_has_output)
        || delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| {
                calls.iter().any(|call| {
                    call.get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty())
                        || call.get("function").is_some_and(function_delta_has_output)
                })
            })
}

fn function_delta_has_output(function: &Value) -> bool {
    ["name", "arguments"].into_iter().any(|key| {
        function
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    })
}

impl<S> Drop for UsageStream<S> {
    fn drop(&mut self) {
        self.finish(Some(false), Some("client_cancelled"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::test_support::test_usage_event;
    use std::convert::Infallible;
    use std::sync::Mutex;

    #[test]
    fn streaming_terminal_errors_keep_the_canonical_category() {
        let terminal = parse_sse_event(
            br#"data: {"type":"response.failed","response":{"error":{"type":"usage_limit_reached","resets_in_seconds":7}}}

"#,
        );
        assert_eq!(terminal.error_category, Some("upstream_quota_exhausted"));
        assert_eq!(terminal.error_status, Some(StatusCode::TOO_MANY_REQUESTS));
        assert_eq!(terminal.cooldown_hint.retry_after_ms, Some(7_000));
        assert!(terminal.cooldown_hint.global);
    }

    #[tokio::test]
    async fn oversized_sse_event_is_recorded_as_failure() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut stream = UsageStream::new(
            futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            UsageEvent {
                request_id: "request".into(),
                attempt: 1,
                local_key_id: "key".into(),
                source_id: "source".into(),
                candidate_id: Some("source".into()),
                account_id: None,
                routing: None,
                requested_model: Some("model".into()),
                resolved_model: Some("model".into()),
                wire_api: crate::WireApi::Responses,
                service_tier: DefaultServiceTier::Standard,
                applied_service_tier: None,
                success: true,
                http_status: 200,
                error_category: None,
                cooldown_scope: None,
                retry_at_ms: None,
                consecutive_failures: Some(0),
                latency_ms: 0,
                ttft_ms: None,
                generation_ms: None,
                input_tokens: None,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                reasoning_tokens: None,
                output_tokens: None,
                total_tokens: None,
                quota_snapshot: None,
            },
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );
        stream.ingest_sse(&vec![b'x'; MAX_SSE_EVENT_BYTES + 1]);
        assert!(stream.terminated);
        assert!(stream.sse_pending.is_empty());
        let failure =
            String::from_utf8(stream.output_pending.pop_front().unwrap().to_vec()).unwrap();
        assert!(failure.starts_with("event: response.failed\ndata: "));
        assert!(failure.contains("\"code\":\"stream_event_too_large\""));
        assert!(stream.output_pending.is_empty());
        drop(stream);

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert_eq!(
            events[0].error_category.as_deref(),
            Some("stream_event_too_large")
        );
    }

    #[tokio::test]
    async fn usage_stream_forwards_chunks_without_waiting_for_an_sse_boundary() {
        let first =
            Bytes::from_static(br#"data: {"type":"response.output_text.delta","delta":"hel"#);
        let second = Bytes::from_static(b"lo\"}\n\n");
        let completed = Bytes::from_static(
            b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\"}}\n\n",
        );
        let input = futures_util::stream::iter([
            Ok::<_, Infallible>(first.clone()),
            Ok(second.clone()),
            Ok(completed.clone()),
        ]);
        let mut stream = UsageStream::new(
            input,
            Arc::new(|_| {}),
            test_usage_event(),
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );

        assert_eq!(stream.next().await.unwrap().unwrap(), first);
        assert_eq!(stream.next().await.unwrap().unwrap(), second);
        assert_eq!(stream.next().await.unwrap().unwrap(), completed);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn streaming_chat_usage_captures_cached_prompt_tokens() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut stream = UsageStream::new(
            futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            test_usage_event(),
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );
        stream.ingest_sse(
            b"data: {\"type\":\"response.completed\",\"response\":{\"service_tier\":\"default\",\"usage\":{\"prompt_tokens\":32,\"prompt_tokens_details\":{\"cached_tokens\":9,\"cache_write_tokens\":7},\"completion_tokens\":6,\"completion_tokens_details\":{\"reasoning_tokens\":4}}}}\n\n",
        );

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, Some(32));
        assert_eq!(events[0].cached_input_tokens, Some(9));
        assert_eq!(events[0].cache_write_input_tokens, Some(7));
        assert_eq!(events[0].reasoning_tokens, Some(4));
        assert_eq!(events[0].output_tokens, Some(6));
        assert_eq!(events[0].total_tokens, Some(38));
        assert_eq!(
            events[0].applied_service_tier,
            Some(DefaultServiceTier::Standard)
        );
    }

    #[test]
    fn all_responses_error_terminal_types_are_failures() {
        for event_type in [
            "response.failed",
            "response.incomplete",
            "response.cancelled",
            "response.canceled",
            "error",
        ] {
            let event = format!("data: {{\"type\":\"{event_type}\"}}\n\n");
            assert_eq!(
                parse_sse_event(event.as_bytes()).outcome,
                Some(TerminalOutcome::Failure)
            );
        }
    }

    #[test]
    fn ttft_requires_real_output_for_supported_stream_protocols() {
        for event in [
            "data: {\"type\":\"response.created\"}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"hidden\"}\n\n",
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":null}}\n\n",
        ] {
            assert!(!parse_sse_event(event.as_bytes()).has_output_delta);
        }
        for event in [
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\"}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
        ] {
            assert!(parse_sse_event(event.as_bytes()).has_output_delta);
        }
    }
}
