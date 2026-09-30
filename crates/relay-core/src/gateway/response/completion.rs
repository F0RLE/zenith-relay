use super::super::errors::{upstream_failure_status, AttemptFailure};
use super::super::streaming::{parse_sse_event, StreamBootstrapFailure, TerminalOutcome};
use crate::error_codes;
use crate::protocol::sse_event_end;
use axum::http::StatusCode;
use serde_json::Value;
pub(in crate::gateway) fn completed_upstream_response(
    bytes: &[u8],
    account_stream: bool,
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
                execution: failure.execution,
                upstream_error: Some(crate::usage::UpstreamErrorDetails::from_value(None, &value)),
                preserved: super::super::errors::preserved_upstream_error_value(&failure, &value),
                ..failure.into()
            }));
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
                    execution: if saw_output {
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
