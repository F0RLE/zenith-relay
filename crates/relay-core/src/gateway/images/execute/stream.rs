use super::super::{
    ImageEndpoint, MAX_IMAGE_EVENT_BYTES,
    native::{ImageEnvelope, ImageUsageState},
};
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

mod normalize;
#[cfg(test)]
mod tests;

impl ImageCompletion {
    fn finish(&mut self, error: Option<&str>) {
        let Some(mut event) = self.event.take() else {
            return;
        };
        event.latency_ms = self.started.elapsed().as_millis() as u64;
        if let Some(transport) = event
            .routing
            .as_mut()
            .and_then(|routing| routing.transport.as_mut())
        {
            if let Some(completed) = transport.upstream_response_ms {
                event.latency_ms = completed;
            }
            if error.is_none() {
                transport.client_response_ready_ms =
                    Some(self.started.elapsed().as_millis() as u64);
            }
        }
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
    incoming: Bytes,
    event_limit: usize,
    completion: ImageCompletion,
    terminal_kind: String,
    expected_images: u64,
    completed_images: u64,
    normalize_codex_edits: bool,
    usage: ImageUsageState,
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
                let mut normalized_frame = None;
                if !data.is_empty() {
                    if data.as_ref() == b"[DONE]" {
                        return Some(self.fail(error_codes::STREAM_INCOMPLETE));
                    }
                    let Ok(envelope) = serde_json::from_slice::<ImageEnvelope<'_>>(&data) else {
                        return Some(self.fail(error_codes::STREAM_INVALID));
                    };
                    let normalized_kind = if self.normalize_codex_edits {
                        envelope
                            .kind
                            .as_deref()
                            .and_then(|kind| kind.strip_prefix("image_generation."))
                            .map(|suffix| format!("image_edit.{suffix}"))
                    } else {
                        None
                    };
                    if let Some(kind) = &normalized_kind {
                        let Some(rewritten) =
                            normalize::codex_edit_frame(&frame, &data, kind, self.event_limit)
                        else {
                            return Some(self.fail(error_codes::STREAM_INVALID));
                        };
                        normalized_frame = Some(rewritten);
                    }
                    let kind = normalized_kind.as_deref().or(envelope.kind.as_deref());
                    let elapsed_ms = self.completion.started.elapsed().as_millis() as u64;
                    let has_image = envelope
                        .b64_json
                        .as_deref()
                        .is_some_and(|data| !data.trim().is_empty());
                    let image_output = has_image
                        && kind.is_some_and(|kind| {
                            kind.ends_with(".partial_image") || kind == self.terminal_kind
                        });
                    if let Some(event) = self.completion.event.as_mut() {
                        if let Some(transport) = event
                            .routing
                            .as_mut()
                            .and_then(|routing| routing.transport.as_mut())
                        {
                            transport
                                .stream_response
                                .get_or_insert_default()
                                .observe_event(true, image_output, elapsed_ms);
                            if image_output {
                                transport.client_first_output_ms.get_or_insert(elapsed_ms);
                            }
                        }
                        if kind == Some(self.terminal_kind.as_str()) {
                            super::super::diagnostics::observe_response(
                                &self.completion.runtime,
                                event,
                                &data,
                            );
                        }
                        self.usage.observe(event, &envelope);
                        if image_output {
                            event.ttft_ms.get_or_insert(elapsed_ms);
                        }
                    }
                    if envelope.error.is_some() || kind == Some("error") {
                        self.finished = true;
                        if let Some(event) = self.completion.event.as_mut() {
                            if let Some(transport) = event
                                .routing
                                .as_mut()
                                .and_then(|routing| routing.transport.as_mut())
                            {
                                transport.upstream_response_ms = Some(elapsed_ms);
                            }
                            event.upstream_error =
                                Some(crate::usage::UpstreamErrorDetails::from_response_body(
                                    Some(200),
                                    &data,
                                ));
                        }
                        self.completion.finish(Some(error_codes::STREAM_ERROR));
                    } else if kind == Some(self.terminal_kind.as_str()) {
                        if !envelope
                            .b64_json
                            .as_deref()
                            .is_some_and(|data| !data.trim().is_empty())
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
                            if let Some(transport) = self
                                .completion
                                .event
                                .as_mut()
                                .and_then(|event| event.routing.as_mut()?.transport.as_mut())
                            {
                                transport.upstream_response_ms = Some(elapsed_ms);
                            }
                            self.completion.finish(None);
                        }
                    }
                }
                return Some(Bytes::from(normalized_frame.unwrap_or(frame)));
            }
            if !self.incoming.is_empty() {
                let available = self.event_limit.saturating_sub(self.pending.len());
                if available == 0 {
                    return Some(self.fail(error_codes::STREAM_EVENT_TOO_LARGE));
                }
                let part = self.incoming.split_to(available.min(self.incoming.len()));
                self.pending.extend_from_slice(&part);
                continue;
            }
            match self.upstream.next().await {
                Some(Ok(chunk)) => {
                    if let Some(transport) = self
                        .completion
                        .event
                        .as_mut()
                        .and_then(|event| event.routing.as_mut()?.transport.as_mut())
                    {
                        transport
                            .stream_response
                            .get_or_insert_default()
                            .observe_body(
                                !chunk.is_empty(),
                                self.completion.started.elapsed().as_millis() as u64,
                            );
                    }
                    self.incoming = chunk;
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
    mut event: UsageEvent,
    started: Instant,
    endpoint: ImageEndpoint,
    expected_images: u64,
) -> Response<Body> {
    let status = upstream.status();
    let headers = upstream.headers().clone();
    if let Some(transport) = event
        .routing
        .as_mut()
        .and_then(|routing| routing.transport.as_mut())
    {
        transport.downstream_http_status = Some(status.as_u16());
    }
    let normalize_codex_edits = event.account_id.is_some() && endpoint == ImageEndpoint::Edits;
    let state = ImageStream {
        upstream: Box::pin(upstream.bytes_stream()),
        pending: Vec::new(),
        incoming: Bytes::new(),
        event_limit: MAX_IMAGE_EVENT_BYTES,
        completion: ImageCompletion {
            runtime,
            lease,
            event: Some(event),
            started,
        },
        terminal_kind: format!("{}.completed", endpoint.stream_prefix()),
        expected_images: expected_images.max(1),
        completed_images: 0,
        normalize_codex_edits,
        usage: ImageUsageState::default(),
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
