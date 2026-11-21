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
            ("intermediate", "Intermediate tool-discovery step.", false)
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
        ) => ("retrying", "Intermediate attempt; Relay continued.", false),
        Some(AttemptOutcome::ExecutionUncertain) => (
            "execution_uncertain",
            "Execution may have started; completion is unknown. Automatic retry is unsafe.",
            true,
        ),
        Some(AttemptOutcome::ClientDisconnected) => (
            "client_disconnected",
            "Client disconnected; provider completion is unknown.",
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
        None => ("unknown", "Terminal outcome unavailable.", false),
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
    let routing = final_request.and_then(|row| row.routing.as_ref());
    let phases = routing.and_then(|routing| routing.request_timing.as_ref()?.phases.as_ref());
    let first_attempt_phases = phases.filter(|phases| {
        phases.prior_attempts_ms.is_some() || final_request.is_some_and(|row| row.attempt <= 1)
    });
    let request_preparation_ms = first_attempt_phases.and_then(|phases| {
        if phases.prior_attempts_ms.is_some() {
            phases.request_preparation_ms
        } else {
            routing?.transport.as_ref()?.request_preparation_ms
        }
    });
    let pre_attempt_unattributed_ms = first_attempt_phases.and_then(|phases| {
        let mut measured = [
            phases.body_read_and_parse_ms,
            phases.bps_access_refresh_ms,
            phases.cache_context_ms,
            phases.candidate_selection_ms,
            request_preparation_ms,
        ]
        .into_iter()
        .flatten();
        let first = measured.next()?;
        let observed = measured.try_fold(first, u64::checked_add)?;
        phases.pre_attempt_ms.checked_sub(observed)
    });
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
            "upstreamTtftMs": phases.and_then(|phases| phases.upstream_ttft_ms),
            "clientFirstOutputMs": phases.and_then(|phases| phases.client_first_output_ms),
            "legacyTtftMs": final_request.filter(|_| phases.is_none()).and_then(|row| row.ttft_ms),
            "preAttemptMs": first_attempt_phases.map(|phases| phases.pre_attempt_ms),
            "priorAttemptsMs": phases.and_then(|phases| phases.prior_attempts_ms),
            "legacyPreFinalAttemptMs": phases.filter(|_| first_attempt_phases.is_none()).map(|phases| phases.pre_attempt_ms),
            "postAttemptMs": phases.and_then(|phases| phases.post_attempt_ms),
            "afterClientReadyMs": phases.and_then(|phases| phases.after_client_ready_ms),
            "bodyReadAndParseMs": phases.and_then(|phases| phases.body_read_and_parse_ms),
            "bpsAccessRefreshMs": phases.and_then(|phases| phases.bps_access_refresh_ms),
            "cacheContextMs": phases.and_then(|phases| phases.cache_context_ms),
            "candidateSelectionMs": first_attempt_phases.and_then(|phases| phases.candidate_selection_ms),
            "requestPreparationMs": request_preparation_ms,
            "preAttemptUnattributedMs": pre_attempt_unattributed_ms,
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
