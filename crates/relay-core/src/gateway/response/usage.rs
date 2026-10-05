use super::super::now_ms;
use crate::runtime::{AccountTransport, ExecutorRoute};
use crate::usage::{normalize_reported_cache_ttls, ReasoningEffortDiagnostics};
use crate::{
    normalize_observed_service_tier, GatewayRuntime, ObservedServiceTier, ToolUseDiagnostics,
    UsageEvent,
};
use serde_json::Value;

pub(in crate::gateway) struct UsageAttempt<'a> {
    pub(in crate::gateway) request_id: &'a str,
    pub(in crate::gateway) attempt: u16,
    pub(in crate::gateway) local_key_id: &'a str,
    pub(in crate::gateway) route: &'a ExecutorRoute,
    pub(in crate::gateway) reasoning_effort: Option<&'a ReasoningEffortDiagnostics>,
    pub(in crate::gateway) requested_model: &'a str,
    pub(in crate::gateway) tool_use: ToolUseDiagnostics,
}

pub(in crate::gateway) fn usage_event(
    attempt: UsageAttempt<'_>,
    success: bool,
    http_status: u16,
    error_category: Option<String>,
    latency_ms: u64,
) -> UsageEvent {
    let UsageAttempt {
        request_id,
        attempt,
        local_key_id,
        route,
        reasoning_effort,
        requested_model,
        tool_use,
    } = attempt;
    let mut routing = route.routing.clone();
    if let Some(diagnostics) = routing.as_mut() {
        diagnostics.endpoint_kind = Some(match route.account_transport {
            AccountTransport::NativeResponses => route
                .adapter
                .route_suffix(route.client_wire_api)
                .to_string(),
            AccountTransport::ExcelBasisPoints => "excel_basis_points".to_string(),
        });
    }
    let mut event = UsageEvent {
        request_id: request_id.to_string(),
        attempt,
        local_key_id: local_key_id.to_string(),
        source_id: route.source_id.clone(),
        candidate_id: Some(route.candidate_id.clone()),
        account_id: route.account_id.clone(),
        account_token_generation: route.account_token_generation,
        client_context_id: route.client_context_id.clone(),
        routing,
        requested_model: Some(requested_model.to_string()),
        resolved_model: Some(route.source_model.clone()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: route.client_wire_api,
        transport: route.client_transport,
        service_tier: route.service_tier,
        applied_service_tier: None,
        success,
        http_status,
        error_category,
        tool_use,
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: None,
        latency_ms,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: None,
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        // Usage records describe what the upstream actually reported, not the
        // configured preference. `apply_usage` sets this only for a cache write.
        cache_write_ttl: None,
        reasoning_tokens: None,
        output_tokens: None,
        total_tokens: None,
        upstream_error: None,
        quota_snapshot: None,
    };
    if let Some(reasoning_effort) = reasoning_effort {
        reasoning_effort.apply_to(&mut event);
    }
    event
}

pub(in crate::gateway) fn populate_tokens(event: &mut UsageEvent, body: &[u8]) {
    let Ok(body) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    event.tool_use.set_terminal_response(&body);
    event.applied_service_tier = response_service_tier(&body);
    let Some(usage) = find_usage(&body) else {
        return;
    };
    apply_usage(event, usage);
}

pub(in crate::gateway) fn response_service_tier(value: &Value) -> Option<ObservedServiceTier> {
    std::iter::successors(Some(value), |value| value.get("response"))
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .find_map(|value| value.get("service_tier").and_then(Value::as_str))
        .and_then(normalize_observed_service_tier)
}

pub(in crate::gateway) fn emit_usage(runtime: &GatewayRuntime, mut event: UsageEvent) {
    if event.success && event.error_category.is_none() {
        if let Some(origin) = runtime.request_origin(&event.request_id) {
            event.error_category = Some(format!("codex_{origin}"));
        }
    }
    if event
        .account_id
        .as_deref()
        .is_some_and(|account_id| !runtime.account_candidate_is_active(account_id))
    {
        return;
    }
    let observed_at_ms = now_ms();
    runtime.apply_usage_event(&event, observed_at_ms);
    if event.quota_snapshot.is_none() {
        event.quota_snapshot = event.candidate_id.as_deref().and_then(|candidate_id| {
            runtime.take_passive_quota_snapshot(candidate_id, observed_at_ms)
        });
    }
    emit_callback(&runtime.usage, event);
}

pub(in crate::gateway) fn emit_callback(callback: &crate::UsageCallback, event: UsageEvent) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(event)));
}

pub(in crate::gateway) fn apply_usage(event: &mut UsageEvent, usage: &Value) {
    let gemini = usage.get("usageMetadata").unwrap_or(usage);
    let reported_input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .or_else(|| gemini.get("promptTokenCount"))
        .and_then(Value::as_u64);
    let anthropic_cache_read_tokens = usage.get("cache_read_input_tokens").and_then(Value::as_u64);
    let anthropic_cache_write_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64);
    let input_tokens =
        if anthropic_cache_read_tokens.is_some() || anthropic_cache_write_tokens.is_some() {
            reported_input_tokens.map(|input| {
                input
                    .saturating_add(anthropic_cache_read_tokens.unwrap_or_default())
                    .saturating_add(anthropic_cache_write_tokens.unwrap_or_default())
            })
        } else {
            reported_input_tokens
        };
    let mut output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .or_else(|| gemini.get("candidatesTokenCount"))
        .and_then(Value::as_u64);
    if gemini.get("candidatesTokenCount").is_some() {
        output_tokens = output_tokens.map(|output| {
            output.saturating_add(
                gemini
                    .get("thoughtsTokenCount")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            )
        });
    }
    if let Some(input_tokens) = input_tokens {
        event.input_tokens = Some(input_tokens);
    }
    let cached_input_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
        })
        .or_else(|| usage.get("cached_tokens"))
        .or_else(|| usage.get("cache_read_input_tokens"))
        .or_else(|| gemini.get("cachedContentTokenCount"))
        .and_then(Value::as_u64)
        .map(|cached| cached.min(input_tokens.unwrap_or(event.input_tokens.unwrap_or(cached))));
    if let Some(cached_input_tokens) = cached_input_tokens {
        event.cached_input_tokens = Some(cached_input_tokens);
    }
    let cache_write_input_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cache_write_tokens"))
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cache_write_tokens"))
        })
        .or_else(|| usage.get("cache_write_tokens"))
        .or_else(|| usage.get("cache_creation_input_tokens"))
        .and_then(Value::as_u64)
        .map(|written| {
            written.min(
                input_tokens
                    .unwrap_or(written)
                    .saturating_sub(event.cached_input_tokens.unwrap_or_default()),
            )
        });
    if let Some(cache_write_input_tokens) = cache_write_input_tokens {
        event.cache_write_input_tokens = Some(cache_write_input_tokens);
    }
    event.cache_write_ttl =
        cache_write_ttl_from_usage(usage).or_else(|| event.cache_write_ttl.clone());
    let reasoning_tokens = usage
        .get("reasoning_tokens")
        .or_else(|| {
            usage
                .get("output_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
        })
        .or_else(|| {
            usage
                .get("completion_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
        })
        .or_else(|| gemini.get("thoughtsTokenCount"))
        .and_then(Value::as_u64)
        .map(|reasoning| {
            reasoning.min(output_tokens.unwrap_or(event.output_tokens.unwrap_or(reasoning)))
        });
    if let Some(reasoning_tokens) = reasoning_tokens {
        event.reasoning_tokens = Some(reasoning_tokens);
    }
    if let Some(output_tokens) = output_tokens {
        event.output_tokens = Some(output_tokens);
    }
    let reported_total = usage
        .get("total_tokens")
        .or_else(|| gemini.get("totalTokenCount"))
        .and_then(Value::as_u64);
    if let Some(total_tokens) = reported_total.or_else(|| {
        input_tokens
            .zip(output_tokens)
            .map(|(input, output)| input.saturating_add(output))
    }) {
        event.total_tokens = Some(
            total_tokens.max(
                event
                    .input_tokens
                    .unwrap_or_default()
                    .saturating_add(event.output_tokens.unwrap_or_default()),
            ),
        );
    }
}

pub(in crate::gateway) fn find_usage(value: &Value) -> Option<&Value> {
    value
        .get("usage")
        .or_else(|| value.get("usageMetadata"))
        .or_else(|| value.pointer("/message/usage"))
        .or_else(|| {
            let response = value.get("response")?;
            response.get("usage").or_else(|| {
                response
                    .get("response")
                    .and_then(|nested| nested.get("usage"))
            })
        })
}

pub(in crate::gateway) fn response_id(value: &Value) -> Option<&str> {
    value
        .pointer("/response/response/id")
        .or_else(|| value.pointer("/response/id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(in crate::gateway) fn response_id_from_bytes(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| response_id(&value).map(str::to_string))
}

pub(in crate::gateway::response) fn cache_write_ttl_from_usage(usage: &Value) -> Option<String> {
    let mut windows = Vec::new();
    let mut usage_objects = vec![usage];
    for field in [
        "input_tokens_details",
        "prompt_tokens_details",
        "inputTokensDetails",
        "promptTokensDetails",
    ] {
        if let Some(details) = usage.get(field) {
            usage_objects.push(details);
        }
    }

    for object in usage_objects {
        for field in [
            "cache_write_ttl",
            "cacheWriteTtl",
            "cache_creation_ttl",
            "cacheCreationTtl",
        ] {
            if let Some(value) = object.get(field).and_then(Value::as_str) {
                windows.push(value.to_string());
            }
        }

        if let Some(object) = object.as_object() {
            for (key, tokens) in object {
                if tokens.as_u64().is_some_and(|tokens| tokens > 0) {
                    if let Some(window) = cache_window_from_token_field(key) {
                        windows.push(window);
                    }
                }
            }
        }

        if let Some(creation) = object
            .get("cache_creation")
            .or_else(|| object.get("cacheCreation"))
            .and_then(Value::as_object)
        {
            for (key, tokens) in creation {
                let window = key
                    .strip_prefix("ephemeral_")
                    .and_then(|key| key.strip_suffix("_input_tokens"));
                if tokens.as_u64().is_some_and(|tokens| tokens > 0) {
                    if let Some(window) = window {
                        windows.push(window.to_string());
                    }
                }
            }
        }
    }
    normalize_reported_cache_ttls(&windows.join(","))
}

fn cache_window_from_token_field(key: &str) -> Option<String> {
    let normalized = key.to_ascii_lowercase();
    [
        "cache_creation_input_tokens_",
        "cache_creation_tokens_",
        "cachecreationinputtokens",
        "cachecreationtokens",
        "cache_write_input_tokens_",
        "cachewriteinputtokens",
        "cache_write_tokens_",
        "cachewritetokens",
    ]
    .into_iter()
    .find_map(|prefix| normalized.strip_prefix(prefix))
    .map(str::to_string)
}
