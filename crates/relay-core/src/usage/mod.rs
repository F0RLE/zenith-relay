use crate::error_codes;
mod api_equivalent;
mod upstream_error;

pub use upstream_error::UpstreamErrorDetails;

pub use api_equivalent::{
    estimate_api_equivalent_with_catalog, estimate_api_equivalent_with_token_price,
    estimate_candidate_api_equivalent_with_catalog, normalize_model_price_overrides,
    resolve_candidate_price, ApiEquivalentUsage, ApiModelPriceOverride, ApiModelPriceSources,
    SourceModelPriceOverrides,
};

/// Escapes a user value for a `LIKE ? ESCAPE '\\'` contains query.
pub fn sql_like_contains_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    for character in value.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

use crate::{quota::QuotaSnapshot, DefaultServiceTier, RoutingDiagnostics, WireApi};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Validates and normalizes provider-reported cache-window durations.
/// Multiple values are kept in ascending duration order, for example
/// `"5m, 1h"`. Unknown or malformed values stay unreported in the UI.
pub fn normalize_reported_cache_ttls(value: &str) -> Option<String> {
    let mut windows = Vec::new();
    for raw in value.split([',', '+']) {
        let raw = raw.trim().to_ascii_lowercase();
        let (amount, unit) = ["ms", "s", "m", "h", "d"]
            .into_iter()
            .find_map(|unit| raw.strip_suffix(unit).map(|amount| (amount, unit)))?;
        let amount = amount.parse::<u32>().ok()?;
        if amount == 0 {
            return None;
        }
        let multiplier = match unit {
            "ms" => 1_u64,
            "s" => 1_000,
            "m" => 60_000,
            "h" => 3_600_000,
            "d" => 86_400_000,
            _ => return None,
        };
        let duration_ms = u64::from(amount).checked_mul(multiplier)?;
        if !windows.iter().any(|(duration, _)| *duration == duration_ms) {
            windows.push((duration_ms, format!("{amount}{unit}")));
        }
        if windows.len() > 8 {
            return None;
        }
    }
    if windows.is_empty() {
        return None;
    }
    windows.sort_by_key(|(duration, _)| *duration);
    Some(
        windows
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>()
            .join(", "),
    )
}

pub type UsageCallback = Arc<dyn Fn(UsageEvent) + Send + Sync>;

/// A provider-neutral monetary value attached to measured token usage.
///
/// The OpenAI catalog is one way to produce this value; the estimate does not
/// imply that the provider charged or debited the same amount.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageValue {
    pub micro_usd: u64,
    pub priced_tokens: u64,
    pub unpriced_tokens: u64,
}

impl UsageValue {
    pub fn merge(&mut self, other: Self) {
        self.micro_usd = self.micro_usd.saturating_add(other.micro_usd);
        self.priced_tokens = self.priced_tokens.saturating_add(other.priced_tokens);
        self.unpriced_tokens = self.unpriced_tokens.saturating_add(other.unpriced_tokens);
    }
}

/// Compatibility name used by the management and desktop DTOs. New provider
/// code should use `UsageValue` so it does not imply an OpenAI-only source.
pub type ApiEquivalentSummary = UsageValue;

/// A safe, provider-reported service-tier diagnostic.
///
/// It intentionally remains separate from [`DefaultServiceTier`], which is
/// Relay's Normal/Fast pool policy. Upstreams can add tier names, so Relay
/// stores safe normalized text instead of silently discarding a new value.
pub type ObservedServiceTier = String;

pub fn normalize_observed_service_tier(value: &str) -> Option<ObservedServiceTier> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 48
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some(value.to_ascii_lowercase())
}

/// Whitelisted reasoning effort metadata for one Usage event.
///
/// This is diagnostics only. It records the client-facing request value and
/// the value present in the exact prepared upstream payload; it never infers
/// capability from a model or changes request routing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ReasoningEffortDiagnostics {
    pub(crate) requested: Option<String>,
    pub(crate) effective: Option<String>,
}

impl ReasoningEffortDiagnostics {
    pub(crate) fn from_bodies(
        client_body: &Value,
        upstream_body: &Value,
        client_wire_api: WireApi,
    ) -> Self {
        Self {
            requested: requested_reasoning_effort(client_body, client_wire_api),
            effective: effective_reasoning_effort(upstream_body),
        }
    }

    pub(crate) fn apply_to(&self, event: &mut UsageEvent) {
        event.requested_reasoning_effort.clone_from(&self.requested);
        event.effective_reasoning_effort.clone_from(&self.effective);
    }
}

fn requested_reasoning_effort(request: &Value, wire_api: WireApi) -> Option<String> {
    let effort = match wire_api {
        WireApi::Responses => request.pointer("/reasoning/effort"),
        WireApi::ChatCompletions => request.get("reasoning_effort"),
        WireApi::Messages => None,
        WireApi::Gemini => None,
    };
    effort
        .and_then(Value::as_str)
        .and_then(normalize_reasoning_effort)
}

fn effective_reasoning_effort(upstream_body: &Value) -> Option<String> {
    upstream_body
        .pointer("/output_config/effort")
        .and_then(Value::as_str)
        .and_then(normalize_reasoning_effort)
        .or_else(|| {
            upstream_body
                .pointer("/reasoning/effort")
                .and_then(Value::as_str)
                .and_then(normalize_reasoning_effort)
        })
        .or_else(|| {
            upstream_body
                .get("reasoning_effort")
                .and_then(Value::as_str)
                .and_then(normalize_reasoning_effort)
        })
        .or_else(|| {
            let budget = upstream_body
                .pointer("/thinking/budget_tokens")
                .and_then(Value::as_u64)?;
            match budget {
                1_024 => Some("minimal"),
                4_096 => Some("low"),
                8_192 => Some("medium"),
                16_384 => Some("high"),
                24_576 => Some("xhigh"),
                32_000 => Some("max"),
                _ => None,
            }
            .map(str::to_owned)
        })
}

/// Returns a canonical diagnostic effort value, rejecting arbitrary text.
pub fn normalize_reasoning_effort(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    matches!(
        value.as_str(),
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
    )
    .then_some(value)
}

/// Privacy-safe evidence about tool handling for one request. This deliberately
/// excludes tool names, arguments, prompt text, and response text.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoiceMode {
    #[default]
    Unspecified,
    Auto,
    Required,
    None,
    AllowedTools,
    Specific,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutputKind {
    #[default]
    Unknown,
    Empty,
    Text,
    ToolCall,
    Mixed,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUseDiagnostics {
    pub client_tool_count: u16,
    pub forwarded_tool_count: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_schema_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarded_schema_bytes: Option<u64>,
    #[serde(default)]
    pub filtered_tool_count: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_mode: Option<crate::ToolPolicyMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_outcome: Option<crate::ToolPolicyOutcome>,
    /// A single pre-output retry restored the original catalog after Relay's
    /// own stream parser rejected a filtered native Responses attempt.
    #[serde(default)]
    pub policy_fallback: bool,
    /// The native Responses request used provider-hosted deferred tool search.
    /// The complete trusted catalog remains available to the provider; only
    /// the tool schemas are loaded into model context on demand.
    #[serde(default)]
    pub deferred_tool_search: bool,
    #[serde(default)]
    pub tool_choice: ToolChoiceMode,
    pub tool_call_count: u16,
    pub text_output: bool,
    #[serde(default)]
    pub terminal_output: TerminalOutputKind,
}

impl ToolUseDiagnostics {
    pub fn observe_output_item(&mut self, item: &Value) {
        let output = output_observation_from_item(item);
        self.tool_call_count = self.tool_call_count.saturating_add(output.tool_call_count);
        self.text_output |= output.text_output;
    }

    pub fn observe_stream_payload(&mut self, value: &Value) {
        if let Some(response) = value.get("response") {
            self.set_terminal_response(response);
            return;
        }
        if value.get("type").and_then(Value::as_str) == Some("response.output_item.done") {
            if let Some(item) = value.get("item") {
                self.observe_output_item(item);
            }
        }
        if let Some(content_block) = value.get("content_block") {
            self.observe_output_item(content_block);
        }
        let output = output_observation_from_chat_choices(value);
        self.tool_call_count = self.tool_call_count.max(output.tool_call_count);
        self.text_output |= output.text_output;
    }

    pub fn set_terminal_response(&mut self, value: &Value) {
        let output = output_observation(value);
        let terminal_output_is_empty =
            output.inspected && output.tool_call_count == 0 && !output.text_output;
        if output.inspected
            && !(terminal_output_is_empty && (self.tool_call_count > 0 || self.text_output))
        {
            self.tool_call_count = output.tool_call_count;
            self.text_output = output.text_output;
        }
        self.finish();
    }

    pub fn finish(&mut self) {
        self.terminal_output = match (self.tool_call_count > 0, self.text_output) {
            (false, false) => TerminalOutputKind::Empty,
            (false, true) => TerminalOutputKind::Text,
            (true, false) => TerminalOutputKind::ToolCall,
            (true, true) => TerminalOutputKind::Mixed,
        };
    }

    pub fn tools_were_available_but_not_called(&self) -> bool {
        self.forwarded_tool_count > 0
            && self.tool_call_count == 0
            && matches!(self.terminal_output, TerminalOutputKind::Text)
    }

    /// Usage storage is sparse: an ordinary text request without a tools
    /// configuration should not grow a meaningless diagnostics section.
    pub fn has_evidence(&self) -> bool {
        self.client_tool_count > 0
            || self.forwarded_tool_count > 0
            || self.filtered_tool_count > 0
            || self.policy_fallback
            || self.deferred_tool_search
            || self.tool_call_count > 0
            || !matches!(self.tool_choice, ToolChoiceMode::Unspecified)
    }
}

#[derive(Default)]
struct OutputObservation {
    inspected: bool,
    tool_call_count: u16,
    text_output: bool,
}

fn output_observation(value: &Value) -> OutputObservation {
    let response = value.get("response").unwrap_or(value);
    if let Some(items) = response.get("output").and_then(Value::as_array) {
        let mut output = OutputObservation {
            inspected: true,
            ..OutputObservation::default()
        };
        for item in items {
            merge_output_observation(&mut output, output_observation_from_item(item));
        }
        return output;
    }
    if let Some(content) = response.get("content").and_then(Value::as_array) {
        let mut output = OutputObservation {
            inspected: true,
            ..OutputObservation::default()
        };
        for item in content {
            merge_output_observation(&mut output, output_observation_from_item(item));
        }
        return output;
    }
    output_observation_from_chat_choices(response)
}

fn output_observation_from_item(item: &Value) -> OutputObservation {
    let mut output = OutputObservation::default();
    match item.get("type").and_then(Value::as_str) {
        Some("function_call" | "custom_tool_call" | "tool_use") => {
            output.tool_call_count = 1;
        }
        Some("message") => {
            output.text_output = message_has_text(item);
        }
        Some("output_text" | "text") => {
            output.text_output = true;
        }
        _ => {}
    }
    output
}

fn output_observation_from_chat_choices(value: &Value) -> OutputObservation {
    let Some(choices) = value.get("choices").and_then(Value::as_array) else {
        return OutputObservation::default();
    };
    let mut output = OutputObservation {
        inspected: true,
        ..OutputObservation::default()
    };
    for choice in choices {
        let message = choice
            .get("message")
            .or_else(|| choice.get("delta"))
            .unwrap_or(choice);
        output.tool_call_count = output.tool_call_count.saturating_add(
            message
                .get("tool_calls")
                .and_then(Value::as_array)
                .map_or(0, |calls| calls.len().min(u16::MAX as usize) as u16),
        );
        if message.get("function_call").is_some() {
            output.tool_call_count = output.tool_call_count.saturating_add(1);
        }
        output.text_output |= message_has_text(message);
    }
    output
}

fn merge_output_observation(target: &mut OutputObservation, next: OutputObservation) {
    target.inspected |= next.inspected;
    target.tool_call_count = target.tool_call_count.saturating_add(next.tool_call_count);
    target.text_output |= next.text_output;
}

fn message_has_text(value: &Value) -> bool {
    match value.get("content") {
        Some(Value::String(content)) => !content.is_empty(),
        Some(Value::Array(items)) => items.iter().any(|item| {
            matches!(
                item.get("type").and_then(Value::as_str),
                Some("output_text" | "text")
            )
        }),
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorOrigin {
    Provider,
    Account,
    Relay,
}

impl ErrorOrigin {
    pub fn for_category(self, category: &str) -> Self {
        if relay_error_category(category) || adapter_error_category_is_relay(category) {
            return Self::Relay;
        }
        // Origin identifies the selected route. Whether an upstream failure
        // affects account health is a separate decision in affects_account_state.
        self
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Account => "account",
            Self::Relay => "relay",
        }
    }
}

impl std::str::FromStr for ErrorOrigin {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "provider" => Ok(Self::Provider),
            "account" => Ok(Self::Account),
            "relay" => Ok(Self::Relay),
            _ => Err(()),
        }
    }
}

define_usage_request_contract! {
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    pub request_id: String,
    pub attempt: u16,
    pub local_key_id: String,
    pub source_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Transient credential provenance for desktop account-state handling.
    ///
    /// It is deliberately excluded from persisted/exported usage. The desktop
    /// callback uses it only to make a delayed 401 a no-op when a newer OAuth
    /// credential generation is already stored for the same account.
    #[serde(skip)]
    pub account_token_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_context_id: Option<String>,
    #[serde(default)]
    pub tool_use: ToolUseDiagnostics,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consecutive_failures: Option<u32>,
    pub latency_ms: u64,
    pub ttft_ms: Option<u64>,
    pub generation_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    /// Exact retention durations reported by the upstream usage payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_ttl: Option<String>,
    pub reasoning_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_snapshot: Option<QuotaSnapshot>,
}
}

impl UsageEvent {
    /// Attributes a failed attempt to the component that produced its error.
    /// A Relay-origin error was constructed locally; otherwise the selected
    /// account or API source is responsible for the upstream result.
    pub fn error_origin(&self) -> Option<ErrorOrigin> {
        if self.success || self.error_category.is_none() {
            return None;
        }
        let category = self.error_category.as_deref().unwrap_or_default();
        let route_origin = if self.account_id.is_some() {
            ErrorOrigin::Account
        } else {
            ErrorOrigin::Provider
        };
        Some(route_origin.for_category(category))
    }

    pub fn affects_account_state(&self) -> bool {
        if self.account_id.is_none() || self.success {
            return false;
        }
        !matches!(
            self.error_category.as_deref(),
            Some(
                error_codes::CLIENT_CANCELLED
                    | error_codes::RESPONSE_AFFINITY_MISS
                    | error_codes::RESPONSE_INCOMPLETE
                    | error_codes::UPSTREAM_CANCELLED
                    | error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND
                    | error_codes::UPSTREAM_TOOL_CALL_MISMATCH
                    | error_codes::UPSTREAM_CONTEXT_TOO_LARGE
                    | error_codes::UPSTREAM_ENCRYPTED_CONTENT_INVALID
                    | error_codes::UPSTREAM_INSTRUCTIONS_REQUIRED
                    | error_codes::UPSTREAM_CONTENT_POLICY
                    | error_codes::UPSTREAM_PAYLOAD_TOO_LARGE
                    | error_codes::UPSTREAM_UNSUPPORTED_REQUEST
                    | error_codes::UPSTREAM_WEBSOCKET_UNSUPPORTED
                    | error_codes::UPSTREAM_INVALID_REQUEST
                    | error_codes::UPSTREAM_MODEL_NOT_FOUND
                    | error_codes::UPSTREAM_MODEL_UNSUPPORTED
                    | error_codes::UPSTREAM_USAGE_NOT_INCLUDED
                    | error_codes::UPSTREAM_MODEL_CAPACITY
                    | error_codes::UPSTREAM_OVERLOADED
                    | error_codes::UPSTREAM_SERVER_ERROR
                    | error_codes::UPSTREAM_BAD_GATEWAY
                    | error_codes::UPSTREAM_UNAVAILABLE
                    | error_codes::UPSTREAM_GATEWAY_TIMEOUT
                    | error_codes::IMAGE_GENERATION_NOT_ENABLED
            )
        )
    }
}

fn relay_error_category(category: &str) -> bool {
    matches!(
        category,
        error_codes::INVALID_REQUEST
            | error_codes::MODEL_NOT_FOUND
            | error_codes::NO_ELIGIBLE_SOURCE
            | error_codes::ALL_SOURCES_TEMPORARILY_UNAVAILABLE
            | error_codes::ALL_SOURCES_COOLING_DOWN
            | "adapter_websocket_not_supported"
            | error_codes::CLIENT_CANCELLED
            | "client_websocket"
            | error_codes::RESPONSE_AFFINITY_MISS
            | error_codes::STREAM_EVENT_TOO_LARGE
    )
}

fn adapter_error_category_is_relay(category: &str) -> bool {
    category.starts_with("adapter_") && !category.starts_with("adapter_upstream_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        );
        assert_eq!(bridge.requested.as_deref(), Some("max"));
        assert_eq!(bridge.effective.as_deref(), Some("low"));

        let budget = ReasoningEffortDiagnostics::from_bodies(
            &json!({"reasoning_effort": "high"}),
            &json!({"thinking": {"type": "enabled", "budget_tokens": 32_000}}),
            WireApi::ChatCompletions,
        );
        assert_eq!(budget.requested.as_deref(), Some("high"));
        assert_eq!(budget.effective.as_deref(), Some("max"));

        let absent = ReasoningEffortDiagnostics::from_bodies(
            &json!({"reasoning": {"effort": "untrusted value"}}),
            &json!({"thinking": {"budget_tokens": 123}}),
            WireApi::Responses,
        );
        assert_eq!(absent, ReasoningEffortDiagnostics::default());
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
        let mut old: ToolUseDiagnostics = serde_json::from_value(json!({
            "clientToolCount":73,"forwardedToolCount":0,"toolCallCount":0,"textOutput":true,"terminalOutput":"text"
        })).unwrap();
        assert_eq!(old.client_schema_bytes, None);
        assert_eq!(old.policy_mode, None);
        assert_eq!(old.filtered_tool_count, 0);
        assert!(!old.tools_were_available_but_not_called());
        old.policy_mode = Some(crate::ToolPolicyMode::PassThrough);
        old.policy_outcome = Some(crate::ToolPolicyOutcome::PassThrough);
        old.client_schema_bytes = Some(12345);
        old.forwarded_schema_bytes = Some(2);
        old.filtered_tool_count = 73;
        let json = serde_json::to_value(&old).unwrap();
        assert_eq!(
            serde_json::from_value::<ToolUseDiagnostics>(json).unwrap(),
            old
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
}
