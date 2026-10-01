use super::super::errors::{
    responses_tool_call_links_rejected_value, upstream_failure_status,
    zenith_gateway_invalid_request_value, AttemptFailure, PreservedUpstreamError,
};
use super::events::{is_empty_responses_incomplete, parse_sse_event, TerminalOutcome};
use super::UpstreamStream;
use crate::error_codes;
use crate::protocol::sse_event_end;
use axum::body::Bytes;
use futures_util::StreamExt;

pub(in crate::gateway) struct StreamBootstrapFailure {
    pub(in crate::gateway) execution: crate::scheduler::rotation::ExecutionObservation,
    pub(in crate::gateway) upstream_error: Option<crate::usage::UpstreamErrorDetails>,
    pub(in crate::gateway) failure: AttemptFailure,
    pub(in crate::gateway) preserved: Option<PreservedUpstreamError>,
    pub(in crate::gateway) zenith_gateway_invalid_request: bool,
    pub(in crate::gateway) responses_tool_call_links_rejected: bool,
}

impl From<AttemptFailure> for StreamBootstrapFailure {
    fn from(failure: AttemptFailure) -> Self {
        Self {
            execution: crate::scheduler::rotation::ExecutionObservation::unknown(),
            failure,
            upstream_error: None,
            preserved: None,
            zenith_gateway_invalid_request: false,
            responses_tool_call_links_rejected: false,
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

#[expect(
    clippy::result_large_err,
    reason = "The bounded bootstrap failure carries the diagnostics needed for retry and response ownership."
)]
pub(in crate::gateway) async fn bootstrap_stream(
    upstream: reqwest::Response,
    block_degraded_routes: bool,
) -> Result<(reqwest::header::HeaderMap, Bytes, UpstreamStream), StreamBootstrapFailure> {
    let headers = upstream.headers().clone();
    let mut stream: UpstreamStream = Box::pin(upstream.bytes_stream());
    let mut buffered = Vec::new();
    let mut inspected = 0;
    let mut saw_output = false;
    let mut completed_output_items = 0_usize;
    // `response.created` and other setup frames do not make a response safe to
    // commit. Keep them private until the source produces real output or a
    // terminal event, so a pre-output provider failure can use another route.
    // No local generation deadline: quiet reasoning or provider queuing is not
    // a failed attempt. EOF, explicit provider errors and cancellation still end it.
    loop {
        match stream.next().await {
            Some(Ok(chunk)) => {
                if chunk.len() > super::MAX_SSE_EVENT_BYTES {
                    return Err(AttemptFailure::stream(error_codes::STREAM_EVENT_TOO_LARGE).into());
                }
                // Bootstrap may contain a large Responses setup event before the
                // first visible delta. Keep the same bounded budget as the
                // regular SSE parser instead of rejecting valid upstream data
                // at the old 256 KiB bootstrap threshold.
                if buffered.len().saturating_add(chunk.len()) > super::MAX_SSE_EVENT_BYTES {
                    return Err(AttemptFailure::stream(error_codes::STREAM_EVENT_TOO_LARGE).into());
                }
                buffered.extend_from_slice(&chunk);
                let mut ready_to_forward = false;
                while let Some(end) = sse_event_end(&buffered[inspected..]) {
                    let absolute_end = inspected + end;
                    let event = parse_sse_event(&buffered[inspected..absolute_end]);
                    if block_degraded_routes
                        && event
                            .payload
                            .as_ref()
                            .is_some_and(super::served_model_is_degraded)
                    {
                        // The served model is known, and this buffer has not
                        // reached the client. Drop the attempt, including a
                        // later delta that arrived in the same chunk.
                        return Err(degraded_route_stream_failure());
                    }
                    if event.has_data && !event.valid {
                        return Err(StreamBootstrapFailure {
                            upstream_error: event.upstream_error,
                            ..AttemptFailure::stream(error_codes::STREAM_INVALID).into()
                        });
                    }
                    if event.outcome == Some(TerminalOutcome::Failure) {
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
                        return Err(StreamBootstrapFailure {
                            execution: if saw_output {
                                crate::scheduler::rotation::ExecutionObservation::accepted()
                            } else {
                                failure.execution
                            },
                            failure,
                            upstream_error: event.upstream_error,
                            preserved: event.preserved_error,
                            zenith_gateway_invalid_request: event
                                .payload
                                .as_ref()
                                .is_some_and(zenith_gateway_invalid_request_value),
                            responses_tool_call_links_rejected: event
                                .payload
                                .as_ref()
                                .is_some_and(responses_tool_call_links_rejected_value),
                        });
                    }
                    if event.output_item.is_some() && !event.is_compaction {
                        completed_output_items = completed_output_items.saturating_add(1);
                    }
                    saw_output |= event.semantic_output;
                    // A zero-token incomplete response has not committed any
                    // client-visible output. Treat it as a pre-output source
                    // failure, allowing the request executor to retry another
                    // candidate. A non-empty incomplete response remains a
                    // terminal client response (for example max output).
                    if event.payload.as_ref().is_some_and(|payload| {
                        is_empty_responses_incomplete(payload, saw_output, completed_output_items)
                    }) {
                        return Err(AttemptFailure::stream(error_codes::STREAM_INCOMPLETE).into());
                    }
                    let terminal = event.outcome.is_some();
                    ready_to_forward |= terminal || event.semantic_output;
                    inspected = absolute_end;
                    if terminal {
                        // A transport chunk may also contain later frames. The
                        // first terminal owns the response; never expose its tail.
                        buffered.truncate(inspected);
                        break;
                    }
                }
                if ready_to_forward {
                    return Ok((headers, Bytes::from(buffered), stream));
                }
            }
            Some(Err(error)) => return Err(AttemptFailure::transport(&error).into()),
            None => return Err(AttemptFailure::stream(error_codes::STREAM_INCOMPLETE).into()),
        }
    }
}
