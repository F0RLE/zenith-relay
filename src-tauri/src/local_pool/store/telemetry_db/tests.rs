use super::*;
use zenith_relay_core::{
    ErrorOrigin, SelectionReason, TerminalOutputKind, ToolChoiceMode, WireApi,
};
mod maintenance;
mod pricing;
mod reporting;
mod rollups;

fn aggregate_test_event(
    request_id: &str,
    attempt: u16,
    input_tokens: u64,
    cache_write_input_tokens: u64,
    cache_write_ttl: Option<String>,
    output_tokens: u64,
) -> UsageEvent {
    UsageEvent {
        request_id: request_id.into(),
        attempt,
        local_key_id: "key".into(),
        source_id: "source".into(),
        candidate_id: Some("source".into()),
        account_id: None,
        account_token_generation: None,
        client_context_id: None,
        routing: None,
        requested_model: Some("model".into()),
        resolved_model: Some("model".into()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: WireApi::Responses,
        service_tier: DefaultServiceTier::Standard,
        applied_service_tier: None,
        success: true,
        http_status: 200,
        error_category: None,
        tool_use: ToolUseDiagnostics::default(),
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: None,
        latency_ms: 1,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: Some(input_tokens),
        cached_input_tokens: None,
        cache_write_input_tokens: Some(cache_write_input_tokens),
        cache_write_ttl,
        reasoning_tokens: None,
        output_tokens: Some(output_tokens),
        total_tokens: Some(input_tokens + output_tokens),
        upstream_error: None,
        quota_snapshot: None,
    }
}

fn private_model_source_event(request_id: &str, source_id: &str) -> UsageEvent {
    let mut event = aggregate_test_event(request_id, 1, 1_000_000, 0, None, 100_000);
    event.local_key_id = "key_1".into();
    event.source_id = source_id.into();
    event.candidate_id = Some(source_id.into());
    event.requested_model = Some("private-model".into());
    event.resolved_model = Some("private-model".into());
    event.consecutive_failures = Some(0);
    event.latency_ms = 100;
    event.ttft_ms = Some(10);
    event.generation_ms = Some(90);
    event.cached_input_tokens = Some(0);
    event.reasoning_tokens = Some(0);
    event
}

fn failed_fallback_test_event(request_id: &str) -> UsageEvent {
    UsageEvent {
        request_id: request_id.into(),
        attempt: 1,
        local_key_id: "key_1".into(),
        source_id: "source_1".into(),
        candidate_id: Some("source_1".into()),
        account_id: None,
        account_token_generation: None,
        client_context_id: None,
        routing: None,
        requested_model: Some("gpt-test".into()),
        resolved_model: Some("gpt-test".into()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: WireApi::Responses,
        service_tier: DefaultServiceTier::Standard,
        applied_service_tier: None,
        success: false,
        http_status: 503,
        error_category: Some("upstream_unavailable".into()),
        tool_use: ToolUseDiagnostics::default(),
        cooldown_scope: Some("*".into()),
        retry_at_ms: Some(60_000),
        consecutive_failures: Some(1),
        latency_ms: 5,
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
