use super::super::errors::{
    AttemptFailure, PreservedUpstreamError,
    responses_function_call_output_has_invalid_call_id_value,
    responses_tool_call_links_rejected_value, upstream_failure_status,
};
use super::UpstreamStream;
use super::events::{TerminalOutcome, is_empty_responses_terminal, parse_sse_event};
use crate::error_codes;
use crate::protocol::completion::{GeminiStreamCompletion, ResponseCompletion};
use crate::protocol::sse_event_end;
use axum::body::Bytes;
use futures_util::StreamExt;
use std::time::Instant;

pub(in crate::gateway) enum StreamBootstrapObservation<'a> {
    Transport {
        started: Instant,
        transport: &'a mut crate::usage::TransportDiagnostics,
    },
    Buffered {
        started: Instant,
        timing: &'a mut crate::gateway::response::BufferedResponseTiming,
    },
}

impl StreamBootstrapObservation<'_> {
    fn body(&mut self, nonempty: bool) {
        if nonempty && let Self::Transport { started, transport } = self {
            transport
                .stream_response
                .get_or_insert_default()
                .observe_body(true, started.elapsed().as_millis() as u64);
        }
    }

    fn event(&mut self, event: &super::events::TerminalEvent) {
        match self {
            Self::Transport { started, transport } if event.event_payload.is_some() => {
                let elapsed_ms = started.elapsed().as_millis() as u64;
                let timing = transport.stream_response.get_or_insert_default();
                timing.observe_event(true, event.semantic_output, elapsed_ms);
                if let Some(payload) = &event.event_payload {
                    timing.observe_reasoning(payload, elapsed_ms);
                    timing.observe_output(payload, elapsed_ms);
                }
            }
            Self::Buffered { started, timing } => {
                timing.observe(event, started.elapsed().as_millis() as u64);
            }
            Self::Transport { .. } => {}
        }
    }
}

pub(in crate::gateway) struct StreamBootstrapFailure {
    pub(in crate::gateway) execution: crate::scheduler::rotation::ExecutionObservation,
    pub(in crate::gateway) upstream_error: Option<crate::usage::UpstreamErrorDetails>,
    pub(in crate::gateway) failure: AttemptFailure,
    pub(in crate::gateway) preserved: Option<PreservedUpstreamError>,
    pub(in crate::gateway) invalid_function_call_output_call_id: bool,
    pub(in crate::gateway) responses_tool_call_links_rejected: bool,
    pub(in crate::gateway) partial_body: Vec<u8>,
}

impl From<AttemptFailure> for StreamBootstrapFailure {
    fn from(failure: AttemptFailure) -> Self {
        Self {
            execution: crate::scheduler::rotation::ExecutionObservation::unknown(),
            failure,
            upstream_error: None,
            preserved: None,
            invalid_function_call_output_call_id: false,
            responses_tool_call_links_rejected: false,
            partial_body: Vec::new(),
        }
    }
}

pub(in crate::gateway) fn degraded_route_stream_failure() -> StreamBootstrapFailure {
    let failure = AttemptFailure::classified_with_hint(
        upstream_failure_status(error_codes::UPSTREAM_ROUTE_DEGRADED),
        error_codes::UPSTREAM_ROUTE_DEGRADED,
        Default::default(),
    );
    // `From<AttemptFailure>` marks the attempt unknown. This rejection is
    // proven before any client byte, so rotation may try another account.
    let execution = failure.execution;
    StreamBootstrapFailure {
        execution,
        ..failure.into()
    }
}

#[cfg(test)]
pub(in crate::gateway) async fn bootstrap_stream(
    upstream: reqwest::Response,
    expected_model: Option<&str>,
) -> Result<(reqwest::header::HeaderMap, Bytes, UpstreamStream), Box<StreamBootstrapFailure>> {
    bootstrap_stream_impl(
        upstream,
        expected_model,
        false,
        crate::protocol::sse::MAX_EVENT_BYTES,
        false,
    )
    .await
}

#[cfg(test)]
pub(super) async fn bootstrap_stream_impl(
    upstream: reqwest::Response,
    expected_model: Option<&str>,
    basis_points: bool,
    max_buffered_bytes: usize,
    prewarm: bool,
) -> Result<(reqwest::header::HeaderMap, Bytes, UpstreamStream), Box<StreamBootstrapFailure>> {
    bootstrap_observed_stream(
        upstream,
        expected_model,
        basis_points,
        max_buffered_bytes,
        prewarm,
        None,
    )
    .await
}

pub(in crate::gateway) async fn bootstrap_observed_stream(
    upstream: reqwest::Response,
    expected_model: Option<&str>,
    basis_points: bool,
    max_buffered_bytes: usize,
    prewarm: bool,
    mut observation: Option<StreamBootstrapObservation<'_>>,
) -> Result<(reqwest::header::HeaderMap, Bytes, UpstreamStream), Box<StreamBootstrapFailure>> {
    let http_status = upstream.status().as_u16();
    let headers = upstream.headers().clone();
    // A preserved non-identity encoding means decoding has not been established.
    // Do not label encoded application reads as decoded or raw wire arrivals.
    let decoded = headers
        .get_all(reqwest::header::CONTENT_ENCODING)
        .iter()
        .all(|value| value.as_bytes().eq_ignore_ascii_case(b"identity"));
    let mut stream: UpstreamStream = Box::pin(upstream.bytes_stream());
    if basis_points {
        stream = super::basis_points::guard_progress(
            stream,
            crate::transport::basis_points_progress_timeout(),
        );
    }
    let mut buffered = Vec::new();
    let mut inspected = 0;
    let mut saw_output = false;
    let mut saw_generation = false;
    let mut completed_output_items = 0_usize;
    let mut gemini_completion = GeminiStreamCompletion::default();
    let mut native_gemini = false;
    // `response.created` and other setup frames do not make a response safe to
    // commit. Keep them private until the source produces real output or a
    // terminal event, so a pre-output provider failure can use another route.
    // No local generation deadline: quiet reasoning or provider queuing is not
    // a failed attempt. EOF, explicit provider errors and cancellation still end it.
    loop {
        match stream.next().await {
            Some(Ok(chunk)) => {
                if decoded && let Some(observation) = observation.as_mut() {
                    observation.body(!chunk.is_empty());
                }
                let available = max_buffered_bytes.saturating_sub(buffered.len());
                let prefix_len = available.min(chunk.len());
                buffered.extend_from_slice(&chunk[..prefix_len]);
                let mut ready_to_forward = false;
                while let Some(end) = sse_event_end(&buffered[inspected..]) {
                    let absolute_end = inspected + end;
                    let event = parse_sse_event(&buffered[inspected..absolute_end]);
                    if let Some(observation) = observation.as_mut() {
                        observation.event(&event);
                    }
                    saw_generation |= event.semantic_output
                        || event
                            .event_payload
                            .as_ref()
                            .is_some_and(crate::gateway::response::has_generated_tokens);
                    if expected_model.is_some_and(|expected| {
                        event.event_payload.as_ref().is_some_and(|served_model| {
                            super::served_model_is_rejected(served_model, expected)
                        })
                    }) {
                        // The served model is known, and this buffer has not
                        // reached the client. Drop the attempt, including a
                        // later delta that arrived in the same chunk.
                        let mut failure = degraded_route_stream_failure();
                        if saw_generation {
                            failure.execution =
                                crate::scheduler::rotation::ExecutionObservation::accepted();
                        }
                        failure.partial_body = buffered;
                        return Err(Box::new(failure));
                    }
                    if event.has_data && !event.valid {
                        return Err(Box::new(StreamBootstrapFailure {
                            upstream_error: event.upstream_error,
                            partial_body: buffered,
                            ..AttemptFailure::stream(error_codes::STREAM_INVALID).into()
                        }));
                    }
                    if event.outcome == Some(TerminalOutcome::Failure) {
                        buffered.truncate(absolute_end);
                        let category = event
                            .error_category
                            .unwrap_or(error_codes::UPSTREAM_TERMINAL);
                        let failure = AttemptFailure::classified_with_hint(
                            event
                                .error_status
                                .unwrap_or_else(|| upstream_failure_status(category)),
                            category,
                            event.cooldown_hint,
                        );
                        return Err(Box::new(StreamBootstrapFailure {
                            execution: if saw_generation {
                                crate::scheduler::rotation::ExecutionObservation::accepted()
                            } else {
                                failure.execution
                            },
                            failure,
                            upstream_error: event.upstream_error,
                            preserved: event.preserved_error,
                            invalid_function_call_output_call_id: event
                                .event_payload
                                .as_ref()
                                .is_some_and(
                                    responses_function_call_output_has_invalid_call_id_value,
                                ),
                            responses_tool_call_links_rejected: event
                                .event_payload
                                .as_ref()
                                .is_some_and(responses_tool_call_links_rejected_value),
                            partial_body: buffered,
                        }));
                    }
                    if event.output_item.is_some() && !event.is_compaction {
                        completed_output_items = completed_output_items.saturating_add(1);
                    }
                    saw_output |= event.semantic_output;
                    // An unexplained empty terminal is a source failure, but
                    // does not prove that generation was never accepted.
                    // Its execution stays unknown, so no automatic retry is safe.
                    // A non-empty incomplete response belongs to the client.
                    if !(prewarm && event.outcome == Some(TerminalOutcome::Success))
                        && event
                            .event_payload
                            .as_ref()
                            .is_some_and(|terminal_payload| {
                                is_empty_responses_terminal(
                                    terminal_payload,
                                    saw_output,
                                    completed_output_items,
                                )
                            })
                    {
                        return Err(Box::new(StreamBootstrapFailure {
                            partial_body: buffered,
                            ..AttemptFailure::stream(error_codes::RESPONSE_INCOMPLETE).into()
                        }));
                    }
                    if let Some(payload) = event.event_payload.as_ref() {
                        native_gemini |= payload.get("candidates").is_some()
                            || payload.get("promptFeedback").is_some()
                            || payload.get("usageMetadata").is_some();
                        if native_gemini && gemini_completion.observe(payload).is_err() {
                            return Err(Box::new(StreamBootstrapFailure {
                                partial_body: buffered,
                                ..AttemptFailure::stream(error_codes::STREAM_INVALID).into()
                            }));
                        }
                    }
                    // A Gemini finish reason closes one candidate, not the
                    // transport. Retain the rest of this chunk and late usage.
                    let terminal = event.outcome.is_some() && !native_gemini;
                    ready_to_forward |= terminal
                        || event.semantic_output
                        || native_gemini
                            && gemini_completion.outcome() != ResponseCompletion::Invalid;
                    inspected = absolute_end;
                    if terminal {
                        // A transport chunk may also contain later frames. The
                        // first terminal owns the response; never expose its tail.
                        buffered.truncate(inspected);
                        break;
                    }
                }
                if ready_to_forward {
                    if prefix_len < chunk.len() {
                        stream = Box::pin(
                            futures_util::stream::once(
                                async move { Ok(chunk.slice(prefix_len..)) },
                            )
                            .chain(stream),
                        );
                    }
                    return Ok((headers, Bytes::from(buffered), stream));
                }
                if prefix_len < chunk.len() {
                    return Err(Box::new(StreamBootstrapFailure {
                        partial_body: buffered,
                        ..AttemptFailure::stream(error_codes::STREAM_EVENT_TOO_LARGE).into()
                    }));
                }
            }
            Some(Err(error)) => {
                return Err(Box::new(StreamBootstrapFailure {
                    partial_body: buffered,
                    ..AttemptFailure::transport(&error).into()
                }));
            }
            None => {
                // Some upstreams return a JSON error even after HTTP 200. Keep its
                // structured refusal instead of reporting only a missing SSE terminal.
                if inspected == 0
                    && buffered.len() <= 64 * 1024
                    && let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&buffered)
                    && let Some(category) = super::super::errors::upstream_event_failure_category(
                        payload.get("type").and_then(serde_json::Value::as_str),
                        &payload,
                    )
                {
                    let preserved = super::events::preserved_stream_error(&payload);
                    let failure = AttemptFailure::classified_with_hint(
                        preserved.as_ref().map_or_else(
                            || upstream_failure_status(category),
                            |error| error.status,
                        ),
                        category,
                        super::super::errors::rate_limit_body_hint_value(
                            &payload,
                            std::time::SystemTime::now(),
                        ),
                    );
                    return Err(Box::new(StreamBootstrapFailure {
                        upstream_error: Some(crate::usage::UpstreamErrorDetails::from_response(
                            Some(http_status),
                            &buffered,
                            &headers,
                        )),
                        invalid_function_call_output_call_id:
                            responses_function_call_output_has_invalid_call_id_value(&payload),
                        responses_tool_call_links_rejected:
                            responses_tool_call_links_rejected_value(&payload),
                        preserved,
                        partial_body: buffered,
                        ..failure.into()
                    }));
                }
                return Err(Box::new(StreamBootstrapFailure {
                    upstream_error: Some(super::diagnostics::incomplete_stream(
                        http_status,
                        &headers,
                        &buffered,
                        inspected,
                    )),
                    partial_body: buffered,
                    ..AttemptFailure::stream(error_codes::STREAM_INCOMPLETE).into()
                }));
            }
        }
    }
}
