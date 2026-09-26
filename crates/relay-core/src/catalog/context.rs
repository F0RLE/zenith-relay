use super::is_valid_model_id;
use serde::{de::Error as _, Deserialize, Deserializer};
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_ADVERTISED_CONTEXT_WINDOW: u64 = 16_000_000;
const MAX_REASONING_EFFORT_LENGTH: usize = 64;
const MAX_MODEL_REASONING_LEVELS: usize = 64;
mod images;
pub use images::source_model_declares_image_input;

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

fn valid_reasoning_effort(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REASONING_EFFORT_LENGTH
        && !value.chars().any(char::is_control)
}
