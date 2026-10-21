use super::super::errors::{upstream_failure_status, AttemptFailure};
use super::super::streaming::{
    has_generation_evidence, parse_sse_event, StreamBootstrapFailure, TerminalEvent,
    TerminalOutcome,
};
use crate::error_codes;
use crate::protocol::sse_event_end;
use axum::http::StatusCode;
use futures_util::StreamExt;
use serde_json::Value;
use std::time::Instant;

#[derive(Default)]
pub(in crate::gateway) struct BufferedResponseTiming {
    first_output_ms: Option<u64>,
    finished_ms: Option<u64>,
}

impl BufferedResponseTiming {
    pub(in crate::gateway) fn observe(&mut self, event: &TerminalEvent, elapsed_ms: u64) {
        // A completed snapshot is not evidence of when its first token arrived.
        if event.valid
            && event.has_output_delta
            && !event.is_compaction
            && self.first_output_ms.is_none()
        {
            self.first_output_ms = Some(elapsed_ms);
        }
    }

    pub(in crate::gateway) fn finish(&mut self, elapsed_ms: u64) {
        self.finished_ms = Some(elapsed_ms);
    }

    pub(in crate::gateway) fn apply(&self, event: &mut crate::usage::UsageEvent) {
        if self.finished_ms.is_none() && self.first_output_ms.is_none() {
            // A default recorder is also used by transports it does not observe.
            // Do not erase those transports' own timing evidence.
            return;
        }
        if let Some(finished_ms) = self.finished_ms {
            event.latency_ms = finished_ms;
            if let Some(transport) = event
                .routing
                .as_mut()
                .and_then(|routing| routing.transport.as_mut())
            {
                transport.upstream_response_ms = Some(finished_ms);
            }
        }
        // On disconnect or malformed EOF, retain the raw first-output evidence
        // and use the stream settlement's elapsed time for the partial attempt.
        event.ttft_ms = self.first_output_ms;
        event.generation_ms = self
            .first_output_ms
            .map(|first| event.latency_ms.saturating_sub(first));
    }
}

pub(in crate::gateway) fn stop_basis_points_event(
    event: &[u8],
    expected_model: Option<&str>,
) -> bool {
    let event = parse_sse_event(event);
    should_stop_basis_points_event(&event, expected_model)
}

fn should_stop_basis_points_event(event: &TerminalEvent, expected_model: Option<&str>) -> bool {
    event.outcome.is_some()
        || (event.has_data && !event.valid)
        || expected_model.is_some_and(|expected| {
            event.event_payload.as_ref().is_some_and(|payload| {
                super::super::streaming::served_model_is_rejected(payload, expected)
            })
        })
}

pub(in crate::gateway) async fn collect_basis_points_response(
    upstream: reqwest::Response,
    expected_model: Option<&str>,
    started: Instant,
    timing: &mut BufferedResponseTiming,
) -> Result<Vec<u8>, Box<StreamBootstrapFailure>> {
    let bytes = crate::transport::collect_with_progress(
        upstream,
        crate::transport::basis_points_progress_timeout(),
        |bytes| {
            let event = parse_sse_event(bytes);
            timing.observe(&event, started.elapsed().as_millis() as u64);
            should_stop_basis_points_event(&event, expected_model)
        },
    )
    .await;
    // Capture transport completion before JSON validation or client translation,
    // including partial failures. Synthetic client SSE is never timing evidence.
    timing.finish(started.elapsed().as_millis() as u64);
    let bytes = bytes.map_err(|error| {
        let category = if error.timed_out {
            error_codes::STREAM_IDLE_TIMEOUT
        } else {
            error_codes::UPSTREAM_BODY
        };
        Box::new(StreamBootstrapFailure {
            partial_body: error.bytes,
            ..AttemptFailure::stream(category).into()
        })
    })?;
    completed_upstream_response(&bytes, true, expected_model).map_err(|mut failure| {
        failure.partial_body = bytes;
        failure
    })
}

/// Inspect the SSE preamble before collecting a buffered response. Basis Points
/// can stream upstream even though its tool envelope is translated at completion.
pub(in crate::gateway) async fn collect_upstream_response(
    upstream: reqwest::Response,
    account_stream: bool,
    expected_model: Option<&str>,
) -> Result<Vec<u8>, Box<StreamBootstrapFailure>> {
    let is_sse = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|header_value| header_value.to_str().ok())
        .is_some_and(|content_type| {
            content_type
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        });
    let bytes = if is_sse && account_stream {
        let (_, first, mut remaining) =
            super::super::streaming::bootstrap_stream(upstream, expected_model).await?;
        let mut bytes = first.to_vec();
        let mut inspected = 0;
        loop {
            while let Some(end) = sse_event_end(&bytes[inspected..]) {
                let terminal = parse_sse_event(&bytes[inspected..inspected + end]);
                inspected += end;
                let rejected = expected_model.is_some_and(|expected| {
                    terminal
                        .event_payload
                        .as_ref()
                        .is_some_and(|event_payload| {
                            super::super::streaming::served_model_is_rejected(
                                event_payload,
                                expected,
                            )
                        })
                });
                if rejected || terminal.outcome.is_some() || (terminal.has_data && !terminal.valid)
                {
                    // Completion belongs to this response even when the provider
                    // keeps its HTTP connection open. Inspect only through it.
                    return completed_upstream_response(
                        &bytes[..inspected],
                        account_stream,
                        expected_model,
                    )
                    .map_err(|mut failure| {
                        failure.partial_body = bytes[..inspected].to_vec();
                        failure
                    });
                }
            }
            let Some(chunk) = remaining.next().await else {
                break;
            };
            let chunk = chunk.map_err(|error| {
                Box::new(StreamBootstrapFailure {
                    partial_body: bytes.clone(),
                    ..AttemptFailure::transport(&error).into()
                })
            })?;
            bytes.extend_from_slice(&chunk);
        }
        bytes
    } else {
        crate::transport::collect(upstream)
            .await
            .map_err(|_| Box::new(AttemptFailure::stream(error_codes::UPSTREAM_BODY).into()))?
    };
    completed_upstream_response(&bytes, account_stream, expected_model).map_err(|mut failure| {
        failure.partial_body = bytes;
        failure
    })
}

pub(in crate::gateway) fn completed_upstream_response(
    bytes: &[u8],
    account_stream: bool,
    expected_model: Option<&str>,
) -> Result<Vec<u8>, Box<StreamBootstrapFailure>> {
    if let Ok(upstream_json) = serde_json::from_slice::<Value>(bytes) {
        validate_upstream_response(&upstream_json, expected_model)?;
        // Buffered account traffic uses Responses. Generic API traffic is
        // validated against its selected protocol by the request executor.
        if account_stream
            && crate::protocol::completion::ResponseCompletion::for_protocol(
                crate::WireApi::Responses,
                &upstream_json,
            ) == crate::protocol::completion::ResponseCompletion::Invalid
        {
            return Err(invalid_buffered_response());
        }
        return Ok(bytes.to_vec());
    }
    if !account_stream {
        return Ok(bytes.to_vec());
    }
    let mut offset = 0;
    let mut output_items = Vec::new();
    let mut saw_generation = false;
    while let Some(end) = sse_event_end(&bytes[offset..]) {
        let terminal = parse_sse_event(&bytes[offset..offset + end]);
        saw_generation |= terminal.semantic_output
            || terminal
                .event_payload
                .as_ref()
                .is_some_and(super::has_generated_tokens);
        if expected_model.is_some_and(|expected| {
            terminal
                .event_payload
                .as_ref()
                .is_some_and(|event_payload| {
                    super::super::streaming::served_model_is_rejected(event_payload, expected)
                })
        }) {
            let mut failure = super::super::streaming::degraded_route_stream_failure();
            if saw_generation {
                failure.execution = crate::scheduler::rotation::ExecutionObservation::accepted();
            }
            return Err(Box::new(failure));
        }
        if terminal.has_data && !terminal.valid {
            return Err(Box::new(StreamBootstrapFailure {
                upstream_error: terminal.upstream_error,
                ..AttemptFailure::stream(error_codes::STREAM_INVALID).into()
            }));
        }
        if let Some(output_item) = terminal.output_item {
            output_items.push(output_item);
        }
        match terminal.outcome {
            Some(TerminalOutcome::Failure) => {
                let category = terminal
                    .error_category
                    .unwrap_or(error_codes::UPSTREAM_TERMINAL);
                let failure = AttemptFailure::classified_with_hint(
                    terminal
                        .error_status
                        .unwrap_or_else(|| upstream_failure_status(category)),
                    category,
                    terminal.cooldown_hint,
                );
                return Err(Box::new(StreamBootstrapFailure {
                    execution: if saw_generation {
                        crate::scheduler::rotation::ExecutionObservation::accepted()
                    } else {
                        failure.execution
                    },
                    upstream_error: terminal.upstream_error,
                    preserved: terminal.preserved_error,
                    ..failure.into()
                }));
            }
            Some(TerminalOutcome::Success | TerminalOutcome::Incomplete) => {
                if let Some(mut response) = terminal.response {
                    if response
                        .get("output")
                        .and_then(Value::as_array)
                        .is_some_and(Vec::is_empty)
                    {
                        response["output"] = Value::Array(output_items);
                    }
                    if crate::protocol::completion::ResponseCompletion::for_protocol(
                        crate::WireApi::Responses,
                        &response,
                    ) == crate::protocol::completion::ResponseCompletion::Invalid
                    {
                        return Err(invalid_buffered_response());
                    }
                    return serde_json::to_vec(&response).map_err(|_| {
                        Box::new(AttemptFailure::stream(error_codes::STREAM_INVALID).into())
                    });
                }
            }
            None => {}
        }
        offset += end;
    }
    Err(Box::new(
        AttemptFailure::stream(error_codes::STREAM_INCOMPLETE).into(),
    ))
}

/// Check error and served-model evidence for account endpoints with their own schema.
pub(in crate::gateway) fn validate_upstream_response(
    upstream_json: &Value,
    expected_model: Option<&str>,
) -> Result<(), Box<StreamBootstrapFailure>> {
    if !upstream_json.is_object() {
        return Err(invalid_buffered_response());
    }
    let response_payload = upstream_json.get("response").unwrap_or(upstream_json);
    let bytes = serde_json::to_vec(upstream_json).map_err(|_| invalid_buffered_response())?;
    if response_payload
        .get("error")
        .is_some_and(|error| !error.is_null())
        || upstream_json.get("type").and_then(Value::as_str) == Some("error")
        || matches!(
            response_payload.get("status").and_then(Value::as_str),
            Some("failed" | "cancelled" | "canceled")
        )
    {
        let failure = AttemptFailure::status_with_body(StatusCode::BAD_GATEWAY, Some(&bytes));
        return Err(Box::new(StreamBootstrapFailure {
            execution: if has_generation_evidence(upstream_json, None) {
                crate::scheduler::rotation::ExecutionObservation::accepted()
            } else {
                failure.execution
            },
            upstream_error: Some(crate::usage::UpstreamErrorDetails::from_value(
                None,
                upstream_json,
            )),
            preserved: super::super::errors::preserved_upstream_error_value(
                &failure,
                upstream_json,
            ),
            ..failure.into()
        }));
    }
    if expected_model.is_some_and(|expected| {
        super::super::streaming::served_model_is_rejected(upstream_json, expected)
    }) {
        let mut failure = super::super::streaming::degraded_route_stream_failure();
        if has_generation_evidence(upstream_json, None) {
            failure.execution = crate::scheduler::rotation::ExecutionObservation::accepted();
        }
        return Err(Box::new(failure));
    }
    Ok(())
}

fn invalid_buffered_response() -> Box<StreamBootstrapFailure> {
    let mut failure: StreamBootstrapFailure =
        AttemptFailure::stream(error_codes::STREAM_INVALID).into();
    failure.execution = crate::scheduler::rotation::ExecutionObservation::unknown();
    Box::new(failure)
}
