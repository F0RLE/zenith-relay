use crate::{Error, Result, is_valid_model_id};

/// Native Codex Images routes supported by Relay, not account entitlement evidence.
/// Keep the default first; explicit model selections are never substituted.
pub const CODEX_IMAGE_MODELS: &[&str] = &[
    "gpt-image-2.6-sunburst",
    "gpt-image-2.6-flare",
    "gpt-image-2.5-sunburst",
    "gpt-image-2.5-flare",
];

/// Default for an Images request that omits its model.
pub const IMAGE_API_MODEL: &str = CODEX_IMAGE_MODELS[0];

pub(crate) fn is_image_model_id(model: &str) -> bool {
    let model_id = crate::model_id_key(model);
    model_id.starts_with("gpt-image-") || model_id.starts_with("dall-e-")
}

/// Preserve import compatibility; native Images no longer selects a text model.
pub fn normalize_image_base_model(model_id: Option<String>) -> Result<Option<String>> {
    let Some(model_id) = model_id else {
        return Ok(None);
    };
    let model_id = model_id.trim();
    if model_id.is_empty() || model_id.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    if !is_valid_model_id(model_id) {
        return Err(Error::Validation(
            "image base model id is invalid".to_string(),
        ));
    }
    Ok(Some(model_id.to_string()))
}
