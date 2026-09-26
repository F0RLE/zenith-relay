use serde_json::{Map, Value};

pub fn source_model_declares_image_input(model: &Map<String, Value>) -> Option<bool> {
    if let Some(input) = model.get("modalities").and_then(|value| value.get("input")) {
        return Some(array_contains_image(input));
    }
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
