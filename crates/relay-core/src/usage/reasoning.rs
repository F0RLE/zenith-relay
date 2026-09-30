use super::UsageEvent;
use crate::WireApi;
use serde_json::Value;
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
