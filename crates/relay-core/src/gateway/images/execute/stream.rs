use super::super::{ImageEndpoint, MAX_IMAGE_EVENT_BYTES, native::ImageEnvelope};
use crate::gateway::{
    now_ms,
    response::{emit_usage, proxy_sse_response},
};
use crate::runtime::CandidateLease;
use crate::{GatewayRuntime, UsageEvent, error_codes};
use axum::{
    body::{Body, Bytes},
    http::Response,
};
use futures_util::{Stream, StreamExt};
use std::{convert::Infallible, pin::Pin, sync::Arc, time::Instant};

/// Owned by the response body even before its first poll. Drop accounts for a
/// disconnected client without retrying an accepted/uncertain generation.
struct ImageCompletion {
    runtime: Arc<GatewayRuntime>,
    lease: Arc<CandidateLease>,
    event: Option<UsageEvent>,
    started: Instant,
}

#[cfg(test)]
mod tests;

impl ImageCompletion {
    fn finish(&mut self, error: Option<&str>) {
        let Some(mut event) = self.event.take() else {
            return;
        };
        event.latency_ms = self.started.elapsed().as_millis() as u64;
        event.generation_ms = event
            .ttft_ms
            .map(|ttft| event.latency_ms.saturating_sub(ttft));
        event.success = error.is_none();
        event.error_category = error.map(str::to_owned);
        if error.is_none() {
            let recovered = self
                .lease
                .settle_rotation_success_with_latency(now_ms(), event.ttft_ms);
            event.consecutive_failures = recovered.then_some(0);
        } else {
            event.http_status = 502;
            if let Some(routing) = event.routing.as_mut() {
                routing.attempt_outcome = Some(if error == Some(error_codes::CLIENT_CANCELLED) {
                    crate::AttemptOutcome::ClientDisconnected
                } else {
                    crate::AttemptOutcome::ExecutionUncertain
                });
            }
            self.lease.settle_rotation_unknown(now_ms());
        }
        emit_usage(&self.runtime, event);
    }
}

impl Drop for ImageCompletion {
    fn drop(&mut self) {
        self.finish(Some(error_codes::CLIENT_CANCELLED));
    }
}

struct ImageStream {
    upstream: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
    pending: Vec<u8>,
    completion: ImageCompletion,
    terminal_kind: String,
    expected_images: u64,
    completed_images: u64,
    finished: bool,
}

impl ImageStream {
    fn fail(&mut self, category: &'static str) -> Bytes {
        self.finished = true;
        self.completion.finish(Some(category));
        Bytes::from(format!(
            "event: error\ndata: {{\"type\":\"error\",\"error\":{{\"type\":\"upstream_error\",\"code\":\"{category}\",\"message\":\"Image stream did not complete\"}}}}\n\n"
        ))
    }

    async fn next(&mut self) -> Option<Bytes> {
        if self.finished {
            return None;
        }
        loop {
            if let Some(end) = crate::protocol::sse::event_end(&self.pending) {
                let frame: Vec<u8> = self.pending.drain(..end).collect();
                let data = crate::protocol::sse::event_data(&frame);
                if !data.is_empty() {
                    if data.as_ref() == b"[DONE]" {
                        return Some(self.fail(error_codes::STREAM_INCOMPLETE));
                    }
                    let Ok(envelope) = serde_json::from_slice::<ImageEnvelope<'_>>(&data) else {
                        return Some(self.fail(error_codes::STREAM_INVALID));
                    };
                    if let Some(event) = self.completion.event.as_mut() {
                        envelope.observe_usage(event);
                        if envelope.kind.as_deref().is_some_and(|kind| {
                            kind.ends_with(".partial_image") || kind == self.terminal_kind
                        }) {
                            event.ttft_ms.get_or_insert(self.completion.started.elapsed().as_millis() as u64);
                        }
                    }
                    if envelope.error.is_some() || envelope.kind.as_deref() == Some("error") {
                        self.finished = true;
                        if let Some(event) = self.completion.event.as_mut() {
                            event.upstream_error =
                                Some(crate::usage::UpstreamErrorDetails::from_response_body(
                                    Some(200),
                                    &data,
                                ));
                        }
                        self.completion.finish(Some(error_codes::STREAM_ERROR));
                    } else if envelope.kind.as_deref() == Some(self.terminal_kind.as_str()) {
                        if !envelope
                            .b64_json
                            .as_deref()
                            .is_some_and(|data| !data.is_empty())
                        {
                            return Some(self.fail(error_codes::IMAGE_OUTPUT_MISSING));
                        }
                        self.completed_images += 1;
                        if let Some(event) = self.completion.event.as_mut() {
                            event
                                .image_usage
                                .get_or_insert_with(Default::default)
                                .generated_images = self.completed_images;
                        }
                        if self.completed_images >= self.expected_images {
                            self.finished = true;
                            self.completion.finish(None);
                        }
                    }
                }
                return Some(Bytes::from(frame));
            }
            match self.upstream.next().await {
                Some(Ok(chunk)) => {
                    if self.pending.len().saturating_add(chunk.len()) > MAX_IMAGE_EVENT_BYTES {
                        return Some(self.fail(error_codes::STREAM_EVENT_TOO_LARGE));
                    }
                    self.pending.extend_from_slice(&chunk);
                }
                Some(Err(_)) | None => return Some(self.fail(error_codes::STREAM_INCOMPLETE)),
            }
        }
    }
}

pub(super) fn image_stream_response(
    runtime: Arc<GatewayRuntime>,
    lease: Arc<CandidateLease>,
    upstream: reqwest::Response,
    event: UsageEvent,
    started: Instant,
    endpoint: ImageEndpoint,
    expected_images: u64,
) -> Response<Body> {
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let state = ImageStream {
        upstream: Box::pin(upstream.bytes_stream()),
        pending: Vec::new(),
        completion: ImageCompletion {
            runtime,
            lease,
            event: Some(event),
            started,
        },
        terminal_kind: format!("{}.completed", endpoint.stream_prefix()),
        expected_images: expected_images.max(1),
        completed_images: 0,
        finished: false,
    };
    let stream = futures_util::stream::unfold(state, |mut state| async move {
        state
            .next()
            .await
            .map(|bytes| (Ok::<_, Infallible>(bytes), state))
    });
    proxy_sse_response(status, &headers, Body::from_stream(stream))
}
