use crate::{Error, Result, is_valid_model_id};

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
