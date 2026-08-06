use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ADVERTISED_CONTEXT_WINDOW: u64 = 16_000_000;
const MAX_REASONING_EFFORT_LENGTH: usize = 64;
const MAX_REASONING_DESCRIPTION_LENGTH: usize = 256;
const CLAUDE_MANUAL_REASONING_LEVELS: &[(&str, &str)] = &[
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "Extra high"),
    ("max", "Maximum"),
    ("ultra", "Ultra"),
];
const CLAUDE_MANUAL_DEFAULT_REASONING_EFFORT: &str = "medium";

/// Provider-declared reasoning controls for one model on one source route.
///
/// This is intentionally independent of model names and provider names. Relay
/// only creates it from a source's structured `/models` metadata, then
/// intersects it across every route that can receive the same public model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceReasoningCapabilities {
    levels: Vec<SourceReasoningLevel>,
    default_effort: Option<String>,
    supports_summary_parameter: bool,
    supports_summaries: bool,
    default_summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceReasoningLevel {
    effort: String,
    description: String,
}

impl SourceReasoningCapabilities {
    /// Removes efforts that cannot be represented by the selected adapter.
    ///
    /// Native Responses routes preserve provider-defined effort names. A
    /// protocol bridge must opt into only the efforts it can actually map to
    /// the upstream contract.
    pub(crate) fn retain_efforts(&mut self, accepts_effort: impl Fn(&str) -> bool) -> bool {
        self.levels.retain(|level| accepts_effort(&level.effort));
        if self.default_effort.as_ref().is_some_and(|default_effort| {
            !self
                .levels
                .iter()
                .any(|level| level.effort.eq_ignore_ascii_case(default_effort))
        }) {
            self.default_effort = None;
        }
        !self.levels.is_empty()
    }

    /// A Responses-to-Messages bridge does not represent `reasoning.summary`.
    pub(crate) fn clear_summary_capabilities(&mut self) {
        self.supports_summary_parameter = false;
        self.supports_summaries = false;
        self.default_summary = "none".to_string();
    }

    /// Converts the canonical source capability into the narrow set of Codex
    /// model-catalog fields that Relay is allowed to advertise.
    pub(crate) fn codex_catalog_template(&self) -> Map<String, Value> {
        let mut template = Map::new();
        template.insert(
            "supported_reasoning_levels".to_string(),
            Value::Array(
                self.levels
                    .iter()
                    .map(|level| {
                        json!({
                            "effort": level.effort,
                            "description": level.description,
                        })
                    })
                    .collect(),
            ),
        );
        if let Some(default_effort) = &self.default_effort {
            template.insert(
                "default_reasoning_level".to_string(),
                Value::String(default_effort.clone()),
            );
        }
        if self.supports_summary_parameter {
            template.insert(
                "supports_reasoning_summary_parameter".to_string(),
                Value::Bool(true),
            );
        }
        if self.supports_summaries {
            template.insert(
                "supports_reasoning_summaries".to_string(),
                Value::Bool(true),
            );
        }
        if self.supports_summary_parameter || self.supports_summaries {
            template.insert(
                "default_reasoning_summary".to_string(),
                Value::String(self.default_summary.clone()),
            );
        }
        template
    }
}

pub(crate) fn source_context_windows(
    manifest: &Value,
    configured_models: &BTreeSet<String>,
) -> BTreeMap<String, u64> {
    manifest
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("id")?.as_str()?.trim();
            if !configured_models
                .iter()
                .any(|configured| configured.eq_ignore_ascii_case(id))
            {
                return None;
            }
            let context_window = [
                model.get("context_length"),
                model.get("context_window"),
                model.get("max_context_tokens"),
                model.get("max_input_tokens"),
                model.pointer("/top_provider/context_length"),
                model.pointer("/metadata/context_length"),
            ]
            .into_iter()
            .flatten()
            .find_map(context_window)?;
            Some((id.to_ascii_lowercase(), context_window))
        })
        .collect()
}

/// Reads an explicit image-input declaration from a generic source model
/// manifest. Missing metadata is intentionally not treated as support.
pub(crate) fn source_image_input_capabilities(
    manifest: &Value,
    configured_models: &BTreeSet<String>,
) -> BTreeMap<String, bool> {
    manifest
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let object = model.as_object()?;
            let id = object.get("id")?.as_str()?.trim();
            if !configured_models
                .iter()
                .any(|configured| configured.eq_ignore_ascii_case(id))
            {
                return None;
            }
            Some((
                id.to_ascii_lowercase(),
                source_model_declares_image_input(object).unwrap_or(false),
            ))
        })
        .collect()
}

pub fn source_model_declares_image_input(model: &Map<String, Value>) -> Option<bool> {
    for key in [
        "input_modalities",
        "inputModalities",
        "input_types",
        "inputTypes",
    ] {
        if let Some(value) = model.get(key) {
            return Some(array_contains_image(value));
        }
    }
    for key in [
        "supports_vision",
        "supportsVision",
        "supports_images",
        "supportsImages",
        "image_input",
        "imageInput",
    ] {
        if let Some(value) = model.get(key).and_then(Value::as_bool) {
            return Some(value);
        }
    }
    model
        .get("capabilities")
        .and_then(Value::as_object)
        .and_then(source_model_declares_image_input)
}

fn array_contains_image(value: &Value) -> bool {
    value.as_array().is_some_and(|values| {
        values.iter().any(|value| {
            value.as_str().is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "image" | "image_url" | "vision"
                )
            })
        })
    })
}

/// Reads optional, provider-declared reasoning capabilities from OpenAI-style
/// `data` model rows. The source API remains provider-neutral: it may expose
/// Relay's canonical `capabilities.reasoning` object, a nested `reasoning`
/// object, Codex-compatible fields, or the common
/// `reasoningEffortModes` extension.
///
/// An ordinary `/models` response with IDs only returns no capabilities. That
/// is deliberate: Relay must hide the Codex selector rather than infer
/// reasoning support from a model name.
pub(crate) fn source_reasoning_capabilities(
    manifest: &Value,
    configured_models: &BTreeSet<String>,
) -> BTreeMap<String, SourceReasoningCapabilities> {
    manifest
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let object = model.as_object()?;
            let id = object.get("id")?.as_str()?.trim();
            if !configured_models
                .iter()
                .any(|configured| configured.eq_ignore_ascii_case(id))
            {
                return None;
            }
            parse_source_reasoning_capabilities(object)
                .map(|capabilities| (id.to_ascii_lowercase(), capabilities))
        })
        .collect()
}

/// Applies the explicit Claude compatibility fallback for source catalogs
/// that omit some of Claude's known Codex effort levels.
///
/// Provider-declared capabilities remain authoritative for every other model:
/// Relay must not infer a model's effort vocabulary from its name or maintain a
/// blacklist of levels that might become valid later.
///
/// Native ChatGPT rows do not use this function and remain untouched.
pub(crate) fn apply_claude_reasoning_capability_fallback(
    model_id: &str,
    capabilities: Option<SourceReasoningCapabilities>,
) -> Option<SourceReasoningCapabilities> {
    if !is_claude_model_id(model_id) {
        return capabilities;
    }

    let mut capabilities = capabilities.unwrap_or_else(|| SourceReasoningCapabilities {
        levels: Vec::new(),
        default_effort: Some(CLAUDE_MANUAL_DEFAULT_REASONING_EFFORT.to_string()),
        supports_summary_parameter: false,
        supports_summaries: false,
        default_summary: "none".to_string(),
    });
    let mut provider_levels = std::mem::take(&mut capabilities.levels);
    let mut levels = Vec::with_capacity(
        CLAUDE_MANUAL_REASONING_LEVELS
            .len()
            .saturating_add(provider_levels.len()),
    );
    for (effort, description) in CLAUDE_MANUAL_REASONING_LEVELS {
        let provider_level = provider_levels
            .iter()
            .position(|level| level.effort.eq_ignore_ascii_case(effort))
            .map(|index| provider_levels.remove(index));
        levels.push(provider_level.unwrap_or_else(|| SourceReasoningLevel {
            effort: (*effort).to_string(),
            description: (*description).to_string(),
        }));
    }
    levels.append(&mut provider_levels);
    capabilities.levels = levels;
    if capabilities.default_effort.is_none() {
        capabilities.default_effort = Some(CLAUDE_MANUAL_DEFAULT_REASONING_EFFORT.to_string());
    }
    Some(capabilities)
}

/// A public model can be routed through more than one provider source. Only
/// advertise an effort that every currently eligible route proves it can
/// serve; otherwise Codex could select an effort and be routed to a source
/// that rejects it.
pub(crate) fn intersect_source_reasoning_capabilities(
    capabilities: impl IntoIterator<Item = SourceReasoningCapabilities>,
) -> Option<SourceReasoningCapabilities> {
    let mut capabilities = capabilities.into_iter();
    let mut common = capabilities.next()?;

    for next in capabilities {
        common.levels.retain(|level| {
            next.levels
                .iter()
                .any(|candidate| candidate.effort.eq_ignore_ascii_case(&level.effort))
        });
        common.default_effort = match (&common.default_effort, &next.default_effort) {
            (Some(current), Some(candidate))
                if current.eq_ignore_ascii_case(candidate)
                    && common
                        .levels
                        .iter()
                        .any(|level| level.effort.eq_ignore_ascii_case(current)) =>
            {
                Some(current.clone())
            }
            _ => None,
        };
        common.supports_summary_parameter &= next.supports_summary_parameter;
        common.supports_summaries &= next.supports_summaries;
        if common.default_summary != next.default_summary {
            common.default_summary = "none".to_string();
        }
    }

    if common.levels.is_empty() {
        return None;
    }
    if !common.supports_summary_parameter && !common.supports_summaries {
        common.default_summary = "none".to_string();
    }
    Some(common)
}

pub(crate) fn context_window(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|window| (1..=MAX_ADVERTISED_CONTEXT_WINDOW).contains(window))
}

fn parse_source_reasoning_capabilities(
    model: &Map<String, Value>,
) -> Option<SourceReasoningCapabilities> {
    let nested_capabilities = model
        .get("capabilities")
        .and_then(Value::as_object)
        .and_then(|capabilities| capabilities.get("reasoning"))
        .and_then(Value::as_object)
        .and_then(parse_reasoning_object);
    let nested_reasoning = model
        .get("reasoning")
        .and_then(Value::as_object)
        .and_then(parse_reasoning_object);
    nested_capabilities
        .or(nested_reasoning)
        .or_else(|| parse_reasoning_object(model))
}

fn parse_reasoning_object(value: &Map<String, Value>) -> Option<SourceReasoningCapabilities> {
    let levels = parse_reasoning_levels(value)?;
    if levels.is_empty() {
        return None;
    }

    let default_effort = first_string(
        value,
        &[
            "default_effort",
            "default_reasoning_level",
            "defaultReasoningLevel",
        ],
    )
    .filter(|effort| {
        levels
            .iter()
            .any(|level| level.effort.eq_ignore_ascii_case(effort))
    });
    let supports_summary_parameter = first_bool(
        value,
        &[
            "supports_summary_parameter",
            "supports_reasoning_summary_parameter",
            "supportsReasoningSummaryParameter",
        ],
    )
    .unwrap_or(false);
    let supports_summaries = first_bool(
        value,
        &[
            "supports_summaries",
            "supports_reasoning_summaries",
            "supportsReasoningSummaries",
        ],
    )
    .unwrap_or(false);
    let default_summary = first_string(
        value,
        &[
            "default_summary",
            "default_reasoning_summary",
            "defaultReasoningSummary",
        ],
    )
    .filter(|summary| matches!(summary.as_str(), "auto" | "concise" | "detailed" | "none"))
    .unwrap_or_else(|| "none".to_string());

    Some(SourceReasoningCapabilities {
        levels,
        default_effort,
        supports_summary_parameter,
        supports_summaries,
        default_summary,
    })
}

fn parse_reasoning_levels(value: &Map<String, Value>) -> Option<Vec<SourceReasoningLevel>> {
    let raw_levels = value
        .get("supported_reasoning_levels")
        .or_else(|| value.get("supportedReasoningLevels"))
        .or_else(|| value.get("efforts"))
        .or_else(|| value.get("reasoning_effort_modes"))
        .or_else(|| value.get("reasoningEffortModes"))?
        .as_array()?;
    let mut seen = BTreeSet::new();
    let levels = raw_levels
        .iter()
        .filter_map(parse_reasoning_level)
        .filter(|level| seen.insert(level.effort.to_ascii_lowercase()))
        .collect::<Vec<_>>();
    (levels.len() == raw_levels.len()).then_some(levels)
}

fn parse_reasoning_level(value: &Value) -> Option<SourceReasoningLevel> {
    let (effort, description) = match value {
        Value::String(effort) => (effort.as_str(), effort.as_str()),
        Value::Object(level) => {
            let effort = level
                .get("effort")
                .or_else(|| level.get("id"))
                .and_then(Value::as_str)?;
            let description = level
                .get("description")
                .or_else(|| level.get("label"))
                .and_then(Value::as_str)
                .unwrap_or(effort);
            (effort, description)
        }
        _ => return None,
    };
    let effort = effort.trim();
    let description = description.trim();
    valid_reasoning_effort(effort)
        .then(|| SourceReasoningLevel {
            effort: effort.to_string(),
            description: description.to_string(),
        })
        .filter(|_| valid_reasoning_description(description))
}

fn first_string(value: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|candidate| valid_reasoning_effort(candidate))
        .map(str::to_string)
}

fn first_bool(value: &Map<String, Value>, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
}

fn valid_reasoning_effort(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REASONING_EFFORT_LENGTH
        && !value.chars().any(char::is_control)
}

fn valid_reasoning_description(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REASONING_DESCRIPTION_LENGTH
        && !value.chars().any(char::is_control)
}

fn is_claude_model_id(model_id: &str) -> bool {
    let model = model_id.rsplit('/').next().unwrap_or(model_id).trim();
    model.eq_ignore_ascii_case("claude") || model.to_ascii_lowercase().starts_with("claude-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_common_context_fields_only_for_configured_models() {
        let configured = ["claude-fable-5", "grok-4.5", "gemini-3.5-flash"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let manifest = json!({
            "data": [
                {"id": "claude-fable-5", "context_length": 1_000_000},
                {"id": "grok-4.5", "context_length": 0, "context_window": "500000"},
                {"id": "gemini-3.5-flash", "top_provider": {"context_length": 1_048_576}},
                {"id": "not-configured", "context_length": 2_000_000},
                {"id": "invalid", "context_length": 0}
            ]
        });

        assert_eq!(
            source_context_windows(&manifest, &configured),
            BTreeMap::from([
                ("claude-fable-5".into(), 1_000_000),
                ("gemini-3.5-flash".into(), 1_048_576),
                ("grok-4.5".into(), 500_000),
            ])
        );
    }

    #[test]
    fn reads_explicit_image_input_capabilities_without_inferring_from_model_names() {
        let configured = ["provider/vision", "provider/text", "provider/unknown"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let manifest = json!({
            "data": [
                {"id": "provider/vision", "input_modalities": ["text", "image"]},
                {"id": "provider/text", "supports_vision": false},
                {"id": "provider/unknown"}
            ]
        });

        assert_eq!(
            source_image_input_capabilities(&manifest, &configured),
            BTreeMap::from([
                ("provider/text".into(), false),
                ("provider/unknown".into(), false),
                ("provider/vision".into(), true),
            ])
        );
    }

    #[test]
    fn reads_generic_provider_reasoning_metadata_without_model_name_rules() {
        let configured = ["provider/fable", "provider/no-reasoning"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let manifest = json!({
            "data": [
                {
                    "id": "provider/fable",
                    "capabilities": {
                        "reasoning": {
                            "efforts": [
                                {"id": "low", "label": "Low"},
                                {"id": "ultra", "label": "Maximum"}
                            ],
                            "default_effort": "ultra",
                            "supports_summary_parameter": true,
                            "supports_summaries": true,
                            "default_summary": "detailed"
                        }
                    }
                },
                {"id": "provider/no-reasoning"},
                {
                    "id": "not-configured",
                    "reasoningEffortModes": ["high"]
                }
            ]
        });

        let capabilities = source_reasoning_capabilities(&manifest, &configured);

        assert_eq!(capabilities.len(), 1);
        assert_eq!(
            capabilities["provider/fable"].codex_catalog_template(),
            json!({
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "Low"},
                    {"effort": "ultra", "description": "Maximum"}
                ],
                "default_reasoning_level": "ultra",
                "supports_reasoning_summary_parameter": true,
                "supports_reasoning_summaries": true,
                "default_reasoning_summary": "detailed"
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }

    #[test]
    fn intersects_reasoning_only_when_every_route_confirms_the_effort() {
        let first = parse_reasoning_object(
            json!({
                "reasoningEffortModes": ["low", "high", "ultra"],
                "default_effort": "high",
                "supports_summary_parameter": true,
                "supports_summaries": true,
                "default_summary": "detailed"
            })
            .as_object()
            .unwrap(),
        )
        .unwrap();
        let second = parse_reasoning_object(
            json!({
                "reasoningEffortModes": ["high", "ultra"],
                "default_effort": "ultra",
                "supports_summary_parameter": true,
                "supports_summaries": false
            })
            .as_object()
            .unwrap(),
        )
        .unwrap();

        let common = intersect_source_reasoning_capabilities([first, second]).unwrap();

        assert_eq!(
            common.codex_catalog_template(),
            json!({
                "supported_reasoning_levels": [
                    {"effort": "high", "description": "high"},
                    {"effort": "ultra", "description": "ultra"}
                ],
                "supports_reasoning_summary_parameter": true,
                "default_reasoning_summary": "none"
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }

    #[test]
    fn manual_claude_exception_completes_the_codex_effort_set() {
        let declared = parse_reasoning_object(
            json!({
                "reasoningEffortModes": ["low", "medium", "high"],
                "default_effort": "medium"
            })
            .as_object()
            .unwrap(),
        );

        let capabilities =
            apply_claude_reasoning_capability_fallback("vendor/claude-fable-5", declared).unwrap();

        assert_eq!(
            capabilities.codex_catalog_template(),
            json!({
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "low"},
                    {"effort": "medium", "description": "medium"},
                    {"effort": "high", "description": "high"},
                    {"effort": "xhigh", "description": "Extra high"},
                    {"effort": "max", "description": "Maximum"},
                    {"effort": "ultra", "description": "Ultra"}
                ],
                "default_reasoning_level": "medium"
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }

    #[test]
    fn non_claude_models_do_not_receive_manual_efforts_without_metadata() {
        assert!(
            apply_claude_reasoning_capability_fallback("gpt-5.6-sol", None).is_none(),
            "only Claude model IDs receive the manual effort set"
        );
    }

    #[test]
    fn non_claude_models_preserve_every_source_declared_effort() {
        let declared = parse_reasoning_object(
            json!({
                "reasoningEffortModes": [
                    "low",
                    "medium",
                    "high",
                    "xhigh",
                    "max",
                    "ultra",
                    "very_high"
                ],
                "default_effort": "very_high"
            })
            .as_object()
            .unwrap(),
        );

        for model_id in ["grok-4.5", "glm-5.2"] {
            let capabilities =
                apply_claude_reasoning_capability_fallback(model_id, declared.clone())
                    .expect("baseline reasoning levels remain available");

            assert_eq!(
                capabilities.codex_catalog_template(),
                json!({
                    "supported_reasoning_levels": [
                        {"effort": "low", "description": "low"},
                        {"effort": "medium", "description": "medium"},
                        {"effort": "high", "description": "high"},
                        {"effort": "xhigh", "description": "xhigh"},
                        {"effort": "max", "description": "max"},
                        {"effort": "ultra", "description": "ultra"},
                        {"effort": "very_high", "description": "very_high"}
                    ],
                    "default_reasoning_level": "very_high"
                })
                .as_object()
                .unwrap()
                .clone(),
                "source-declared effort was rewritten for {model_id}"
            );
        }
    }
}
