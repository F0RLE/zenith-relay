use serde::{de::Error as _, Deserialize, Deserializer};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ADVERTISED_CONTEXT_WINDOW: u64 = 16_000_000;
const MAX_REASONING_EFFORT_LENGTH: usize = 64;
const MAX_REASONING_DESCRIPTION_LENGTH: usize = 256;
const MAX_MODEL_REASONING_LEVELS: usize = 64;
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
/// combines it across routes that can receive the same public model.
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
    pub(crate) fn effort_ids(&self) -> impl Iterator<Item = &str> {
        self.levels.iter().map(|level| level.effort.as_str())
    }

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
        // API sources must not inherit a provider-specific automatic default
        // such as `ultra`. Relay uses the neutral middle option only when it
        // was actually confirmed; native ChatGPT catalog rows never use this
        // template and retain their provider-owned defaults.
        let default_effort = self
            .levels
            .iter()
            .find(|level| level.effort.eq_ignore_ascii_case("medium"))
            .map(|level| &level.effort);
        if let Some(default_effort) = default_effort {
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
/// `data` rows or a top-level `models` catalog. The source API remains
/// provider-neutral: it may expose Relay's canonical
/// `capabilities.reasoning` object, a nested `reasoning` object,
/// Codex-compatible fields, `reasoningEffortModes`, or a catalog of
/// `reasoningEfforts` options.
///
/// An ordinary `/models` response with IDs only returns no capabilities. That
/// is deliberate: Relay must hide the Codex selector rather than infer
/// reasoning support from a model name.
pub(crate) fn source_reasoning_capabilities(
    manifest: &Value,
    configured_models: &BTreeSet<String>,
) -> BTreeMap<String, SourceReasoningCapabilities> {
    let mut capabilities = BTreeMap::new();
    for model in source_catalog_model_rows(manifest) {
        let Some(object) = model.as_object() else {
            continue;
        };
        let Some(id) = source_catalog_model_id(object) else {
            continue;
        };
        if !configured_models
            .iter()
            .any(|configured| configured.eq_ignore_ascii_case(id))
        {
            continue;
        }
        let Some(source_capabilities) = parse_source_reasoning_capabilities(object) else {
            continue;
        };
        // `data` precedes `models`, so a source's canonical OpenAI-style row
        // wins if it and a secondary catalog describe the same model.
        capabilities
            .entry(id.to_ascii_lowercase())
            .or_insert(source_capabilities);
    }
    capabilities
}

fn source_catalog_model_rows(manifest: &Value) -> impl Iterator<Item = &Value> {
    ["data", "models"].into_iter().flat_map(move |key| {
        manifest
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
    })
}

fn source_catalog_model_id(model: &Map<String, Value>) -> Option<&str> {
    ["id", "slug", "model"]
        .into_iter()
        .filter_map(|key| model.get(key).and_then(Value::as_str))
        .map(str::trim)
        .find(|id| !id.is_empty())
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

/// Combines capabilities confirmed by one or more provider routes.
///
/// A route that does not publish metadata is intentionally absent from this
/// input. The request path excludes that route when a client explicitly asks
/// for an effort, so it is safe to expose every level confirmed by at least
/// one route instead of hiding controls for the whole public model.
pub(crate) fn union_source_reasoning_capabilities(
    capabilities: impl IntoIterator<Item = SourceReasoningCapabilities>,
) -> Option<SourceReasoningCapabilities> {
    let mut capabilities = capabilities.into_iter();
    let mut combined = capabilities.next()?;

    for next in capabilities {
        for level in next.levels {
            if combined
                .levels
                .iter()
                .any(|candidate| candidate.effort.eq_ignore_ascii_case(&level.effort))
            {
                continue;
            }
            combined.levels.push(level);
        }
        combined.default_effort = match (&combined.default_effort, &next.default_effort) {
            (Some(current), Some(candidate))
                if current.eq_ignore_ascii_case(candidate)
                    && combined
                        .levels
                        .iter()
                        .any(|level| level.effort.eq_ignore_ascii_case(current)) =>
            {
                Some(current.clone())
            }
            _ => None,
        };
        combined.supports_summary_parameter &= next.supports_summary_parameter;
        combined.supports_summaries &= next.supports_summaries;
        if combined.default_summary != next.default_summary {
            combined.default_summary = "none".to_string();
        }
    }

    if combined.levels.is_empty() {
        return None;
    }
    if !combined.supports_summary_parameter && !combined.supports_summaries {
        combined.default_summary = "none".to_string();
    }
    Some(combined)
}

pub fn normalize_model_reasoning_allowed_levels(
    allowed_levels: BTreeMap<String, Vec<String>>,
) -> Result<BTreeMap<String, Vec<String>>, &'static str> {
    let mut normalized = BTreeMap::new();
    for (model, levels) in allowed_levels {
        let model = model.trim();
        if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
            return Err("model reasoning allowed levels are invalid");
        }
        if levels.len() > MAX_MODEL_REASONING_LEVELS {
            return Err("model reasoning allowed levels are invalid");
        }
        let mut model_levels = BTreeSet::new();
        for level in levels {
            let level = level.trim().to_ascii_lowercase();
            if !valid_reasoning_effort(&level) {
                return Err("model reasoning allowed levels are invalid");
            }
            model_levels.insert(level);
        }
        if !model_levels.is_empty() {
            normalized.insert(
                model.to_ascii_lowercase(),
                model_levels.into_iter().collect(),
            );
        }
    }
    Ok(normalized)
}

/// Reads the v2 one-default format as a one-item allow-list so local state
/// and exported presets remain recoverable after the setting was clarified.
pub fn deserialize_model_reasoning_allowed_levels<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawLevels {
        Levels(Vec<String>),
        LegacyDefault(String),
    }

    let raw = BTreeMap::<String, RawLevels>::deserialize(deserializer)?;
    let allowed_levels = raw
        .into_iter()
        .filter_map(|(model, levels)| match levels {
            RawLevels::Levels(levels) => Some((model, levels)),
            RawLevels::LegacyDefault(default) if default.eq_ignore_ascii_case("auto") => None,
            RawLevels::LegacyDefault(default) => Some((model, vec![default])),
        })
        .collect();
    normalize_model_reasoning_allowed_levels(allowed_levels).map_err(D::Error::custom)
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
    if first_bool(
        model,
        &["supports_reasoning_effort", "supportsReasoningEffort"],
    ) == Some(false)
    {
        return None;
    }
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
    if first_bool(
        value,
        &["supports_reasoning_effort", "supportsReasoningEffort"],
    ) == Some(false)
    {
        return None;
    }
    let parsed_levels = parse_reasoning_levels(value)?;
    if parsed_levels.levels.is_empty() {
        return None;
    }

    let default_effort = first_string(
        value,
        &[
            "default_effort",
            "default_reasoning_level",
            "defaultReasoningLevel",
            "reasoning_effort",
            "reasoningEffort",
        ],
    )
    .filter(|effort| {
        parsed_levels
            .levels
            .iter()
            .any(|level| level.effort.eq_ignore_ascii_case(effort))
    })
    .or(parsed_levels.default_effort.clone());
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
        levels: parsed_levels.levels,
        default_effort,
        supports_summary_parameter,
        supports_summaries,
        default_summary,
    })
}

struct ParsedReasoningLevels {
    levels: Vec<SourceReasoningLevel>,
    default_effort: Option<String>,
}

fn parse_reasoning_levels(value: &Map<String, Value>) -> Option<ParsedReasoningLevels> {
    let raw_levels = value
        .get("supported_reasoning_levels")
        .or_else(|| value.get("supportedReasoningLevels"))
        .or_else(|| value.get("supported_reasoning_efforts"))
        .or_else(|| value.get("supportedReasoningEfforts"))
        .or_else(|| value.get("efforts"))
        .or_else(|| value.get("reasoning_efforts"))
        .or_else(|| value.get("reasoningEfforts"))
        .or_else(|| value.get("reasoning_effort_options"))
        .or_else(|| value.get("reasoningEffortOptions"))
        .or_else(|| value.get("reasoning_effort_modes"))
        .or_else(|| value.get("reasoningEffortModes"))?
        .as_array()?;
    let mut seen = BTreeSet::new();
    let mut default_efforts = Vec::new();
    let mut levels = Vec::with_capacity(raw_levels.len());
    for raw_level in raw_levels {
        let (level, is_default) = parse_reasoning_level(raw_level)?;
        if !seen.insert(level.effort.to_ascii_lowercase()) {
            return None;
        }
        if is_default {
            default_efforts.push(level.effort.clone());
        }
        levels.push(level);
    }
    Some(ParsedReasoningLevels {
        levels,
        default_effort: (default_efforts.len() == 1).then(|| default_efforts.remove(0)),
    })
}

fn parse_reasoning_level(value: &Value) -> Option<(SourceReasoningLevel, bool)> {
    let (effort, description, is_default) = match value {
        Value::String(effort) => (effort.as_str(), effort.as_str(), false),
        Value::Object(level) => {
            let effort = level
                .get("effort")
                .or_else(|| level.get("id"))
                .or_else(|| level.get("value"))
                .and_then(Value::as_str)?;
            let description = level
                .get("description")
                .or_else(|| level.get("label"))
                .and_then(Value::as_str)
                .unwrap_or(effort);
            let is_default =
                first_bool(level, &["default", "is_default", "isDefault"]).unwrap_or(false);
            (effort, description, is_default)
        }
        _ => return None,
    };
    let effort = effort.trim();
    let description = description.trim();
    valid_reasoning_effort(effort)
        .then(|| {
            (
                SourceReasoningLevel {
                    effort: effort.to_string(),
                    description: description.to_string(),
                },
                is_default,
            )
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
    fn reads_explicit_reasoning_effort_options_without_inference() {
        let configured = ["grok-4.5", "provider/unknown"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let manifest = json!({
            "data": [
                {
                    "id": "grok-4.5",
                    "supportsReasoningEffort": true,
                    "reasoningEffort": "high",
                    "reasoningEfforts": [
                        {"value": "low", "label": "Low"},
                        {"value": "medium", "label": "Medium"},
                        {"value": "high", "label": "High", "default": true}
                    ]
                },
                {
                    "id": "provider/unknown",
                    "supportsReasoningEffort": true
                }
            ]
        });

        let capabilities = source_reasoning_capabilities(&manifest, &configured);

        assert_eq!(capabilities.len(), 1);
        assert_eq!(
            capabilities["grok-4.5"].codex_catalog_template(),
            json!({
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "Low"},
                    {"effort": "medium", "description": "Medium"},
                    {"effort": "high", "description": "High"}
                ],
                "default_reasoning_level": "medium"
            })
            .as_object()
            .unwrap()
            .clone()
        );
        assert!(
            !capabilities.contains_key("provider/unknown"),
            "a support flag without exact levels must not invent a selector"
        );
    }

    #[test]
    fn reads_top_level_models_catalog_with_explicit_effort_options() {
        let configured = ["glm-5.2"].into_iter().map(str::to_string).collect();
        let manifest = json!({
            "models": [{
                "slug": "glm-5.2",
                "reasoningEffortOptions": [
                    {"value": "low", "label": "Low"},
                    {"value": "very_high", "label": "Very high", "isDefault": true}
                ]
            }]
        });

        let capabilities = source_reasoning_capabilities(&manifest, &configured);

        assert_eq!(
            capabilities["glm-5.2"].codex_catalog_template(),
            json!({
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "Low"},
                    {"effort": "very_high", "description": "Very high"}
                ],
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }

    #[test]
    fn explicit_reasoning_support_rejection_hides_stale_option_list() {
        let configured = ["provider/fable"].into_iter().map(str::to_string).collect();
        let manifest = json!({
            "data": [{
                "id": "provider/fable",
                "supportsReasoningEffort": false,
                "reasoningEfforts": [{"value": "high", "label": "High"}]
            }]
        });

        assert!(source_reasoning_capabilities(&manifest, &configured).is_empty());
    }

    #[test]
    fn unions_reasoning_confirmed_by_any_route() {
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

        let combined = union_source_reasoning_capabilities([first, second]).unwrap();

        assert_eq!(
            combined.codex_catalog_template(),
            json!({
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "low"},
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
    fn non_claude_models_preserve_every_source_declared_effort_and_use_medium_auto_default() {
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
                    "default_reasoning_level": "medium"
                })
                .as_object()
                .unwrap()
                .clone(),
                "source-declared effort was rewritten for {model_id}"
            );
        }
    }
}
