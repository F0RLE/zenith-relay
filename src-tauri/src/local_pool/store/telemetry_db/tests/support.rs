use super::*;

mod metrics;
mod reasoning;

#[test]
fn diagnostic_report_keeps_attempts_and_unknowns_without_private_identifiers() {
    let root = std::env::temp_dir().join(format!("relay-support-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let db = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut event = aggregate_test_event("PRIVATE_REQUEST", 1, 0, 0, None, 0);
    event.source_id = "PRIVATE_SOURCE".into();
    event.requested_model = Some("PRIVATE_MODEL".into());
    event.error_category = Some("PRIVATE_ERROR_MESSAGE".into());
    event.cached_input_tokens = None;
    db.record(&event).unwrap();
    event.attempt = 2;
    event.cached_input_tokens = Some(0);
    db.record(&event).unwrap();
    let report = db.request_diagnostics("PRIVATE_REQUEST").unwrap();
    let text = serde_json::to_string(&report).unwrap();
    assert!(!text.contains("PRIVATE_"));
    assert_eq!(report["schemaVersion"], 3);
    assert_eq!(report["attempts"].as_array().unwrap().len(), 2);
    assert!(report["attempts"][0]["usage"]["cachedInputTokens"].is_null());
    assert_eq!(report["attempts"][1]["usage"]["cachedInputTokens"], 0);
    assert_eq!(report["finalRequest"]["attempt"], 2);
    assert_eq!(
        report["attempts"][0]["sourceRef"],
        report["attempts"][1]["sourceRef"]
    );
    let next_report = db.request_diagnostics("PRIVATE_REQUEST").unwrap();
    assert_ne!(
        report["finalRequest"]["sourceRef"],
        next_report["finalRequest"]["sourceRef"]
    );
    assert!(db.request_diagnostics("missing").is_err());
    for attempt in 3..=70 {
        event.attempt = attempt;
        db.record(&event).unwrap();
    }
    let bounded = db.request_diagnostics("PRIVATE_REQUEST").unwrap();
    assert_eq!(bounded["availability"]["attemptsTruncated"], true);
    assert_eq!(bounded["attempts"].as_array().unwrap().len(), 64);
    assert_eq!(bounded["attempts"][0]["attempt"], 7);
    assert_eq!(bounded["finalRequest"]["attempt"], 70);
    assert!(serde_json::to_vec_pretty(&bounded).unwrap().len() <= 256 * 1024);

    let mut row = db.list(1).unwrap().pop().unwrap();
    row.routing = Some(
        serde_json::from_value(serde_json::json!({
            "reason": "manual_priority", "eligibleCandidates": 1,
            "inFlightBefore": 0, "dispatchesBefore": 0,
            "endpointKind": "PRIVATE_ENDPOINT", "upstreamResponseModel": "PRIVATE_MODEL",
            "cacheContext": {
                "baseline": "first_observation", "scope": "unavailable",
                "clientChanges": vec!["tools"; 40_000], "upstreamChanges": [], "relayChanges": [],
                "clientHistory": {"comparison": "not_compared"},
                "upstreamHistory": {"comparison": "not_compared"},
                "relayHistory": {"comparison": "not_compared"}
            }
        }))
        .unwrap(),
    );
    let oversized = super::super::support::document(Some(&row), std::slice::from_ref(&row), false);
    let bytes = serde_json::to_vec_pretty(&oversized).unwrap();
    assert!(bytes.len() <= 256 * 1024);
    assert!(!String::from_utf8(bytes).unwrap().contains("PRIVATE_"));
    assert_eq!(oversized["finalRequest"]["attempt"], 70);
    assert_eq!(oversized["availability"]["finalRoutingTruncated"], true);
    assert_eq!(oversized["availability"]["attemptsTruncated"], true);
    assert_eq!(oversized["availability"]["exportedAttemptRows"], 0);
    assert_eq!(
        oversized["availability"]["summaryIncludesOmittedAttempts"],
        true
    );
    assert_eq!(oversized["summary"]["includedSends"], 1);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_backend_timing_survives_storage_and_export_without_raw_identifiers() {
    let root = std::env::temp_dir().join(format!("relay-backend-timing-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let db = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut event = aggregate_test_event("synthetic-timing", 1, 0, 0, None, 0);
    event.routing = Some(
        serde_json::from_value(serde_json::json!({
            "reason": "only_eligible", "eligibleCandidates": 1,
            "inFlightBefore": 0, "dispatchesBefore": 0,
            "transport": {"upstream": "websocket", "mode": "native",
                "connectionReused": false,
                "codexServerTiming": {"overheadMs": 0, "inferenceMs": 12.75,
                    "responseId": "PRIVATE_RESPONSE", "extension": "PRIVATE_EXTENSION"}}
        }))
        .unwrap(),
    );
    db.record(&event).unwrap();
    let report = db.request_diagnostics("synthetic-timing").unwrap();
    let timing = &report["finalRequest"]["routing"]["transport"]["codexServerTiming"];
    assert_eq!(
        timing,
        &serde_json::json!({"overheadMs": 0, "inferenceMs": 12.75})
    );
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE_"));
    assert!(
        report["semantics"]["codexServerTiming"]
            .as_str()
            .unwrap()
            .contains("not additive")
    );

    event.request_id = "synthetic-no-timing".into();
    event
        .routing
        .as_mut()
        .unwrap()
        .transport
        .as_mut()
        .unwrap()
        .codex_server_timing = None;
    db.record(&event).unwrap();
    let report = db.request_diagnostics("synthetic-no-timing").unwrap();
    assert!(
        report["finalRequest"]["routing"]["transport"]
            .get("codexServerTiming")
            .is_none()
    );
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

fn diagnostic_rows() -> Vec<UsageLog> {
    let root = std::env::temp_dir().join(format!("relay-diagnostic-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let db = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut event = aggregate_test_event("synthetic_request", 1, 100, 10, None, 20);
    event.success = false;
    event.latency_ms = 200;
    event.routing = Some(
        serde_json::from_value(serde_json::json!({
            "reason": "manual_priority", "eligibleCandidates": 1,
            "inFlightBefore": 0, "dispatchesBefore": 0, "attemptOutcome": "tool_discovery"
        }))
        .unwrap(),
    );
    db.record(&event).unwrap();
    let first = db.list(1).unwrap().pop().unwrap();
    event.attempt = 2;
    event.success = true;
    event.latency_ms = 500;
    event.input_tokens = Some(300);
    event.total_tokens = Some(320);
    event.cached_input_tokens = Some(280);
    event.routing.as_mut().unwrap().attempt_outcome =
        Some(zenith_relay_core::AttemptOutcome::Completed);
    db.record(&event).unwrap();
    let last = db.list(1).unwrap().pop().unwrap();
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
    vec![first, last]
}

#[test]
fn diagnostic_report_distinguishes_discovery_from_failure_and_counts_every_send_once() {
    let rows = diagnostic_rows();
    let report = super::super::support::document(rows.last(), &rows, false);
    assert_eq!(report["attempts"][0]["outcome"]["status"], "intermediate");
    assert_eq!(report["attempts"][0]["outcome"]["terminal"], false);
    assert_eq!(report["summary"]["outcome"]["status"], "succeeded");
    assert_eq!(report["summary"]["steps"]["toolDiscovery"], 1);
    assert_eq!(report["summary"]["steps"]["returnedFailures"], 0);
    assert_eq!(
        report["summary"]["consumption"]["inputTokens"]["observedTotal"],
        400
    );
    assert_eq!(
        report["summary"]["consumption"]["inputTokens"]["complete"],
        true
    );
    assert_eq!(
        report["summary"]["consumption"]["cachedInputTokens"]["observedTotal"],
        280
    );
    assert_eq!(
        report["summary"]["consumption"]["cachedInputTokens"]["complete"],
        false
    );
    assert_eq!(
        report["summary"]["consumption"]["cachedInputTokens"]["observedSends"],
        1
    );
    assert_eq!(report["summary"]["reportedTiming"]["durationMs"], 500);
    assert_eq!(report["summary"]["finalSendAddedFromProjection"], false);
    assert!(
        report["attempts"][0]["observedAt"]
            .as_str()
            .unwrap()
            .ends_with("+00:00")
    );
    assert_eq!(report["attempts"][1]["protocol"]["wireApi"], "responses");
}

#[test]
fn diagnostic_report_marks_partial_history_and_never_invents_unknown_tokens() {
    let mut rows = diagnostic_rows();
    rows[1].reasoning_tokens = None;
    let report = super::super::support::document(rows.last(), &rows[1..], true);
    assert_eq!(
        report["summary"]["attemptSequenceCompleteThroughLatest"],
        false
    );
    assert_eq!(
        report["summary"]["consumption"]["inputTokens"]["observedTotal"],
        300
    );
    assert_eq!(
        report["summary"]["consumption"]["inputTokens"]["complete"],
        false
    );
    assert!(report["summary"]["consumption"]["reasoningTokens"]["observedTotal"].is_null());
    let legacy = super::super::support::document(rows.last(), &[], false);
    assert_eq!(legacy["summary"]["includedSends"], 1);
    assert_eq!(legacy["summary"]["finalSendAddedFromProjection"], true);
    assert_eq!(
        legacy["summary"]["consumption"]["inputTokens"]["observedTotal"],
        300
    );
    assert_eq!(
        legacy["summary"]["consumption"]["inputTokens"]["complete"],
        false
    );
    let missing = super::super::support::document(None, &rows[..1], false);
    assert!(missing["summary"]["outcome"].is_null());
    assert!(missing["summary"]["reportedTiming"]["durationMs"].is_null());
    rows[0].input_tokens = Some(u64::MAX);
    let overflow = super::super::support::document(rows.last(), &rows, false);
    assert!(overflow["summary"]["consumption"]["inputTokens"]["observedTotal"].is_null());
    assert_eq!(
        overflow["summary"]["consumption"]["inputTokens"]["overflow"],
        true
    );
}

#[test]
fn diagnostic_report_exports_known_error_codes_but_not_arbitrary_provider_text() {
    let mut row = diagnostic_rows().pop().unwrap();
    row.success = false;
    row.http_status = 502;
    row.error_category = Some("adapter_upstream_response_invalid".into());
    row.routing.as_mut().unwrap().attempt_outcome =
        Some(zenith_relay_core::AttemptOutcome::ReturnedFailure);
    let report = super::super::support::document(Some(&row), std::slice::from_ref(&row), false);
    assert_eq!(report["finalRequest"]["outcome"]["status"], "failed");
    assert_eq!(
        report["finalRequest"]["errorCode"],
        "adapter_upstream_response_invalid"
    );
    row.error_category = Some("PRIVATE_PROVIDER_TEXT".into());
    row.wire_api = "PRIVATE_PROTOCOL".into();
    row.transport = "PRIVATE_TRANSPORT".into();
    row.requested_reasoning_effort = Some("PRIVATE_REASONING".into());
    row.cache_write_ttl = Some("PRIVATE_TTL".into());
    row.applied_service_tier = Some("PRIVATE_TIER".repeat(40_000));
    row.upstream_error = Some(serde_json::from_value(serde_json::json!({
        "httpStatus": 200, "code": "PRIVATE_CODE", "errorType": "PRIVATE_TYPE",
        "message": "PRIVATE_MESSAGE", "requestId": "PRIVATE_ID", "redacted": false, "truncated": false
    })).unwrap());
    let private = super::super::support::document(Some(&row), std::slice::from_ref(&row), false);
    assert!(
        !serde_json::to_string(&private)
            .unwrap()
            .contains("PRIVATE_")
    );
    assert!(private["finalRequest"]["errorCode"].is_null());
    assert!(private["finalRequest"]["protocol"]["appliedServiceTier"].is_null());
    assert!(private["finalRequest"]["protocol"]["appliedServiceTierRef"].is_string());
    assert!(serde_json::to_vec_pretty(&private).unwrap().len() <= 256 * 1024);
    row.routing.as_mut().unwrap().attempt_outcome =
        Some(zenith_relay_core::AttemptOutcome::ToolDiscovery);
    let conflict = super::super::support::document(Some(&row), &[], false);
    assert_eq!(
        conflict["finalRequest"]["outcome"]["status"],
        "inconsistent"
    );
}

#[test]
fn diagnostic_report_preserves_context_and_distinguishes_native_from_client_tool_calls() {
    let mut row = diagnostic_rows().pop().unwrap();
    row.tool_use = Some(serde_json::from_value(serde_json::json!({
        "clientToolCount": 4, "forwardedToolCount": 4, "toolCallCount": 2,
        "upstreamToolCallCount": 1, "toolChoice": "auto", "textOutput": false,
        "terminalOutput": "tool_call", "requestedParallelToolCalls": true,
        "clientContext": { "bodyBytes": 1000, "instructionBytes": 100, "conversationBytes": 300, "toolCatalogBytes": 500 },
        "upstreamContext": { "bodyBytes": 1200, "instructionBytes": 800, "conversationBytes": 300, "toolCatalogBytes": 0 },
        "basisPointsCatalog": { "mode": "discovery", "fullSchemaBytes": 500, "renderedSchemaBytes": 200,
            "catalogPromptBytes": 700, "loadedToolCount": 2, "discoveryRounds": 1,
            "discoveryLatencyMs": 200, "discoveryInputTokens": 100, "discoveryOutputTokens": 20 }
    })).unwrap());
    let report = super::super::support::document(Some(&row), std::slice::from_ref(&row), false);
    assert_eq!(report["finalRequest"]["tools"]["toolCallCount"], 2);
    assert_eq!(report["finalRequest"]["tools"]["upstreamToolCallCount"], 1);
    assert_eq!(
        report["finalRequest"]["tools"]["catalog"]["discoveryRounds"],
        1
    );
    assert_eq!(
        report["finalRequest"]["tools"]["clientContext"]["bodyBytes"],
        1000
    );
    assert_eq!(
        report["finalRequest"]["tools"]["upstreamContext"]["bodyBytes"],
        1200
    );
    row.tool_use
        .as_mut()
        .unwrap()
        .basis_points_catalog
        .as_mut()
        .unwrap()
        .mode = "PRIVATE_MODE".into();
    let private = super::super::support::document(Some(&row), std::slice::from_ref(&row), false);
    assert!(
        !serde_json::to_string(&private)
            .unwrap()
            .contains("PRIVATE_")
    );
}

#[test]
fn diagnostic_report_preserves_retry_stop_and_pending_outcomes() {
    use zenith_relay_core::AttemptOutcome::*;
    let mut row = diagnostic_rows().pop().unwrap();
    for (stage, success, http, status, terminal) in [
        (None, false, 200, "unknown", false),
        (None, false, 502, "failed", true),
        (Some(Completed), false, 200, "inconsistent", false),
        (Some(Completed), true, 200, "succeeded", true),
        (Some(ToolDiscovery), false, 200, "intermediate", false),
        (Some(ToolDiscovery), true, 200, "inconsistent", false),
        (Some(RetrySelection), false, 429, "retrying", false),
        (Some(RetryRepair), false, 400, "retrying", false),
        (Some(RetryAuthorization), false, 401, "retrying", false),
        (Some(ReturnedFailure), false, 200, "failed", true),
        (
            Some(ExecutionUncertain),
            false,
            200,
            "execution_uncertain",
            true,
        ),
        (
            Some(ClientDisconnected),
            false,
            200,
            "client_disconnected",
            true,
        ),
    ] {
        row.routing.as_mut().unwrap().attempt_outcome = stage;
        row.success = success;
        row.http_status = http;
        let report = super::super::support::document(Some(&row), std::slice::from_ref(&row), false);
        let outcome = &report["summary"]["outcome"];
        assert_eq!(outcome["status"], status, "{stage:?}");
        assert_eq!(outcome["terminal"], terminal, "{stage:?}");
        assert_eq!(outcome["recordedSuccessFlag"], success);
    }
}

#[test]
fn diagnostic_report_marks_gaps_without_discarding_the_final_send() {
    let mut rows = diagnostic_rows();
    let report = super::super::support::document(rows.last(), &rows[..1], false);
    assert_eq!(report["summary"]["finalSendAddedFromProjection"], true);
    assert_eq!(report["summary"]["includedSends"], 2);
    let input = &report["summary"]["consumption"]["inputTokens"];
    assert_eq!(input["observedTotal"], 400);
    assert_eq!(input["complete"], true);
    rows[1].attempt = 3;
    let gapped = super::super::support::document(rows.last(), &rows, false);
    assert_eq!(gapped["availability"]["attemptsTruncated"], false);
    let summary = &gapped["summary"];
    assert_eq!(summary["attemptSequenceCompleteThroughLatest"], false);
    assert_eq!(summary["consumption"]["inputTokens"]["observedTotal"], 400);
    assert_eq!(summary["consumption"]["inputTokens"]["complete"], false);
}
