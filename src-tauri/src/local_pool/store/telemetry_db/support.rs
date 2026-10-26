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
    let mut report = json!({
        "schemaVersion": 3,
        "reasoningLatencyComparison": comparison,
        "appVersion": env!("CARGO_PKG_VERSION"),
        "exportedAt": chrono::Utc::now().to_rfc3339(),
        "privacy": "No prompts, tool arguments, provider messages, credentials, URLs or raw identifiers. References are report-local.",
        "summary": summary::summarize(final_request, attempts, truncated),
        "semantics": {
            "attempts": "Physical sends/observations, not separate client requests. Tool discovery and retries are intermediate steps, not a failed final request. HTTP 200 alone does not establish client success.",
            "finalRequest": "Latest retained logical-request projection. Its usage can duplicate one attempt; never add it to attempts. A retained intermediate outcome does not establish final success or failure.",
            "consumption": "Summary sums each retained send once, including a final projection only when its send is missing. Unknown counters remain null. complete describes coverage through the latest observation, not proof of completed execution.",
            "timestamps": "observedAt is the database observation/update time (UTC, second precision), not request start time.",
            "timings": "Milliseconds. Attempt durations and response/tool-ready offsets can share an executor clock, including discovery rounds: do not add them. Response milestones are offsets, not stage durations or client receipt. requestPreparationMs is an elapsed wall clock, not a sum. schemaValidationMs is inside bodyMs. preparationAccounting.unattributedMs is the remainder outside measured non-overlapping phases, including gaps and rounding; it is not another schema-validation duration.",
            "codexServerTiming": "Optional provider-reported native WebSocket measurements in milliseconds, correlated to the active response. Overhead, inference, engine TTFT and inter-token intervals are not additive phases, Relay TTFT, end-user latency or cache-hit evidence. Missing metrics stay absent. ID-less events are accepted only on the first response of a new connection; late or uncorrelated events are ignored.",
            "tools": "policyCatalog describes tools after client policy, before transport encoding. In BPS, catalog schemas are prompt text, so upstreamContext.toolCatalogBytes can be zero. loadedToolCount counts loaded contracts; renderedSchemaBytes counts rendered parameter schemas, not the full prompt or wire payload. upstreamToolCallCount counts native calls before translation; toolCallCount counts client-visible calls. Batch expansion can make them differ without an error.",
            "history": "clientHistory compares current and previous completed client inputs; upstreamHistory compares current and previous completed upstream inputs. relayHistory compares upstream and client inputs within the same request. sharedPrefixItems is the literal positional prefix. leadingContextAlignment, when present, separately compares the remaining items after leading developer/system messages; skipped messages are not proven cache-reusable and the literal prefix is preserved.",
            "cache": "cachedInputTokens and cacheWriteInputTokens are parts of inputTokens; reasoningTokens are part of outputTokens. History comparisons are structural, not proof of provider cache eviction or residency.",
            "reasoningLatency": "protocol.requestedReasoningEffort and effectiveReasoningEffort describe the requested and Relay-mapped setting, not the provider's internal reasoning time. TTFT is an observed first-output milestone, not pure reasoning duration. reasoningLatencyComparison uses bounded retained same-route observations only, not a controlled experiment; it never changes reasoning or sends model requests.",
            "payload": "requestPayload describes the final encoded HTTP body or WebSocket JSON text, after ownership guards. bodyBytes excludes HTTP/WebSocket framing and TLS, and is not an acknowledgement of delivered bytes. Continuation means a nonempty previous_response_id, not proof of delta-only input or provider cache reuse. fullRequestReason describes transport/payload facts, not an inferred cache-miss cause. compressionUs (legacy compressionMs) measures encoding wall time including preparation and blocking-pool scheduling within the executor clock, not an additional requestPreparation phase. httpSendToHeadersMs starts at HTTP execute after encoding; it includes connection setup and upstream waiting, not isolated upload time, and overlaps responseHeadersMs/TTFT. connectionReused describes only the observed native WebSocket connection.",
            "missing": "Null means unavailable, unobserved or excluded by the privacy allowlist, not zero. Current settings and provider usage payloads are not reconstructed from historical observations.",
            "images": "imageUsage contains provider-reported modality subsets, already included in input/output totals. generatedImages counts observed completed images, not partial previews. Missing usage remains unknown; these counters are not an account credit debit. Token-only pricing cannot value mixed image/text inputs and remains unpriced.",
        },
        "availability": {
            "finalRequestMissing": final_request.is_none(),
            "attemptHistoryMissing": attempts.is_empty()
                && final_request.is_none_or(|row| row.attempt > 0),
            "attemptsTruncated": truncated,
            "exportedAttemptRows": attempts.len(),
            "summaryIncludesOmittedAttempts": false,
            "rawProviderUsage": "not_retained_separately",
            "configuration": "not_exported",
        },
        "finalRequest": final_request.map(|row| observation(row, &salt)),
        "attempts": attempts.iter().map(|row| observation(row, &salt)).collect::<Vec<_>>(),
    });
    while serde_json::to_vec_pretty(&report).map_or(usize::MAX, |bytes| bytes.len())
        > MAX_REPORT_BYTES
    {
        let rows = report["attempts"]
            .as_array_mut()
            .expect("report attempts are an array");
        if rows.is_empty() {
            // Preserve the final outcome; omit oversized optional comparisons.
            report["finalRequest"]["routing"] = Value::Null;
            report["availability"]["finalRoutingTruncated"] = json!(true);
            break;
        }
        rows.remove(0);
        let exported_rows = rows.len();
        report["availability"]["attemptsTruncated"] = json!(true);
        report["availability"]["exportedAttemptRows"] = json!(exported_rows);
        report["availability"]["summaryIncludesOmittedAttempts"] = json!(true);
    }
    report
}

fn observation(row: &UsageLog, salt: &str) -> Value {
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
            "transport": routing.transport.as_ref().map(transport::document), "cacheContext": routing.cache_context,
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
        "errorCode": row.error_category.as_deref().and_then(zenith_relay_core::error_codes::definition).map(|definition| definition.code),
        "errorCategoryRef": row.error_category.as_deref().map(reference),
        "protocol": protocol,
        "latencyMs": row.latency_ms, "ttftMs": row.ttft_ms, "generationMs": row.generation_ms,
        "usage": usage, "routing": routing,
        "tools": row.tool_use.as_ref().map(tools::document),
    })
}
