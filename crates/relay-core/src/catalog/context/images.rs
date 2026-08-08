use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

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
