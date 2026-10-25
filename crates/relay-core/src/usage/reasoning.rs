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
        upstream_wire_api: WireApi,
    ) -> Self {
        Self {
            requested: observed_reasoning_effort(client_body, client_wire_api),
            effective: observed_reasoning_effort(upstream_body, upstream_wire_api),
        }
    }

    /// BPS has a Responses endpoint but uses a flat effort field in its payload.
    pub(crate) fn from_basis_points_bodies(
        client_body: &Value,
        upstream_body: &Value,
        client_wire_api: WireApi,
    ) -> Self {
        Self {
            requested: observed_reasoning_effort(client_body, client_wire_api),
            effective: normalized_value(upstream_body.get("reasoning_effort")),
        }
    }

    pub(crate) fn apply_to(&self, event: &mut UsageEvent) {
        event.requested_reasoning_effort.clone_from(&self.requested);
        event.effective_reasoning_effort.clone_from(&self.effective);
    }
}

fn observed_reasoning_effort(request: &Value, wire_api: WireApi) -> Option<String> {
    let effort = match wire_api {
        WireApi::Responses => request.pointer("/reasoning/effort"),
        WireApi::ChatCompletions => request.get("reasoning_effort"),
        WireApi::Messages => request.pointer("/output_config/effort"),
        WireApi::Gemini => request.pointer("/generationConfig/thinkingConfig/thinkingLevel"),
        WireApi::GeminiInteractions => request.pointer("/generation_config/thinking_level"),
    };
    normalized_value(effort).or_else(|| {
        let budget = match wire_api {
            WireApi::Messages if request.pointer("/thinking/type")?.as_str()? == "disabled" => {
                return Some("none".into());
            }
            WireApi::Messages if request.pointer("/thinking/type")?.as_str()? == "enabled" => {
                request.pointer("/thinking/budget_tokens")
            }
            WireApi::Gemini => request.pointer("/generationConfig/thinkingConfig/thinkingBudget"),
            _ => None,
        }?
        .as_u64()?;
        match budget {
            0 if wire_api == WireApi::Gemini => Some("none"),
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

fn normalized_value(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .and_then(normalize_reasoning_effort)
}

/// Returns a canonical diagnostic effort value, rejecting arbitrary text.
pub fn normalize_reasoning_effort(effort_text: &str) -> Option<String> {
    let canonical_effort = crate::catalog::canonical_reasoning_level(effort_text);
    matches!(
        canonical_effort.as_str(),
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
    )
    .then_some(canonical_effort)
}
