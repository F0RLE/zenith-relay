use super::IMAGE_API_MODEL;
use crate::{is_valid_model_id, pricing::PricingCatalog, Error, Result};
use std::cmp::Ordering as CmpOrdering;
use std::collections::BTreeSet;

pub(crate) fn is_image_model_id(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model.starts_with("gpt-image-") || model.starts_with("dall-e-")
}

pub fn normalize_image_base_model(value: Option<String>) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    if !is_valid_model_id(value) {
        return Err(Error::Validation(
            "image base model id is invalid".to_string(),
        ));
    }
    Ok(Some(value.to_string()))
}

#[cfg(test)]
pub(super) fn select_image_main_model(
    models: &BTreeSet<String>,
    preferred: Option<&str>,
) -> Option<String> {
    select_image_main_model_with_catalog(models, preferred, None)
}

pub(super) fn select_image_main_model_with_catalog(
    models: &BTreeSet<String>,
    preferred: Option<&str>,
    catalog: Option<&PricingCatalog>,
) -> Option<String> {
    match preferred
        .map(str::trim)
        .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("auto"))
    {
        Some(preferred) => models
            .iter()
            .find(|model| {
                model.eq_ignore_ascii_case(preferred)
                    && !model.eq_ignore_ascii_case(IMAGE_API_MODEL)
            })
            .cloned(),
        None => cheapest_image_main_model_with_catalog(models, catalog),
    }
}

#[cfg(test)]
pub(super) fn cheapest_image_main_model(models: &BTreeSet<String>) -> Option<String> {
    cheapest_image_main_model_with_catalog(models, None)
}

pub(super) fn cheapest_image_main_model_with_catalog(
    models: &BTreeSet<String>,
    catalog: Option<&PricingCatalog>,
) -> Option<String> {
    models
        .iter()
        .filter(|model| image_auto_model_is_supported(model))
        .min_by(|left, right| compare_image_main_models(left, right, catalog))
        .cloned()
}

fn image_main_model_is_compatible(model: &str) -> bool {
    let lower = model.trim().to_ascii_lowercase();
    !lower.is_empty()
        && lower != IMAGE_API_MODEL
        && [
            "image",
            "embedding",
            "moderation",
            "realtime",
            "transcribe",
            "tts",
            "audio",
        ]
        .iter()
        .all(|excluded| !lower.contains(excluded))
}

fn image_auto_model_is_supported(model: &str) -> bool {
    // OpenAI's image-generation guide currently requires GPT-5 or newer for the Responses tool.
    let lower = model.trim().to_ascii_lowercase();
    let Some(version) = lower.strip_prefix("gpt-") else {
        return false;
    };
    let major = version
        .split(|character: char| !character.is_ascii_digit())
        .next()
        .and_then(|value| value.parse::<u32>().ok());
    major.is_some_and(|major| major >= 5) && image_main_model_is_compatible(model)
}

fn compare_image_main_models(
    left: &str,
    right: &str,
    catalog: Option<&PricingCatalog>,
) -> CmpOrdering {
    let left_price = catalog.and_then(|catalog| catalog.official_token_price(left, "openai"));
    let right_price = catalog.and_then(|catalog| catalog.official_token_price(right, "openai"));
    match (left_price, right_price) {
        (Some(left_price), Some(right_price)) => left_price
            .input
            .cmp(&right_price.input)
            .then_with(|| left_price.output.cmp(&right_price.output))
            .then_with(|| left.cmp(right)),
        (Some(_), None) => CmpOrdering::Less,
        (None, Some(_)) => CmpOrdering::Greater,
        (None, None) => CmpOrdering::Equal,
    }
    .then_with(|| left.len().cmp(&right.len()))
    .then_with(|| left.cmp(right))
}
