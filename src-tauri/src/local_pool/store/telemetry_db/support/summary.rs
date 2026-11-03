use super::UsageLog;
use serde_json::{Value, json};
use zenith_relay_core::AttemptOutcome;

fn classification(row: &UsageLog) -> (&'static str, &'static str, bool) {
    match row
        .routing
        .as_ref()
        .and_then(|routing| routing.attempt_outcome)
    {
        Some(AttemptOutcome::ToolDiscovery)
            if !row.success
                && (200..300).contains(&row.http_status)
                && row.error_category.is_none()
                && row.error_origin.is_none()
                && row.upstream_error.is_none() =>
        {
            (
                "intermediate",
                "Relay loaded tool definitions; this is not a failed client request.",
                false,
            )
        }
        Some(AttemptOutcome::ToolDiscovery) => (
            "inconsistent",
            "The tool-discovery stage conflicts with success/error evidence; inspect this observation.",
            false,
        ),
        Some(
            AttemptOutcome::RetrySelection
            | AttemptOutcome::RetryRepair
            | AttemptOutcome::RetryAuthorization,
        ) => (
            "retrying",
            "Relay continued with another attempt; this step is not the final request outcome.",
            false,
        ),
        Some(AttemptOutcome::ExecutionUncertain) => (
            "execution_uncertain",
            "Execution may have started; completion is unknown. Do not infer that retrying is safe.",
            true,
        ),
        Some(AttemptOutcome::ClientDisconnected) => (
            "client_disconnected",
            "The client disconnected; this does not establish whether provider execution completed.",
            true,
        ),
        Some(AttemptOutcome::ReturnedFailure) => {
            ("failed", "Relay returned a failure for this request.", true)
        }
        _ if row.success => (
            "succeeded",
            "Relay completed the request successfully.",
            true,
        ),
        Some(AttemptOutcome::Completed) => (
            "inconsistent",
            "A completed outcome has a false success flag; inspect the retained evidence.",
            false,
        ),
        None if row.http_status >= 400 || row.error_category.is_some() => (
            "failed",
            "The retained observation records a failure.",
            true,
        ),
        None => (
            "unknown",
            "No terminal outcome was retained; a false success flag alone does not establish failure.",
            false,
        ),
    }
}

pub(super) fn outcome(row: &UsageLog) -> Value {
    let (status, description, terminal) = classification(row);
    json!({
        "status": status, "description": description, "terminal": terminal,
        "stage": row.routing.as_ref().and_then(|routing| routing.attempt_outcome),
        "recordedSuccessFlag": row.success,
    })
}

pub(super) fn summarize(
    final_request: Option<&UsageLog>,
    attempts: &[UsageLog],
    truncated: bool,
) -> Value {
    let mut sends = attempts.iter().collect::<Vec<_>>();
    let final_send_missing = final_request.is_some_and(|last| {
        last.attempt > 0 && !sends.iter().any(|row| row.attempt == last.attempt)
    });
    if let Some(last) = final_request.filter(|_| final_send_missing) {
        sends.push(last);
    }
    sends.sort_by_key(|row| row.attempt);
    sends.dedup_by_key(|row| row.attempt);
    let sequence_complete = !truncated
        && final_request.is_some_and(|last| {
            (last.attempt == 0 || !attempts.is_empty())
                && sends.len() == usize::from(last.attempt)
                && sends
                    .iter()
                    .enumerate()
                    .all(|(index, row)| usize::from(row.attempt) == index + 1)
        });
    let count = |status| {
        sends
            .iter()
            .filter(|row| classification(row).0 == status)
            .count()
    };
    let counter = |read: fn(&UsageLog) -> Option<u64>| {
        let mut sum = Some(0_u64);
        let mut observed = 0_usize;
        for row in &sends {
            if let Some(value) = read(row) {
                observed += 1;
                sum = sum.and_then(|sum| sum.checked_add(value));
            }
        }
        json!({
            "observedTotal": (observed > 0).then_some(sum).flatten(),
            "observedSends": observed, "includedSends": sends.len(),
            "complete": sequence_complete && observed == sends.len() && sum.is_some(),
            "overflow": sum.is_none(),
        })
    };
    json!({
        "outcome": final_request.map(outcome),
        "scope": "retained_sends_before_export_detail_size_limit",
        "logicalRequests": 1, "retainedAttemptRows": attempts.len(), "includedSends": sends.len(),
        "finalSendAddedFromProjection": final_send_missing,
        "attemptSequenceCompleteThroughLatest": sequence_complete,
        "steps": { "completed": count("succeeded"), "toolDiscovery": count("intermediate"),
            "retrying": count("retrying"), "returnedFailures": count("failed"),
            "executionUncertain": count("execution_uncertain"), "clientDisconnected": count("client_disconnected"),
            "unknown": count("unknown"), "inconsistent": count("inconsistent") },
        "reportedTiming": {
            "source": "latest_request_projection_not_sum_of_attempts",
            "durationMs": final_request.map(|row| row.latency_ms),
            "ttftMs": final_request.and_then(|row| row.ttft_ms),
            "generationMs": final_request.and_then(|row| row.generation_ms),
        },
        "consumption": {
            "inputTokens": counter(|row| row.input_tokens),
            "cachedInputTokens": counter(|row| row.cached_input_tokens),
            "cacheWriteInputTokens": counter(|row| row.cache_write_input_tokens),
            "reasoningTokens": counter(|row| row.reasoning_tokens),
            "outputTokens": counter(|row| row.output_tokens),
            "totalTokens": counter(|row| row.total_tokens),
        },
    })
}
