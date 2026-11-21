//! Support evidence is a new allowlisted projection, never a dump of stored JSON.
//! Unknown fields, provider messages, names, URLs and payloads cannot enter it.
use super::UsageLog;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MAX_REPORT_BYTES: usize = 256 * 1024;
pub(super) mod reasoning;
mod summary;
mod tools;
mod transport;

#[cfg(test)]
pub(super) fn document(
    final_request: Option<&UsageLog>,
    attempts: &[UsageLog],
    truncated: bool,
) -> Value {
    document_with_comparison(final_request, attempts, truncated, None)
}

pub(super) fn document_with_comparison(
    final_request: Option<&UsageLog>,
    attempts: &[UsageLog],
    truncated: bool,
    comparison: Option<Value>,
) -> Value {
    let salt = uuid::Uuid::new_v4().to_string();
    let summary = summary::summarize(final_request, attempts, truncated);
    let mut sends = attempts
        .iter()
        .map(|row| observation(row, &salt))
        .collect::<Vec<_>>();
    let mut local_observation = None;
    let final_request = final_request.map(|row| {
        let observed = observation(row, &salt);
        let latest_attempt = (row.attempt > 0).then_some(row.attempt);
        let usage = summary["consumption"]
            .as_object()
            .expect("consumption object")
            .iter()
            .map(|(key, counter)| (key.clone(), counter["observedTotal"].clone()))
            .collect::<serde_json::Map<_, _>>();
        let final_row = json!({
            "requestRef": observed["requestRef"],
            "outcome": observed["outcome"],
            "httpStatus": observed["httpStatus"],
            "errorOrigin": observed["errorOrigin"],
            "errorCode": observed["errorCode"],
            "errorCategoryRef": observed["errorCategoryRef"],
            "requestOrigin": observed["requestOrigin"],
            "latestAttempt": latest_attempt,
            "latencyMs": row.latency_ms,
            "generationMs": row.generation_ms,
            "usage": usage,
        });
        if let Some(number) = latest_attempt {
            if let Some(attempt) = sends
                .iter_mut()
                .find(|attempt| attempt["attempt"] == number)
            {
                let mut detail = projection_details(observed);
                detail.retain(|key, value| attempt.get(key.as_str()) != Some(value));
                if !detail.is_empty() {
                    attempt["finalObservation"] = Value::Object(detail);
                }
            } else {
                // Retention and older databases may leave only the projection.
                let mut attempt = observed;
                attempt["fromFinalProjection"] = json!(true);
                sends.push(attempt);
                sends.sort_by_key(|attempt| attempt["attempt"].as_u64());
            }
        } else {
            local_observation = Some(Value::Object(projection_details(observed)));
        }
        final_row
    });
    let retained_attempt_count = sends.len();
    let mut report = json!({
        "schemaVersion": 4,
        "reasoningLatencyComparison": comparison,
        "appVersion": env!("CARGO_PKG_VERSION"),
        "exportedAt": chrono::Utc::now().to_rfc3339(),
        "privacy": "No prompts, tool arguments, provider messages, credentials, URLs or raw identifiers. References are report-local.",
        "summary": summary,
        "semantics": {
            "attempts": "Physical sends, including discovery and retries. If the send row is missing, fromFinalProjection marks reconstructed evidence; its timing is logical. finalObservation stores only differing detail fields from the final projection.",
            "finalRequest": "Logical outcome; latestAttempt references an exported attempt number. Usage sums observed physical sends; coverage is in summary.consumption. localObservation holds details of a request rejected without a send.",
            "consumption": "Each retained send is counted once; the final projection is used only if its send is missing. Unknown counters stay null. complete measures observation coverage.",
            "timestamps": "observedAt is the database update time in UTC, with second precision.",
            "timings": "Milliseconds. preAttemptMs ends at the first observed executor start. priorAttemptsMs is wall time from the first to the latest executor start, including gaps; discovery rounds can share a clock. Older retry records have only legacyPreFinalAttemptMs. preAttemptUnattributedMs excludes measured first-attempt preparation. bpsAccessRefreshMs measures the ingress access check, not token renewal alone. SIWC handshakeMs and sessionWaitMs are within the attempt clock. postAttemptMs starts at the latest attempt's latency milestone; afterClientReadyMs starts at readiness, not receipt. Output excludes lifecycle events. schemaValidationMs is within bodyMs.",
            "codexServerTiming": "Provider-reported WebSocket metrics for the active response are not additive. A missing event is not observed on WebSocket and unsupported on HTTP. Late or uncorrelated events cannot be used.",
            "sessionWait": "sessionWaitMs measures local SIWC lane acquisition before dispatch, separately from provider TTFT; do not add overlapping clocks. session_wait_timeout means the retained lane stayed busy; session_capacity_exceeded means no retained slot was available. Historical session_unavailable combines these causes.",
            "tools": "clientWebSearchDeclared checks the incoming Responses request; requestPayload.webSearchDeclared checks the written SIWC frame, not inherited history or tool execution. policyCatalog is after client policy and before encoding. BPS schemas are prompt text, so upstreamContext.toolCatalogBytes can be zero. loadedToolCount counts contracts; renderedSchemaBytes counts parameter schemas only. upstreamToolCallCount counts native calls; toolCallCount counts translated client calls, including batch expansion.",
            "history": "clientHistory and upstreamHistory compare with the prior completed request. relayHistory compares the two sides of one request. leadingContextAlignment skips leading developer/system messages and proven Relay catalog wrappers; the literal prefix stays separate.",
            "cache": "cachedInputTokens and cacheWriteInputTokens are subsets of inputTokens; reasoningTokens are a subset of outputTokens. History alignment measures structure; cache usage comes from provider counters.",
            "providerBackoff": "Observed provider deadline and scope, with Unix-millisecond times and report-local model references. Missing evidence stays null. Runtime readiness, credential ownership and admission are evaluated separately.",
            "reasoningLatency": "protocol.requestedReasoningEffort and effectiveReasoningEffort are requested and Relay-mapped settings. TTFT measures first output. reasoningLatencyComparison summarizes retained same-route observations; provider-internal reasoning duration is unknown.",
            "payload": "jsonBytes measures JSON; bodyBytes measures encoded payload, excluding framing/TLS, and is not an acknowledgement. socketOutputReconciled marks allowlisted replay normalization. authorizationMs covers credential/access preparation; httpDispatchPreparationMs covers subsequent setup, including compressionUs (legacy compressionMs) and blocking-pool scheduling. httpSendToHeadersMs starts at HTTP execute; all three phases overlap responseHeadersMs/TTFT. connectionReused is native WebSocket only.",
            "missing": "Null means unavailable, unobserved or privacy-excluded; never zero. Historical observations cannot reconstruct current settings or provider payloads.",
            "imageDiagnostics": "Opt-in routing.images separates requested, sent and provider-reported options. Only known public model names and option values are retained. Dimensions use bounded raster headers; sentSizeMatches compares exact sent dimensions. Preview frames are excluded and URLs are never fetched. Serving-model identity remains unverified; resizing and replay require separate authorization.",
            "images": "imageUsage records rendering modality subsets, cachedInputTokens and separate svgUsage authoring counters. Rendering/SVG counters enter totals once when reported; missing components and absent SVG cache coverage stay unknown. generatedImages counts completed images. Actual account debits are tracked separately. Mixed image/text inputs and multi-model requests remain unpriced without applicable rates.",
        },
        "availability": {
            "finalRequestMissing": final_request.is_none(),
            "attemptHistoryMissing": attempts.is_empty()
                && final_request.as_ref().is_none_or(|row| !row["latestAttempt"].is_null()),
            "attemptsTruncated": truncated,
            "exportedAttemptRows": retained_attempt_count,
            "summaryIncludesOmittedAttempts": false,
            "rawProviderUsage": "not_retained_separately",
            "configuration": "not_exported",
        },
        "finalRequest": final_request,
        "localObservation": local_observation,
        "attempts": sends,
    });
    while serde_json::to_vec_pretty(&report).map_or(usize::MAX, |bytes| bytes.len())
        > MAX_REPORT_BYTES
    {
        let rows = report["attempts"]
            .as_array_mut()
            .expect("report attempts are an array");
        if !rows.is_empty() {
            let omitted = rows.remove(0);
            let exported_rows = rows.len();
            if report["finalRequest"]["latestAttempt"].is_number()
                && omitted["attempt"] == report["finalRequest"]["latestAttempt"]
            {
                report["availability"]["omittedLatestAttempt"] = omitted["attempt"].clone();
                report["finalRequest"]["latestAttempt"] = Value::Null;
            }
            report["availability"]["attemptsTruncated"] = json!(true);
            report["availability"]["exportedAttemptRows"] = json!(exported_rows);
            report["availability"]["summaryIncludesOmittedAttempts"] = json!(true);
        } else if !report["localObservation"].is_null() {
            report["localObservation"] = Value::Null;
            report["availability"]["localObservationTruncated"] = json!(true);
        } else {
            report["reasoningLatencyComparison"] = Value::Null;
            break;
        }
    }
    report
}

fn projection_details(observed: Value) -> serde_json::Map<String, Value> {
    let Value::Object(mut detail) = observed else {
        unreachable!("observation object");
    };
    for key in [
        "requestRef",
        "attempt",
        "outcome",
        "httpStatus",
        "errorOrigin",
        "errorCode",
        "errorCategoryRef",
        "requestOrigin",
        "latencyMs",
        "ttftMs",
        "generationMs",
    ] {
        detail.remove(key);
    }
    detail
}

fn observation(row: &UsageLog, salt: &str) -> Value {
    let request_origin = row
        .success
        .then_some(row.error_category.as_deref())
        .flatten()
        .and_then(|category| match category {
            "codex_memory_extraction" => Some("memory_extraction"),
            "codex_memory_consolidation" => Some("memory_consolidation"),
            "codex_task_title" => Some("task_title"),
            "codex_activity_summary" => Some("activity_summary"),
            "codex_wake" => Some("wake"),
            _ => None,
        });
    let error_category = row
        .error_category
        .as_deref()
        .filter(|_| request_origin.is_none());
    let reference = |value: &str| {
        let digest = Sha256::digest(format!("{salt}:{value}").as_bytes());
        format!("ref_{}", &hex::encode(digest)[..16])
    };
    let protocol = json!({
        "wireApi": (["responses", "chat_completions", "messages", "gemini", "gemini_interactions"].contains(&row.wire_api.as_str()).then_some(row.wire_api.as_str())),
        "clientTransport": (["http", "websocket"].contains(&row.transport.as_str()).then_some(row.transport.as_str())),
        "requestedServiceTier": row.service_tier,
        "appliedServiceTier": row.applied_service_tier.as_deref().filter(|tier| ["auto", "default", "flex", "priority", "scale", "standard"].contains(tier)),
        "appliedServiceTierRef": row.applied_service_tier.as_deref().map(reference),
        "requestedReasoningEffort": row.requested_reasoning_effort.as_deref().and_then(zenith_relay_core::normalize_reasoning_effort),
        "effectiveReasoningEffort": row.effective_reasoning_effort.as_deref().and_then(zenith_relay_core::normalize_reasoning_effort),
    });
    let usage = json!({
        "inputTokens": row.input_tokens, "cachedInputTokens": row.cached_input_tokens,
        "cacheWriteInputTokens": row.cache_write_input_tokens, "reasoningTokens": row.reasoning_tokens,
        "outputTokens": row.output_tokens, "totalTokens": row.total_tokens,
        "imageUsage": row.image_usage,
        "cacheWriteTtl": row.cache_write_ttl.as_deref().and_then(zenith_relay_core::usage::normalize_reported_cache_ttls),
    });
    let routing = row.routing.as_ref().map(|routing| {
        json!({
            "reason": routing.reason, "eligibleCandidates": routing.eligible_candidates,
            "inFlightBefore": routing.in_flight_before, "attemptOutcome": routing.attempt_outcome,
            "transport": routing.transport.as_ref().map(transport::document),
            "cacheContext": routing.cache_context.as_ref().map(transport::cache_context),
            "cacheAffinity": routing.cache_affinity,
            "images": routing.images.as_ref().map(zenith_relay_core::usage::ImageDiagnostics::sanitized),
            "providerBackoff": routing.provider_backoff.as_ref().map(|backoff| json!({
                "scope": if backoff.scope == "*" { "candidate" } else { "model" },
                "modelRef": (backoff.scope != "*").then(|| reference(&backoff.scope)),
                "notBeforeMs": backoff.not_before_ms,
                "observedAtMs": backoff.observed_at_ms,
            })),
        })
    });
    json!({
        "requestRef": reference(&row.request_id),
        "attempt": row.attempt,
        "observedAt": chrono::DateTime::parse_from_rfc3339(&row.created_at).ok().map(|time| time.to_utc().to_rfc3339()),
        "atMs": chrono::DateTime::parse_from_rfc3339(&row.created_at).ok().map(|time| time.timestamp_millis()),
        "sourceRef": reference(&row.source_id),
        "candidateRef": row.candidate_id.as_deref().map(reference),
        "accountRef": row.account_id.as_deref().map(reference),
        "requestedModelRef": row.requested_model.as_deref().map(reference),
        "resolvedModelRef": row.resolved_model.as_deref().map(reference),
        "outcome": summary::outcome(row),
        "httpStatus": row.http_status, "errorOrigin": row.error_origin,
        "errorCode": error_category.and_then(zenith_relay_core::error_codes::definition).map(|definition| definition.code),
        "errorCategoryRef": error_category.map(reference),
        "requestOrigin": request_origin,
        "protocol": protocol,
        "latencyMs": row.latency_ms, "ttftMs": row.ttft_ms, "generationMs": row.generation_ms,
        "usage": usage, "routing": routing,
        "tools": row.tool_use.as_ref().map(tools::document),
    })
}
