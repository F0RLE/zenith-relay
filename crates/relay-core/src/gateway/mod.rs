use crate::{Error, GatewayRuntime, UsageEvent};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HOST, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::Stream;
use serde_json::{json, Value};
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const MAX_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;

pub fn router(runtime: Arc<GatewayRuntime>) -> Router {
    Router::new()
        .route("/v1/models", get(models))
        .route("/v1/responses", post(responses))
        .with_state(runtime)
}

async fn models(State(runtime): State<Arc<GatewayRuntime>>, headers: HeaderMap) -> Response<Body> {
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    if !runtime.authenticate(headers.get(AUTHORIZATION)) {
        return unauthorized();
    }
    match runtime.discover_models().await {
        Ok(models) => Json(json!({
            "object": "list",
            "data": models.into_iter().map(|id| json!({
                "id": id,
                "object": "model",
                "owned_by": runtime.source_id,
            })).collect::<Vec<_>>()
        }))
        .into_response(),
        Err(_) => api_error(
            StatusCode::BAD_GATEWAY,
            "upstream model discovery failed",
            "upstream_error",
        ),
    }
}

async fn responses(
    State(runtime): State<Arc<GatewayRuntime>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    if !runtime.authenticate(headers.get(AUTHORIZATION)) {
        return unauthorized();
    }

    let request: Value = match serde_json::from_slice(&body) {
        Ok(Value::Object(request)) => Value::Object(request),
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "request body must be a JSON object",
                "invalid_request",
            )
        }
    };
    let Some(model) = request
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string)
    else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "model must be a non-empty string",
            "invalid_request",
        );
    };
    let stream = match request.get("stream") {
        Some(Value::Bool(stream)) => *stream,
        Some(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "stream must be a boolean",
                "invalid_request",
            )
        }
        None => false,
    };
    if !runtime.model_allowed(&model) {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available for this local key",
            "model_not_found",
        );
    }

    let started = Instant::now();
    let request_id = request_id();
    let client = if stream {
        &runtime.client
    } else {
        &runtime.bounded_client
    };
    let upstream = match client
        .post(runtime.responses_url.clone())
        .header(AUTHORIZATION, runtime.source_authorization())
        .header(CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            emit_usage(
                &runtime,
                usage_event(
                    &runtime,
                    request_id,
                    model,
                    false,
                    StatusCode::BAD_GATEWAY.as_u16(),
                    Some("upstream_transport".to_string()),
                    started.elapsed().as_millis() as u64,
                ),
            );
            return api_error(
                StatusCode::BAD_GATEWAY,
                "upstream request failed",
                "upstream_error",
            );
        }
    };

    let status = upstream.status();
    let response_headers = upstream.headers().clone();
    let event = usage_event(
        &runtime,
        request_id,
        model,
        status.is_success(),
        status.as_u16(),
        (!status.is_success()).then(|| "upstream_status".to_string()),
        0,
    );
    if stream {
        let usage_stream = UsageStream::new(
            upstream.bytes_stream(),
            runtime.usage.clone(),
            event,
            started,
        );
        return proxy_response(status, &response_headers, Body::from_stream(usage_stream));
    }

    match crate::runtime::collect_limited(upstream, crate::runtime::MAX_NON_STREAM_BODY_BYTES).await
    {
        Ok(bytes) => {
            let mut event = event;
            event.latency_ms = started.elapsed().as_millis() as u64;
            populate_tokens(&mut event, &bytes);
            emit_usage(&runtime, event);
            proxy_response(status, &response_headers, Body::from(bytes))
        }
        Err(error) => {
            let mut event = event;
            event.success = false;
            event.http_status = StatusCode::BAD_GATEWAY.as_u16();
            let too_large = matches!(error, Error::UpstreamBodyTooLarge);
            event.error_category = Some(if too_large {
                "upstream_body_too_large".to_string()
            } else {
                "upstream_body".to_string()
            });
            event.latency_ms = started.elapsed().as_millis() as u64;
            emit_usage(&runtime, event);
            api_error(
                StatusCode::BAD_GATEWAY,
                if too_large {
                    "upstream response is too large"
                } else {
                    "upstream response failed"
                },
                "upstream_error",
            )
        }
    }
}

fn valid_local_host(headers: &HeaderMap) -> bool {
    let Some(host) = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<axum::http::uri::Authority>().ok())
    else {
        return false;
    };
    host.host().eq_ignore_ascii_case("localhost")
        || host
            .host()
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn invalid_host() -> Response<Body> {
    api_error(
        StatusCode::MISDIRECTED_REQUEST,
        "Host must target the local gateway",
        "invalid_host",
    )
}

fn unauthorized() -> Response<Body> {
    let mut response = api_error(
        StatusCode::UNAUTHORIZED,
        "local API key is missing or invalid",
        "invalid_api_key",
    );
    response
        .headers_mut()
        .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

fn api_error(status: StatusCode, message: &str, code: &str) -> Response<Body> {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": "relay_error",
                "code": code,
            }
        })),
    )
        .into_response()
}

fn proxy_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    for name in [CONTENT_TYPE, CACHE_CONTROL] {
        if let Some(value) = upstream_headers.get(name.as_str()) {
            response.headers_mut().insert(name, value.clone());
        }
    }
    response
}

fn usage_event(
    runtime: &GatewayRuntime,
    request_id: String,
    model: String,
    success: bool,
    http_status: u16,
    error_category: Option<String>,
    latency_ms: u64,
) -> UsageEvent {
    UsageEvent {
        request_id,
        local_key_id: runtime.local_key_id.clone(),
        source_id: runtime.source_id.clone(),
        requested_model: Some(model.clone()),
        resolved_model: Some(model),
        wire_api: runtime.wire_api,
        success,
        http_status,
        error_category,
        latency_ms,
        ttft_ms: None,
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
    }
}

fn populate_tokens(event: &mut UsageEvent, body: &[u8]) {
    let Ok(body) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    let Some(usage) = body.get("usage").or_else(|| {
        body.get("response")
            .and_then(|response| response.get("usage"))
    }) else {
        return;
    };
    apply_usage(event, usage);
}

fn emit_usage(runtime: &GatewayRuntime, event: UsageEvent) {
    emit_callback(&runtime.usage, event);
}

fn emit_callback(callback: &crate::UsageCallback, event: UsageEvent) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(event)));
}

fn request_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("relay-{timestamp}-{sequence}")
}

struct UsageStream<S> {
    inner: Pin<Box<S>>,
    callback: crate::UsageCallback,
    event: Option<UsageEvent>,
    started: Instant,
    sse_pending: Vec<u8>,
    sse_tracking_disabled: bool,
}

impl<S> UsageStream<S> {
    fn new(stream: S, callback: crate::UsageCallback, event: UsageEvent, started: Instant) -> Self {
        Self {
            inner: Box::pin(stream),
            callback,
            event: Some(event),
            started,
            sse_pending: Vec::new(),
            sse_tracking_disabled: false,
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
        event.latency_ms = self.started.elapsed().as_millis() as u64;
        emit_callback(&self.callback, event);
    }

    fn observe_sse(&mut self, bytes: &[u8]) {
        if self.sse_tracking_disabled {
            return;
        }
        if self.sse_pending.len().saturating_add(bytes.len()) > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.sse_tracking_disabled = true;
            self.finish(Some(false), Some("stream_event_too_large"));
            return;
        }
        self.sse_pending.extend_from_slice(bytes);
        while let Some(end) = sse_event_end(&self.sse_pending) {
            let event = self.sse_pending.drain(..end).collect::<Vec<_>>();
            let terminal = parse_sse_event(&event);
            if let Some(usage) = terminal.usage {
                if let Some(current) = self.event.as_mut() {
                    apply_usage(current, &usage);
                }
            }
            match terminal.outcome {
                Some(TerminalOutcome::Success) => self.finish(None, None),
                Some(TerminalOutcome::Failure) => {
                    self.finish(Some(false), Some("upstream_terminal"));
                }
                None => {}
            }
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
        match this.inner.as_mut().poll_next(context) {
            Poll::Ready(Some(Ok(bytes))) => {
                if this
                    .event
                    .as_ref()
                    .is_some_and(|event| event.ttft_ms.is_none() && !bytes.is_empty())
                {
                    if let Some(event) = this.event.as_mut() {
                        event.ttft_ms = Some(this.started.elapsed().as_millis() as u64);
                    }
                }
                this.observe_sse(&bytes);
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(error))) => {
                this.finish(Some(false), Some("upstream_stream"));
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                if this.event.as_ref().is_some_and(|event| event.success) {
                    this.finish(Some(false), Some("stream_incomplete"));
                } else {
                    this.finish(None, None);
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Default)]
struct TerminalEvent {
    outcome: Option<TerminalOutcome>,
    usage: Option<Value>,
}

#[derive(Debug, Eq, PartialEq)]
enum TerminalOutcome {
    Success,
    Failure,
}

fn sse_event_end(bytes: &[u8]) -> Option<usize> {
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

fn parse_sse_event(event: &[u8]) -> TerminalEvent {
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
    if data == b"[DONE]" {
        return TerminalEvent {
            outcome: Some(TerminalOutcome::Success),
            usage: None,
        };
    }
    let Ok(value) = serde_json::from_slice::<Value>(&data) else {
        return TerminalEvent::default();
    };
    let outcome = match value.get("type").and_then(Value::as_str) {
        Some("response.completed" | "response.done") => Some(TerminalOutcome::Success),
        Some("response.failed" | "response.incomplete" | "error") => Some(TerminalOutcome::Failure),
        _ => None,
    };
    let usage = value
        .get("usage")
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("usage"))
        })
        .cloned();
    TerminalEvent { outcome, usage }
}

fn apply_usage(event: &mut UsageEvent, usage: &Value) {
    event.input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
    event.output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
    event.total_tokens = usage.get("total_tokens").and_then(Value::as_u64);
}

impl<S> Drop for UsageStream<S> {
    fn drop(&mut self) {
        self.finish(Some(false), Some("client_cancelled"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{convert::Infallible, sync::Mutex};

    #[test]
    fn oversized_sse_event_is_recorded_as_failure() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut stream = UsageStream::new(
            futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            UsageEvent {
                request_id: "request".into(),
                local_key_id: "key".into(),
                source_id: "source".into(),
                requested_model: Some("model".into()),
                resolved_model: Some("model".into()),
                wire_api: crate::WireApi::Responses,
                success: true,
                http_status: 200,
                error_category: None,
                latency_ms: 0,
                ttft_ms: None,
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
            },
            Instant::now(),
        );
        stream.observe_sse(&vec![b'x'; MAX_SSE_EVENT_BYTES + 1]);
        drop(stream);

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert_eq!(
            events[0].error_category.as_deref(),
            Some("stream_event_too_large")
        );
    }

    #[test]
    fn all_responses_error_terminal_types_are_failures() {
        for event_type in ["response.failed", "response.incomplete", "error"] {
            let event = format!("data: {{\"type\":\"{event_type}\"}}\n\n");
            assert_eq!(
                parse_sse_event(event.as_bytes()).outcome,
                Some(TerminalOutcome::Failure)
            );
        }
    }
}
