use super::is_valid_model_id;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ADVERTISED_CONTEXT_WINDOW: u64 = 16_000_000;
const MAX_REASONING_EFFORT_LENGTH: usize = 64;
const MAX_REASONING_DESCRIPTION_LENGTH: usize = 256;
const MAX_MODEL_REASONING_LEVELS: usize = 64;

mod images;

pub(crate) use images::source_image_input_capabilities;
pub use images::source_model_declares_image_input;

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
    fn empty() -> Self {
        Self {
            levels: Vec::new(),
            default_effort: None,
            supports_summary_parameter: false,
            supports_summaries: false,
            default_summary: "none".to_string(),
        }
    }

    pub(crate) fn effort_ids(&self) -> impl Iterator<Item = &str> {
        self.levels.iter().map(|level| level.effort.as_str())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }

    /// Adds Relay's Anthropic-only top tier when a provider has published
    /// `max`. The upstream adapter translates `ultra` back to `max`.
    pub(crate) fn apply_model_implied_efforts(&mut self, model: &str) {
        if !crate::anthropic_max_implies_ultra(model)
            || !self
                .levels
                .iter()
                .any(|level| level.effort.eq_ignore_ascii_case("max"))
            || self
                .levels
                .iter()
                .any(|level| level.effort.eq_ignore_ascii_case("ultra"))
        {
            return;
        }
        self.levels.push(SourceReasoningLevel {
            effort: "ultra".to_string(),
            description: "ultra".to_string(),
        });
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

/// Reads reasoning modes declared by a provider/Gateway catalog.
///
/// Declared modes are the default enabled set. They are still operator
/// editable, and the explicit Relay probe is only an additional manual
/// diagnostic; it is not required before a declared mode can be used.
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

pub(crate) fn source_reasoning_probe_progress(
    manifest: &Value,
    configured_models: &BTreeSet<String>,
) -> BTreeMap<String, SourceReasoningProbeProgress> {
    source_catalog_model_rows(manifest)
        .filter_map(|model| {
            let object = model.as_object()?;
            let id = source_catalog_model_id(object)?.trim();
            if !configured_models
                .iter()
                .any(|configured| configured.eq_ignore_ascii_case(id))
            {
                return None;
            }
            let probe = object
                .get("reasoningProbe")
                .or_else(|| object.get("reasoning_probe"))
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())?;
            Some((id.to_ascii_lowercase(), probe))
        })
        .collect()
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

/// Combines modes published as confirmed by one or more Gateway routes.
///
/// A route without an explicit publication is intentionally absent. It stays
/// eligible for ordinary requests, but cannot add a reasoning selector.
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

    // An explicit empty declaration is meaningful. Keep it through the
    // merge so the Codex catalog can suppress the model registry fallback.
    // `None` still means that no route supplied reasoning metadata at all.
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
        if !is_valid_model_id(model) {
            return Err("model reasoning allowed levels are invalid");
        }
        if levels.len() > MAX_MODEL_REASONING_LEVELS {
            return Err("model reasoning allowed levels are invalid");
        }
        let mut model_levels = Vec::new();
        for level in levels {
            let level = level.trim().to_ascii_lowercase();
            if !valid_reasoning_effort(&level) {
                return Err("model reasoning allowed levels are invalid");
            }
            model_levels.push(level);
        }
        // An explicit empty list is meaningful: it is the user's override
        // that disables every provider-reported mode for this model.
        normalized.insert(
            model.to_ascii_lowercase(),
            crate::canonicalize_reasoning_levels(model_levels),
        );
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
    let raw_modes = model
        .get("reasoningEffortModes")
        .or_else(|| model.get("reasoning_effort_modes"))
        .and_then(Value::as_array)?;
    // Probe state is diagnostic and must never gate the route or defaults.
    // A missing field means that the provider did not publish reasoning
    // metadata. An explicitly empty array is a real declaration that this
    // route has no reasoning modes, even when no probe object is present.
    if raw_modes.is_empty() {
        return Some(SourceReasoningCapabilities::empty());
    }
    let mut published = Map::new();
    published.insert(
        "reasoningEffortModes".to_string(),
        Value::Array(raw_modes.clone()),
    );
    for (target, aliases) in [
        (
            "default_effort",
            [
                "default_effort",
                "default_reasoning_level",
                "defaultReasoningLevel",
            ]
            .as_slice(),
        ),
        (
            "supports_summary_parameter",
            [
                "supports_summary_parameter",
                "supports_reasoning_summary_parameter",
                "supportsReasoningSummaryParameter",
            ]
            .as_slice(),
        ),
        (
            "supports_summaries",
            [
                "supports_summaries",
                "supports_reasoning_summaries",
                "supportsReasoningSummaries",
            ]
            .as_slice(),
        ),
        (
            "default_summary",
            [
                "default_summary",
                "default_reasoning_summary",
                "defaultReasoningSummary",
            ]
            .as_slice(),
        ),
    ] {
        if let Some(value) = aliases.iter().find_map(|alias| model.get(*alias)).cloned() {
            published.insert((*target).to_string(), value);
        }
    }
    parse_reasoning_object(&published)
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceReasoningProbeProgress {
    pub status: String,
    pub total: i64,
    pub running: i64,
    pub success: i64,
    pub failed: i64,
    pub confirmed: i64,
    pub rejected: i64,
    pub inconclusive: i64,
    pub pending: i64,
    pub last_probe_at: Option<String>,
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
    fn ignores_provider_reasoning_metadata_without_gateway_evidence() {
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

        assert!(source_reasoning_capabilities(&manifest, &configured).is_empty());
    }

    #[test]
    fn accepts_declared_gateway_modes_without_waiting_for_probe() {
        let configured = ["grok-4.5", "provider/unknown"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let manifest = json!({
            "data": [
                {
                    "id": "grok-4.5",
                    "reasoningEffortModes": ["low", "medium", "high"],
                    "reasoningProbe": {
                        "status": "confirmed",
                        "total": 8,
                        "running": 0,
                        "success": 3,
                        "failed": 5,
                        "confirmed": 3,
                        "rejected": 5,
                        "inconclusive": 0,
                        "pending": 0,
                        "lastProbeAt": "2026-08-19T00:00:00Z"
                    }
                },
                {
                    "id": "provider/unknown",
                    "reasoningEffortModes": ["high"],
                    "reasoningProbe": {
                        "status": "running",
                        "total": 8,
                        "running": 1,
                        "success": 0,
                        "failed": 0,
                        "confirmed": 0,
                        "rejected": 0,
                        "inconclusive": 0,
                        "pending": 7,
                        "lastProbeAt": null
                    }
                }
            ]
        });

        let capabilities = source_reasoning_capabilities(&manifest, &configured);

        assert_eq!(capabilities.len(), 2);
        assert_eq!(
            capabilities["grok-4.5"].codex_catalog_template(),
            json!({
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "low"},
                    {"effort": "medium", "description": "medium"},
                    {"effort": "high", "description": "high"}
                ],
                "default_reasoning_level": "medium"
            })
            .as_object()
            .unwrap()
            .clone()
        );
        assert_eq!(
            capabilities["provider/unknown"]
                .effort_ids()
                .collect::<Vec<_>>(),
            vec!["high"]
        );
    }

    #[test]
    fn explicit_rejection_publishes_an_empty_capability() {
        let configured = ["glm-5.2"].into_iter().map(str::to_string).collect();
        let manifest = json!({
            "models": [{
                "slug": "glm-5.2",
                "reasoningEffortModes": [],
                "reasoningProbe": {
                    "status": "rejected",
                    "total": 8,
                    "running": 0,
                    "success": 0,
                    "failed": 8,
                    "confirmed": 0,
                    "rejected": 8,
                    "inconclusive": 0,
                    "pending": 0,
                    "lastProbeAt": "2026-08-19T00:00:00Z"
                }
            }]
        });

        let capabilities = source_reasoning_capabilities(&manifest, &configured);
        assert!(capabilities.contains_key("glm-5.2"));
        assert!(capabilities["glm-5.2"].effort_ids().next().is_none());
        assert_eq!(
            capabilities["glm-5.2"].codex_catalog_template(),
            json!({"supported_reasoning_levels": []})
                .as_object()
                .unwrap()
                .clone()
        );
    }

    #[test]
    fn explicit_empty_modes_without_probe_are_not_treated_as_missing_metadata() {
        let configured = ["gpt-5.6-terra"].into_iter().map(str::to_string).collect();
        let manifest = json!({
            "data": [{
                "id": "gpt-5.6-terra",
                "reasoningEffortModes": []
            }]
        });

        let capabilities = source_reasoning_capabilities(&manifest, &configured);
        assert!(capabilities.contains_key("gpt-5.6-terra"));
        assert!(capabilities["gpt-5.6-terra"].is_empty());
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
    fn model_names_never_create_reasoning_capabilities() {
        let configured = ["provider/gpt-future", "provider/claude-future"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let manifest = json!({
            "data": [{"id": "provider/gpt-future"}, {"id": "provider/claude-future"}]
        });
        assert!(source_reasoning_capabilities(&manifest, &configured).is_empty());
    }

    #[test]
    fn source_declared_efforts_preserve_every_mode_and_use_medium_auto_default() {
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

        let capabilities = declared.expect("source-declared reasoning levels");

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
            .clone()
        );
    }

    #[test]
    fn anthropic_max_adds_relay_ultra_to_the_source_catalog() {
        let mut capabilities = parse_reasoning_object(
            json!({
                "reasoningEffortModes": ["low", "medium", "high", "max"]
            })
            .as_object()
            .unwrap(),
        )
        .expect("source-declared reasoning levels");

        capabilities.apply_model_implied_efforts("claude-fable-5");

        assert_eq!(
            capabilities
                .effort_ids()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>(),
            vec!["low", "medium", "high", "max", "ultra"]
        );
    }
}
