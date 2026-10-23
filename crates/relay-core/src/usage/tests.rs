use super::*;
use crate::{DefaultServiceTier, WireApi};
use serde_json::json;

#[test]
fn upstream_model_declarations_are_bounded_and_do_not_change_routing_identity() {
    let mut event = failed_usage_event("synthetic", None);
    event.routing = Some(
        serde_json::from_value(json!({
            "reason":"only_eligible", "eligibleCandidates":1,
            "quotaRemainingBasisPoints":null, "inFlightBefore":0, "dispatchesBefore":0
        }))
        .unwrap(),
    );
    let requested = event.requested_model.clone();
    let resolved = event.resolved_model.clone();
    for payload in [
        json!({"model":"synthetic-snapshot"}),
        json!({"response":{"model":"synthetic-snapshot"}}),
        json!({"message":{"model":"synthetic-snapshot"}}),
        json!({"modelVersion":"synthetic-snapshot"}),
    ] {
        event.observe_response_metadata(&payload);
        assert_eq!(
            event
                .routing
                .as_ref()
                .unwrap()
                .upstream_response_model
                .as_deref(),
            Some("synthetic-snapshot")
        );
    }
    for model in [
        "",
        " ",
        "synthetic\nprivate",
        "synthetic private",
        "sk-synthetic-secret",
        "acct_synthetic",
        "synthetic@example.test",
        "https://example.test/private",
        &"x".repeat(1025),
    ] {
        event.observe_response_metadata(&json!({"model":model}));
        assert_eq!(
            event
                .routing
                .as_ref()
                .unwrap()
                .upstream_response_model
                .as_deref(),
            Some("synthetic-snapshot")
        );
    }
    assert_eq!(event.requested_model, requested);
    assert_eq!(event.resolved_model, resolved);
    let encoded = serde_json::to_value(event.routing.as_ref().unwrap()).unwrap();
    assert_eq!(encoded["upstreamResponseModel"], "synthetic-snapshot");
    let mut legacy = encoded;
    legacy
        .as_object_mut()
        .unwrap()
        .remove("upstreamResponseModel");
    assert!(serde_json::from_value::<crate::RoutingDiagnostics>(legacy)
        .unwrap()
        .upstream_response_model
        .is_none());
}

#[test]
fn provider_cache_windows_are_normalized_without_guessing() {
    assert_eq!(
        normalize_reported_cache_ttls("1h, 5m, 5m"),
        Some("5m, 1h".to_string())
    );
    assert_eq!(
        normalize_reported_cache_ttls("15m + 30s"),
        Some("30s, 15m".to_string())
    );
    assert_eq!(normalize_reported_cache_ttls("unknown"), None);
    assert_eq!(normalize_reported_cache_ttls("0m"), None);
}

fn failed_usage_event(category: &str, account_id: Option<&str>) -> UsageEvent {
    UsageEvent {
        request_id: "request".into(),
        attempt: 1,
        local_key_id: "key".into(),
        source_id: "source".into(),
        candidate_id: Some("candidate".into()),
        account_id: account_id.map(str::to_owned),
        account_token_generation: None,
        client_context_id: None,
        routing: None,
        requested_model: Some("model".into()),
        resolved_model: Some("model".into()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: WireApi::Responses,
        transport: crate::UsageTransport::Http,
        service_tier: DefaultServiceTier::Standard,
        applied_service_tier: None,
        success: false,
        http_status: 502,
        error_category: Some(category.into()),
        tool_use: ToolUseDiagnostics::default(),
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: None,
        latency_ms: 0,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: None,
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        cache_write_ttl: None,
        reasoning_tokens: None,
        output_tokens: None,
        total_tokens: None,
        upstream_error: None,
        quota_snapshot: None,
    }
}

#[test]
fn failed_usage_events_keep_the_component_that_produced_the_error() {
    assert_eq!(
        failed_usage_event("upstream_invalid_request", None).error_origin(),
        Some(ErrorOrigin::Provider)
    );
    assert_eq!(
        failed_usage_event("upstream_transport", Some("account")).error_origin(),
        Some(ErrorOrigin::Account)
    );
    assert_eq!(
        failed_usage_event("websocket_idle_timeout", Some("account")).error_origin(),
        Some(ErrorOrigin::Account)
    );
    assert_eq!(
        failed_usage_event("stream_semantic_timeout", None).error_origin(),
        Some(ErrorOrigin::Provider)
    );
    assert_eq!(
        failed_usage_event("invalid_request", Some("account")).error_origin(),
        Some(ErrorOrigin::Relay)
    );
    assert_eq!(
        failed_usage_event("adapter_upstream_error", None).error_origin(),
        Some(ErrorOrigin::Provider)
    );
    assert_eq!(
        failed_usage_event("adapter_upstream_response_invalid", Some("account")).error_origin(),
        Some(ErrorOrigin::Account)
    );
    assert_eq!(
        failed_usage_event("adapter_invalid_request", Some("account")).error_origin(),
        Some(ErrorOrigin::Relay)
    );
    assert_eq!(
        failed_usage_event("upstream_overloaded", Some("account")).error_origin(),
        Some(ErrorOrigin::Account)
    );
    assert_eq!(
        failed_usage_event("upstream_server_error", Some("account")).error_origin(),
        Some(ErrorOrigin::Account)
    );
}

#[test]
fn upstream_service_failures_do_not_mark_the_selected_account() {
    for category in [
        "upstream_model_capacity",
        "upstream_overloaded",
        "upstream_server_error",
        "upstream_bad_gateway",
        "upstream_unavailable",
        "upstream_gateway_timeout",
    ] {
        let event = failed_usage_event(category, Some("account"));
        assert_eq!(event.error_origin(), Some(ErrorOrigin::Account));
        assert!(!event.affects_account_state());
    }
}

#[test]
fn siwc_admission_failure_preserves_account_credentials_and_quota() {
    for category in [
        "upstream_unauthorized",
        "upstream_forbidden",
        "upstream_quota_exhausted",
    ] {
        let mut event = failed_usage_event(category, Some("synthetic-account"));
        event.routing = Some(
            serde_json::from_value(json!({
                "reason":"only_eligible", "eligibleCandidates":1,
                "quotaRemainingBasisPoints":null, "inFlightBefore":0, "dispatchesBefore":0,
                "endpointKind":"chatgpt_siwc"
            }))
            .unwrap(),
        );
        assert!(!event.affects_account_state());
        assert_eq!(event.error_origin(), Some(ErrorOrigin::Account));
    }
}

#[test]
fn error_origin_round_trips_through_storage_values() {
    for origin in [
        ErrorOrigin::Provider,
        ErrorOrigin::Account,
        ErrorOrigin::Relay,
    ] {
        assert_eq!(origin.as_str().parse(), Ok(origin));
    }
    assert!("unknown".parse::<ErrorOrigin>().is_err());
}

#[test]
fn observed_service_tier_preserves_safe_upstream_values() {
    assert_eq!(
        normalize_observed_service_tier("priority"),
        Some("priority".to_string())
    );
    assert_eq!(
        normalize_observed_service_tier("flex"),
        Some("flex".to_string())
    );
    assert_eq!(
        normalize_observed_service_tier("ultrafast"),
        Some("ultrafast".to_string())
    );
    assert_eq!(
        normalize_observed_service_tier(" Standard "),
        Some("standard".to_string())
    );
    assert_eq!(normalize_observed_service_tier("bad value"), None);
    assert_eq!(normalize_observed_service_tier("bad\nvalue"), None);
}

#[test]
fn reasoning_effort_diagnostics_keep_only_normalized_request_and_payload_values() {
    let bridge = ReasoningEffortDiagnostics::from_bodies(
        &json!({"reasoning": {"effort": " Max "}}),
        &json!({
            "thinking": {"type": "adaptive"},
            "output_config": {"effort": " Low "}
        }),
        WireApi::Responses,
        WireApi::Messages,
    );
    assert_eq!(bridge.requested.as_deref(), Some("max"));
    assert_eq!(bridge.effective.as_deref(), Some("low"));

    let budget = ReasoningEffortDiagnostics::from_bodies(
        &json!({"reasoning_effort": "high"}),
        &json!({"thinking": {"type": "enabled", "budget_tokens": 32_000}}),
        WireApi::ChatCompletions,
        WireApi::Messages,
    );
    assert_eq!(budget.requested.as_deref(), Some("high"));
    assert_eq!(budget.effective.as_deref(), Some("max"));

    let absent = ReasoningEffortDiagnostics::from_bodies(
        &json!({"reasoning": {"effort": "untrusted value"}}),
        &json!({"thinking": {"budget_tokens": 123}}),
        WireApi::Responses,
        WireApi::Messages,
    );
    assert_eq!(absent, ReasoningEffortDiagnostics::default());
}

#[test]
fn reasoning_effort_diagnostics_read_messages_and_gemini_controls() {
    for (wire_api, body) in [
        (
            WireApi::Messages,
            json!({"output_config":{"effort":"Very high"}}),
        ),
        (
            WireApi::Gemini,
            json!({"generationConfig":{"thinkingConfig":{"thinkingLevel":"HIGH"}}}),
        ),
    ] {
        let observed = ReasoningEffortDiagnostics::from_bodies(&body, &body, wire_api, wire_api);
        let expected = if wire_api == WireApi::Messages {
            "xhigh"
        } else {
            "high"
        };
        assert_eq!(observed.requested.as_deref(), Some(expected));
        assert_eq!(observed.effective.as_deref(), Some(expected));
    }
}

#[test]
fn reasoning_effort_diagnostics_use_the_exact_upstream_contract() {
    let body = json!({
        "reasoning":{"effort":"low"},
        "reasoning_effort":"medium",
        "output_config":{"effort":"high"},
        "generationConfig":{"thinkingConfig":{"thinkingLevel":"Very high"}}
    });
    for (wire_api, expected) in [
        (WireApi::Responses, "low"),
        (WireApi::ChatCompletions, "medium"),
        (WireApi::Messages, "high"),
        (WireApi::Gemini, "xhigh"),
    ] {
        let observed = ReasoningEffortDiagnostics::from_bodies(&body, &body, wire_api, wire_api);
        assert_eq!(observed.requested.as_deref(), Some(expected));
        assert_eq!(observed.effective.as_deref(), Some(expected));
    }
    let bps = ReasoningEffortDiagnostics::from_basis_points_bodies(
        &json!({"reasoning":{"effort":"max"}}),
        &json!({"reasoning_effort":"xhigh"}),
        WireApi::Responses,
    );
    assert_eq!(bps.requested.as_deref(), Some("max"));
    assert_eq!(bps.effective.as_deref(), Some("xhigh"));

    for (wire_api, controls, expected) in [
        (
            WireApi::Messages,
            json!({"thinking":{"type":"disabled"}}),
            "none",
        ),
        (
            WireApi::Gemini,
            json!({"generationConfig":{"thinkingConfig":{"thinkingBudget":0}}}),
            "none",
        ),
        (
            WireApi::Gemini,
            json!({"generationConfig":{"thinkingConfig":{"thinkingBudget":24_576}}}),
            "xhigh",
        ),
    ] {
        let observed =
            ReasoningEffortDiagnostics::from_bodies(&controls, &controls, wire_api, wire_api);
        assert_eq!(observed.requested.as_deref(), Some(expected));
        assert_eq!(observed.effective.as_deref(), Some(expected));
    }
}

#[test]
fn sql_like_pattern_escapes_wildcards_and_escape_characters() {
    assert_eq!(
        sql_like_contains_pattern(r"model%_\name"),
        r"%model\%\_\\name%"
    );
}

#[test]
fn tool_diagnostics_record_counts_without_retaining_tool_content() {
    let mut diagnostics = ToolUseDiagnostics {
        client_tool_count: 2,
        forwarded_tool_count: 2,
        ..ToolUseDiagnostics::default()
    };
    diagnostics.set_terminal_response(&json!({
        "output": [{
            "type": "function_call",
            "name": "private_tool_name",
            "arguments": "{\"secret\":\"value\"}"
        }]
    }));

    assert_eq!(diagnostics.tool_call_count, 1);
    assert_eq!(diagnostics.terminal_output, TerminalOutputKind::ToolCall);
    let stored = serde_json::to_string(&diagnostics).unwrap();
    assert!(!stored.contains("private_tool_name"));
    assert!(!stored.contains("secret"));
}

#[test]
fn tool_diagnostics_marks_text_only_completion_when_tools_were_offered() {
    let mut diagnostics = ToolUseDiagnostics {
        client_tool_count: 1,
        forwarded_tool_count: 1,
        tool_choice: ToolChoiceMode::Auto,
        ..ToolUseDiagnostics::default()
    };
    diagnostics.set_terminal_response(&json!({
        "output": [{
            "type": "message",
            "content": [{"type": "output_text"}]
        }]
    }));

    assert_eq!(diagnostics.terminal_output, TerminalOutputKind::Text);
    assert!(diagnostics.tools_were_available_but_not_called());
}

#[test]
fn tool_policy_diagnostics_read_old_records_and_round_trip_without_false_availability() {
    let mut legacy_diagnostics: ToolUseDiagnostics = serde_json::from_value(json!({
        "clientToolCount":73,"forwardedToolCount":0,"toolCallCount":0,"textOutput":true,"terminalOutput":"text"
    })).unwrap();
    assert_eq!(legacy_diagnostics.client_schema_bytes, None);
    assert_eq!(legacy_diagnostics.policy_mode, None);
    assert_eq!(legacy_diagnostics.filtered_tool_count, 0);
    assert!(!legacy_diagnostics.tools_were_available_but_not_called());
    legacy_diagnostics.policy_mode = Some(crate::ToolPolicyMode::PassThrough);
    legacy_diagnostics.policy_outcome = Some(crate::ToolPolicyOutcome::PassThrough);
    legacy_diagnostics.client_schema_bytes = Some(12345);
    legacy_diagnostics.forwarded_schema_bytes = Some(2);
    legacy_diagnostics.filtered_tool_count = 73;
    let json = serde_json::to_value(&legacy_diagnostics).unwrap();
    assert_eq!(
        serde_json::from_value::<ToolUseDiagnostics>(json).unwrap(),
        legacy_diagnostics
    );
    assert!(ToolUseDiagnostics {
        filtered_tool_count: 2,
        ..Default::default()
    }
    .has_evidence());
}

#[test]
fn stream_completion_without_output_keeps_completed_tool_item() {
    let mut diagnostics = ToolUseDiagnostics::default();
    diagnostics.observe_stream_payload(&json!({
        "type": "response.output_item.done",
        "item": {"type": "custom_tool_call"}
    }));
    diagnostics.observe_stream_payload(&json!({
        "type": "response.completed",
        "response": {"output": []}
    }));

    assert_eq!(diagnostics.tool_call_count, 1);
    assert_eq!(diagnostics.terminal_output, TerminalOutputKind::ToolCall);
}

#[test]
fn tool_diagnostics_are_absent_without_a_tool_request_or_result() {
    let mut diagnostics = ToolUseDiagnostics::default();
    diagnostics.set_terminal_response(&json!({
        "output": [{
            "type": "message",
            "content": [{"type": "output_text"}]
        }]
    }));

    assert_eq!(diagnostics.terminal_output, TerminalOutputKind::Text);
    assert!(!diagnostics.has_evidence());
}

#[test]
fn parallel_diagnostics_keep_unknown_zero_and_call_readiness_distinct() {
    let mut diagnostics = ToolUseDiagnostics::default();
    let legacy = serde_json::to_value(&diagnostics).unwrap();
    for key in [
        "requestedParallelToolCalls",
        "upstreamToolCallCount",
        "toolCallsReadyMs",
    ] {
        assert!(legacy.get(key).is_none());
    }
    let restored: ToolUseDiagnostics = serde_json::from_value(legacy).unwrap();
    assert_eq!(restored, diagnostics);
    diagnostics.observe_upstream_response(&json!({"output":[]}));
    assert_eq!(diagnostics.upstream_tool_call_count, None);
    diagnostics.client_context = Some(Default::default());
    diagnostics.observe_upstream_response(&json!({"output":[]}));
    assert_eq!(diagnostics.upstream_tool_call_count, Some(0));
    assert!(diagnostics.has_evidence());
    diagnostics.tool_call_count = 1;
    diagnostics.observe_tool_calls_ready(20);
    assert_eq!(diagnostics.tool_calls_ready_ms, None);
    diagnostics.basis_points_catalog = Some(
        serde_json::from_value(json!({
            "mode":"compact", "fullSchemaBytes":0, "renderedSchemaBytes":0,
            "catalogPromptBytes":0, "loadedToolCount":0, "discoveryRounds":0
        }))
        .unwrap(),
    );
    diagnostics.observe_tool_calls_ready(0);
    diagnostics.observe_tool_calls_ready(80);
    assert_eq!(diagnostics.tool_calls_ready_ms, Some(0));
    diagnostics.observe_upstream_response(&json!({"output":[{
        "type":"function_call", "name":"synthetic-private-name", "arguments":"synthetic-private-value"
    }]}));
    assert_eq!(diagnostics.upstream_tool_call_count, Some(1));
    let serialized = serde_json::to_string(&diagnostics).unwrap();
    assert!(!serialized.contains("synthetic-private"));
}
