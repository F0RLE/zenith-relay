mod api_equivalent;

pub use api_equivalent::{
    api_model_price, api_pricing_revision, estimate_api_equivalent,
    estimate_api_equivalent_with_price_override, normalize_model_price_overrides, ApiModelPrice,
    ApiModelPriceOverride, MAX_MODEL_PRICE_MICRO_USD_PER_MILLION,
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

pub type UsageCallback = Arc<dyn Fn(UsageEvent) + Send + Sync>;

/// A provider-neutral monetary value attached to measured token usage.
///
/// The OpenAI catalog is one way to produce this value; quota calibration only
/// consumes the value and does not know how the provider price was obtained.
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
        self.client_tool_count > 0
            && self.tool_call_count == 0
            && matches!(self.terminal_output, TerminalOutputKind::Text)
    }

    /// Usage storage is sparse: an ordinary text request without a tools
    /// configuration should not grow a meaningless diagnostics section.
    pub fn has_evidence(&self) -> bool {
        self.client_tool_count > 0
            || self.forwarded_tool_count > 0
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingDiagnostics>,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    pub wire_api: WireApi,
    #[serde(default)]
    pub service_tier: DefaultServiceTier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_service_tier: Option<DefaultServiceTier>,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
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
    pub reasoning_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_snapshot: Option<QuotaSnapshot>,
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
        if relay_error_category(category) || category.starts_with("adapter_") {
            return Some(ErrorOrigin::Relay);
        }
        if self.account_id.is_some() {
            return Some(ErrorOrigin::Account);
        }
        Some(ErrorOrigin::Provider)
    }

    pub fn affects_account_state(&self) -> bool {
        if self.account_id.is_none() || self.success {
            return false;
        }
        !matches!(
            self.error_category.as_deref(),
            Some(
                "client_cancelled"
                    | "response_affinity_miss"
                    | "response_incomplete"
                    | "upstream_cancelled"
                    | "upstream_previous_response_not_found"
                    | "upstream_tool_call_mismatch"
                    | "upstream_context_too_large"
                    | "upstream_encrypted_content_invalid"
                    | "upstream_instructions_required"
                    | "upstream_content_policy"
                    | "upstream_payload_too_large"
                    | "upstream_unsupported_request"
                    | "upstream_websocket_unsupported"
                    | "upstream_invalid_request"
                    | "upstream_model_not_found"
                    | "upstream_model_unsupported"
                    | "upstream_usage_not_included"
                    | "image_generation_not_enabled"
            )
        )
    }
}

fn relay_error_category(category: &str) -> bool {
    matches!(
        category,
        "invalid_request"
            | "reasoning_effort_not_allowed"
            | "model_not_found"
            | "no_eligible_source"
            | "all_sources_temporarily_unavailable"
            | "all_sources_cooling_down"
            | "adapter_websocket_not_supported"
            | "client_cancelled"
            | "client_websocket"
            | "response_affinity_miss"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn failed_usage_event(category: &str, account_id: Option<&str>) -> UsageEvent {
        UsageEvent {
            request_id: "request".into(),
            attempt: 1,
            local_key_id: "key".into(),
            source_id: "source".into(),
            candidate_id: Some("candidate".into()),
            account_id: account_id.map(str::to_owned),
            routing: None,
            requested_model: Some("model".into()),
            resolved_model: Some("model".into()),
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
            reasoning_tokens: None,
            output_tokens: None,
            total_tokens: None,
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
            failed_usage_event("invalid_request", Some("account")).error_origin(),
            Some(ErrorOrigin::Relay)
        );
        assert_eq!(
            failed_usage_event("adapter_upstream_error", None).error_origin(),
            Some(ErrorOrigin::Relay)
        );
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
