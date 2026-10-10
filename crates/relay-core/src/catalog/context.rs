use super::is_valid_model_id;
use serde::{de::Error as _, Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

const MAX_ADVERTISED_CONTEXT_WINDOW: u64 = 16_000_000;
const MAX_REASONING_EFFORT_LENGTH: usize = 64;
const MAX_MODEL_REASONING_LEVELS: usize = 64;

pub fn normalize_model_reasoning_allowed_levels(
    allowed_levels: BTreeMap<String, Vec<String>>,
) -> Result<BTreeMap<String, Vec<String>>, &'static str> {
    let mut normalized = BTreeMap::new();
    for (model_id, levels) in allowed_levels {
        let model_id = model_id.trim();
        if !is_valid_model_id(model_id) {
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
            crate::model_id_key(model_id),
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

    let raw_levels_by_model = BTreeMap::<String, RawLevels>::deserialize(deserializer)?;
    let allowed_levels = raw_levels_by_model
        .into_iter()
        .filter_map(|(model, levels)| match levels {
            RawLevels::Levels(levels) => Some((model, levels)),
            RawLevels::LegacyDefault(default) if default.eq_ignore_ascii_case("auto") => None,
            RawLevels::LegacyDefault(default) => Some((model, vec![default])),
        })
        .collect();
    normalize_model_reasoning_allowed_levels(allowed_levels).map_err(D::Error::custom)
}

pub(crate) fn context_window(limit_value: &Value) -> Option<u64> {
    limit_value
        .as_u64()
        .or_else(|| limit_value.as_str()?.parse().ok())
        .filter(|window| (1..=MAX_ADVERTISED_CONTEXT_WINDOW).contains(window))
}

fn valid_reasoning_effort(effort_text: &str) -> bool {
    !effort_text.is_empty()
        && effort_text.len() <= MAX_REASONING_EFFORT_LENGTH
        && !effort_text.chars().any(char::is_control)
}

pub fn source_model_declares_image_input(model_record: &Map<String, Value>) -> Option<bool> {
    if let Some(input_modalities) = model_record
        .get("modalities")
        .and_then(|modalities| modalities.get("input"))
    {
        return Some(array_contains_image(input_modalities));
    }
    for key in [
        "input_modalities",
        "inputModalities",
        "input_types",
        "inputTypes",
    ] {
        if let Some(modality_value) = model_record.get(key) {
            return Some(array_contains_image(modality_value));
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
        if let Some(supports_image) = model_record.get(key).and_then(Value::as_bool) {
            return Some(supports_image);
        }
    }
    model_record
        .get("capabilities")
        .and_then(Value::as_object)
        .and_then(source_model_declares_image_input)
}

fn array_contains_image(modality_value: &Value) -> bool {
    modality_value.as_array().is_some_and(|modality_values| {
        modality_values.iter().any(|modality_value| {
            modality_value.as_str().is_some_and(|modality_name| {
                matches!(
                    modality_name.to_ascii_lowercase().as_str(),
                    "image" | "image_url" | "vision"
                )
            })
        })
    })
}
