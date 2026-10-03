use super::super::errors::{upstream_failure_status, AttemptFailure};
use super::super::streaming::{parse_sse_event, StreamBootstrapFailure, TerminalOutcome};
use crate::error_codes;
use crate::protocol::sse_event_end;
use axum::http::StatusCode;
use futures_util::StreamExt;
use serde_json::Value;

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
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        });
    let bytes = if is_sse && account_stream {
        let (_, first, mut remaining) =
            super::super::streaming::bootstrap_stream(upstream, expected_model)
                .await
                .map_err(Box::new)?;
        let mut bytes = first.to_vec();
        let mut inspected = 0;
        loop {
            while let Some(end) = sse_event_end(&bytes[inspected..]) {
                let terminal = parse_sse_event(&bytes[inspected..inspected + end]);
                inspected += end;
                let rejected = expected_model.is_some_and(|expected| {
                    terminal.payload.as_ref().is_some_and(|value| {
                        super::super::streaming::served_model_is_rejected(value, expected)
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
                    );
                }
            }
            let Some(chunk) = remaining.next().await else {
                break;
            };
            let chunk =
                chunk.map_err(|error| Box::new(AttemptFailure::transport(&error).into()))?;
            if bytes.len().saturating_add(chunk.len()) > crate::runtime::MAX_NON_STREAM_BODY_BYTES {
                return Err(Box::new(
                    AttemptFailure::stream(error_codes::UPSTREAM_BODY_TOO_LARGE).into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        bytes
    } else {
        crate::transport::collect_limited(upstream, crate::runtime::MAX_NON_STREAM_BODY_BYTES)
            .await
            .map_err(|error| {
                Box::new(
                    AttemptFailure::stream(
                        if matches!(error, crate::Error::UpstreamBodyTooLarge) {
                            error_codes::UPSTREAM_BODY_TOO_LARGE
                        } else {
                            error_codes::UPSTREAM_BODY
                        },
                    )
                    .into(),
                )
            })?
    };
    completed_upstream_response(&bytes, account_stream, expected_model)
}

pub(in crate::gateway) fn completed_upstream_response(
    bytes: &[u8],
    account_stream: bool,
    expected_model: Option<&str>,
) -> Result<Vec<u8>, Box<StreamBootstrapFailure>> {
    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        let response = value.get("response").unwrap_or(&value);
        if response.get("error").is_some_and(|error| !error.is_null())
            || value.get("type").and_then(Value::as_str) == Some("error")
            || matches!(
                response.get("status").and_then(Value::as_str),
                Some("failed" | "cancelled" | "canceled")
            )
        {
            let failure = AttemptFailure::status_with_body(StatusCode::BAD_GATEWAY, Some(bytes));
            return Err(Box::new(StreamBootstrapFailure {
                execution: if super::super::streaming::has_semantic_output(&value, None) {
                    crate::scheduler::rotation::ExecutionObservation::accepted()
                } else {
                    failure.execution
                },
                upstream_error: Some(crate::usage::UpstreamErrorDetails::from_value(None, &value)),
                preserved: super::super::errors::preserved_upstream_error_value(&failure, &value),
                ..failure.into()
            }));
        }
        if expected_model.is_some_and(|expected| {
            super::super::streaming::served_model_is_rejected(&value, expected)
        }) {
            let mut failure = super::super::streaming::degraded_route_stream_failure();
            if super::super::streaming::has_semantic_output(&value, None) {
                failure.execution = crate::scheduler::rotation::ExecutionObservation::accepted();
            }
            return Err(Box::new(failure));
        }
        return Ok(bytes.to_vec());
    }
    if !account_stream {
        return Ok(bytes.to_vec());
    }
    let mut offset = 0;
    let mut output = Vec::new();
    let mut saw_output = false;
    while let Some(end) = sse_event_end(&bytes[offset..]) {
        let terminal = parse_sse_event(&bytes[offset..offset + end]);
        if expected_model.is_some_and(|expected| {
            terminal.payload.as_ref().is_some_and(|value| {
                super::super::streaming::served_model_is_rejected(value, expected)
            })
        }) {
            let mut failure = super::super::streaming::degraded_route_stream_failure();
            if saw_output || terminal.semantic_output {
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
        if let Some(item) = terminal.output_item {
            output.push(item);
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
                    execution: if saw_output || terminal.semantic_output {
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
                        response["output"] = Value::Array(output);
                    }
                    return serde_json::to_vec(&response).map_err(|_| {
                        Box::new(AttemptFailure::stream(error_codes::STREAM_INVALID).into())
                    });
                }
            }
            None => {}
        }
        saw_output |= terminal.semantic_output;
        offset += end;
    }
    Err(Box::new(
        AttemptFailure::stream(error_codes::STREAM_INCOMPLETE).into(),
    ))
}
