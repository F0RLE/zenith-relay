use super::{AdapterError, AdapterResult};
use crate::WireApi;
use crate::model_metadata::{ModelCapabilities, ReasoningMethod};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
/// The internal upstream thinking contract used by a Messages bridge.
/// Persisted source bindings normalize to `Adaptive`; the enum remains part of
/// the adapter contract and focused protocol tests.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessagesReasoningMode {
    #[default]
    Disabled,
    Budget,
    Adaptive,
}

impl MessagesReasoningMode {
    /// Returns whether the Responses-to-Messages bridge can represent the
    /// requested Codex effort on this upstream route.
    ///
    /// The bridge may advertise only efforts it can actually translate. Native
    /// Responses routes do not use this list: they preserve a provider's
    /// confirmed effort value verbatim.
    pub(crate) fn supports_effort(self, effort: &str) -> bool {
        let effort = crate::catalog::canonical_reasoning_level(effort);
        match self {
            _ if effort == "none" => true,
            Self::Disabled => false,
            Self::Budget => reasoning_effort_budget(&effort).is_some(),
            Self::Adaptive => {
                matches!(effort.as_str(), "low" | "medium" | "high" | "xhigh" | "max")
            }
        }
    }
}

/// Relay's explicit effort-to-budget conversion for budget-based bridges.
/// This does not describe a provider's native effort semantics or capabilities.
pub(in crate::protocol::adapter) fn reasoning_effort_budget(effort: &str) -> Option<u64> {
    match crate::catalog::canonical_reasoning_level(effort).as_str() {
        "none" => Some(0),
        "minimal" => Some(1_024),
        "low" => Some(4_096),
        "medium" => Some(8_192),
        "high" => Some(16_384),
        "xhigh" => Some(24_576),
        "max" | "ultra" => Some(32_000),
        _ => None,
    }
}

/// One renderer for both the generic converter and the Responses bridges.
/// Model metadata narrows the configured dialect; missing controls keep provider defaults.
pub(in crate::protocol::adapter) struct ReasoningPolicy<'a> {
    protocol: WireApi,
    mode: MessagesReasoningMode,
    capabilities: Option<&'a ModelCapabilities>,
}

impl<'a> ReasoningPolicy<'a> {
    pub(in crate::protocol::adapter) fn new(
        protocol: WireApi,
        mode: MessagesReasoningMode,
        capabilities: Option<&'a ModelCapabilities>,
    ) -> Self {
        Self {
            protocol,
            mode,
            capabilities,
        }
    }

    fn method(&self) -> Option<&ReasoningMethod> {
        self.capabilities
            .and_then(|capabilities| capabilities.reasoning_method.as_ref())
    }

    fn validate(&self) -> AdapterResult<()> {
        if self
            .capabilities
            .is_some_and(|capabilities| capabilities.reasoning == Some(false))
        {
            return Err(AdapterError::reasoning_unsupported());
        }
        Ok(())
    }

    pub(in crate::protocol::adapter) fn apply_effort(
        &self,
        fields: &mut Map<String, Value>,
        effort: &str,
        explicit_max_tokens: bool,
    ) -> AdapterResult<()> {
        let effort = crate::catalog::canonical_reasoning_level(effort);
        // A model without reasoning already satisfies an explicit disable.
        // Do not emit an unsupported upstream control for this no-op.
        if effort == "none"
            && self
                .capabilities
                .is_some_and(|capabilities| capabilities.reasoning == Some(false))
        {
            return self.apply_disabled(fields);
        }
        self.validate()
            .map_err(|error| error.with_parameter("reasoning.effort"))?;
        if self.capabilities.is_some_and(|capabilities| {
            !capabilities.reasoning_effort_levels.is_empty()
                && !capabilities.reasoning_effort_levels.contains(&effort)
        }) {
            return Err(AdapterError::reasoning_unsupported());
        }
        match self.protocol {
            WireApi::GeminiInteractions => return Err(AdapterError::reasoning_unsupported()),
            WireApi::Responses => {
                fields.insert("reasoning".into(), json!({"effort": effort}));
            }
            WireApi::ChatCompletions => {
                fields.insert("reasoning_effort".into(), effort.into());
            }
            WireApi::Messages | WireApi::Gemini => {
                if effort == "none" {
                    return self.apply_disabled(fields);
                }
                if self.mode == MessagesReasoningMode::Disabled
                    || self.method() == Some(&ReasoningMethod::Toggle)
                {
                    return Err(AdapterError::reasoning_unsupported());
                }
                if self.method() == Some(&ReasoningMethod::BudgetTokens) {
                    // A model's token limits do not define a mapping from effort levels.
                    // Accept an explicit budget instead of inventing one for that model.
                    return Err(AdapterError::reasoning_unsupported());
                }
                if self.method().is_none() && self.mode == MessagesReasoningMode::Budget {
                    let budget = reasoning_effort_budget(&effort)
                        .ok_or_else(AdapterError::reasoning_unsupported)?;
                    return self.apply_budget(fields, budget, explicit_max_tokens);
                }
                match self.protocol {
                    WireApi::Messages => {
                        if !matches!(effort.as_str(), "low" | "medium" | "high" | "xhigh" | "max") {
                            return Err(AdapterError::reasoning_unsupported());
                        }
                        if self.method() == Some(&ReasoningMethod::Adaptive)
                            || (self.capabilities.is_none()
                                && self.mode == MessagesReasoningMode::Adaptive)
                        {
                            fields.insert("thinking".into(), json!({"type": "adaptive"}));
                        }
                        fields.entry("output_config").or_insert_with(|| json!({}))["effort"] =
                            effort.into();
                    }
                    WireApi::Gemini => {
                        if !matches!(effort.as_str(), "minimal" | "low" | "medium" | "high") {
                            return Err(AdapterError::reasoning_unsupported());
                        }
                        fields.insert("thinkingConfig".into(), json!({"thinkingLevel": effort}));
                    }
                    _ => unreachable!(),
                }
            }
        }
        self.validate_messages_controls(fields, explicit_max_tokens)
    }

    pub(in crate::protocol::adapter) fn apply_budget(
        &self,
        fields: &mut Map<String, Value>,
        budget: u64,
        explicit_max_tokens: bool,
    ) -> AdapterResult<()> {
        if budget == 0
            && self
                .capabilities
                .is_some_and(|capabilities| capabilities.reasoning == Some(false))
        {
            return self.apply_disabled(fields);
        }
        self.validate()?;
        if matches!(
            self.method(),
            Some(ReasoningMethod::Effort | ReasoningMethod::Adaptive | ReasoningMethod::Toggle)
        ) {
            return Err(AdapterError::reasoning_unsupported());
        }
        if self.capabilities.is_some_and(|capabilities| {
            capabilities
                .reasoning_budget_min_tokens
                .is_some_and(|minimum| budget < minimum)
                || capabilities
                    .reasoning_budget_max_tokens
                    .is_some_and(|maximum| budget > maximum)
        }) {
            return Err(AdapterError::reasoning_unsupported());
        }
        match self.protocol {
            WireApi::Messages if budget >= 1_024 => {
                fields.insert(
                    "thinking".into(),
                    json!({"type": "enabled", "budget_tokens": budget}),
                );
            }
            WireApi::Gemini => {
                fields.insert("thinkingConfig".into(), json!({"thinkingBudget": budget}));
            }
            _ => return Err(AdapterError::reasoning_unsupported()),
        }
        self.validate_messages_controls(fields, explicit_max_tokens)
    }

    fn apply_disabled(&self, fields: &mut Map<String, Value>) -> AdapterResult<()> {
        if self
            .capabilities
            .is_some_and(|capabilities| capabilities.reasoning == Some(false))
        {
            return Ok(());
        }
        match self.protocol {
            WireApi::Messages => {
                fields.insert("thinking".into(), json!({"type": "disabled"}));
                Ok(())
            }
            WireApi::Gemini
                if !matches!(
                    self.method(),
                    Some(ReasoningMethod::Effort | ReasoningMethod::Adaptive)
                ) =>
            {
                self.apply_budget(fields, 0, false)
            }
            _ => Err(AdapterError::reasoning_unsupported()),
        }
    }

    pub(in crate::protocol::adapter) fn apply_dynamic_budget(
        &self,
        fields: &mut Map<String, Value>,
    ) -> AdapterResult<()> {
        self.validate()?;
        if self.protocol != WireApi::Gemini
            || matches!(
                self.method(),
                Some(ReasoningMethod::Effort | ReasoningMethod::Adaptive | ReasoningMethod::Toggle)
            )
        {
            return Err(AdapterError::reasoning_unsupported());
        }
        fields.insert("thinkingConfig".into(), json!({"thinkingBudget": -1}));
        Ok(())
    }

    fn validate_messages_controls(
        &self,
        fields: &mut Map<String, Value>,
        explicit_max_tokens: bool,
    ) -> AdapterResult<()> {
        if self.protocol != WireApi::Messages {
            return Ok(());
        }
        let thinking_enabled = fields
            .get("thinking")
            .and_then(|thinking| thinking.get("type"))
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "disabled");
        if thinking_enabled
            && ["temperature", "top_p"]
                .iter()
                .any(|field| fields.get(*field).is_some_and(|value| !value.is_null()))
        {
            return Err(AdapterError::parameter_unsupported());
        }
        if thinking_enabled
            && fields
                .get("tool_choice")
                .and_then(|choice| choice.get("type"))
                .and_then(Value::as_str)
                .is_some_and(|kind| matches!(kind, "any" | "tool"))
        {
            return Err(AdapterError::parameter_unsupported_for("tool_choice"));
        }
        if let Some(budget) = fields
            .get("thinking")
            .and_then(|thinking| thinking.get("budget_tokens"))
            .and_then(Value::as_u64)
        {
            if explicit_max_tokens {
                if fields
                    .get("max_tokens")
                    .and_then(Value::as_u64)
                    .is_none_or(|limit| limit <= budget)
                {
                    return Err(AdapterError::parameter_unsupported_for("max_tokens"));
                }
            } else {
                let limit = budget
                    .checked_add(1_024)
                    .ok_or_else(|| AdapterError::parameter_unsupported_for("max_tokens"))?
                    .max(
                        fields
                            .get("max_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or_default(),
                    );
                if self
                    .capabilities
                    .and_then(|capabilities| capabilities.output_limit)
                    .is_some_and(|maximum| limit > maximum)
                {
                    return Err(AdapterError::parameter_unsupported_for("max_tokens"));
                }
                fields.insert("max_tokens".into(), limit.into());
            }
        }
        Ok(())
    }
}
