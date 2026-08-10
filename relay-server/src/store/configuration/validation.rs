use std::collections::{BTreeMap, HashSet};
use zenith_relay_core::{
    is_valid_model_id, normalize_image_base_model, normalize_model_price_overrides,
    normalize_model_reasoning_allowed_levels, normalize_subscription_plan_order,
    protocol::ConfigurationPresetSettings,
};

const MIN_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 10;
const MAX_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 20;

pub(super) fn validate_configuration_settings(
    settings: &ConfigurationPresetSettings,
) -> Result<(), String> {
    validate_quota_request_timeout(settings.quota.request_timeout_seconds)?;
    validate_routing_policy(
        settings.routing.max_retry_candidates,
        settings.routing.cooldown_after_failures,
    )?;
    if normalize_subscription_plan_order(settings.routing.subscription_plan_order.clone())
        .map_err(str::to_string)?
        != settings.routing.subscription_plan_order
        || normalize_image_base_model(settings.routing.image_base_model.clone())
            .map_err(|error| error.to_string())?
            != settings.routing.image_base_model
        || normalize_model_ids(settings.hidden_models.clone())? != settings.hidden_models
        || normalize_model_price_overrides(settings.model_price_overrides.clone())?
            != settings.model_price_overrides
        || normalize_model_reasoning_allowed_levels(
            settings.model_reasoning_allowed_levels.clone(),
        )? != settings.model_reasoning_allowed_levels
    {
        return Err("configuration preset is not normalized".to_string());
    }

    let mut ids = HashSet::new();
    for rule in &settings.sources {
        if rule.id.is_empty()
            || !ids.insert(("source", rule.id.as_str()))
            || rule.weight == 0
            || rule.recovery_delay_seconds > 24 * 60 * 60
            || rule.name.is_empty()
            || rule.name.len() > 256
            || rule.name.chars().any(char::is_control)
            || url::Url::parse(&rule.base_url).is_err()
            || normalize_model_ids(rule.allowed_models.clone())? != rule.allowed_models
            || normalize_model_ids(rule.excluded_models.clone())? != rule.excluded_models
            || normalize_model_price_overrides(rule.model_price_overrides.clone())?
                != rule.model_price_overrides
        {
            return Err("source preset rule is invalid".to_string());
        }
    }

    for rule in &settings.accounts {
        if rule.id.is_empty()
            || !ids.insert(("account", rule.id.as_str()))
            || rule.weight == 0
            || rule.identity_hint.is_empty()
            || rule.identity_hint.len() > 128
            || rule.identity_hint.chars().any(char::is_control)
            || normalize_model_ids(rule.allowed_models.clone())? != rule.allowed_models
            || normalize_model_ids(rule.excluded_models.clone())? != rule.excluded_models
        {
            return Err("account preset rule is invalid".to_string());
        }
    }
    Ok(())
}

pub(super) fn validate_quota_request_timeout(request_timeout_seconds: u64) -> Result<(), String> {
    if !(MIN_QUOTA_REQUEST_TIMEOUT_SECONDS..=MAX_QUOTA_REQUEST_TIMEOUT_SECONDS)
        .contains(&request_timeout_seconds)
    {
        return Err("quota request timeout is invalid".to_string());
    }
    Ok(())
}

pub(super) fn validate_routing_policy(
    max_retry_candidates: u8,
    cooldown_after_failures: u8,
) -> Result<(), String> {
    if !(1..=8).contains(&max_retry_candidates) {
        return Err("max retry candidates is invalid".to_string());
    }
    if !(1..=8).contains(&cooldown_after_failures) {
        return Err("cooldown after failures is invalid".to_string());
    }
    Ok(())
}

pub(super) fn normalize_model_ids(models: Vec<String>) -> Result<Vec<String>, String> {
    if models.len() > 4_096 {
        return Err("model list exceeds the supported limit".to_string());
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for model in models {
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        if !is_valid_model_id(model) {
            return Err("model id is invalid".to_string());
        }
        if seen.insert(model.to_ascii_lowercase()) {
            normalized.push(model.to_string());
        }
    }
    Ok(normalized)
}

pub(super) fn model_reasoning_allowed_levels_from_metadata(
    current: Option<String>,
    legacy: Option<String>,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    if let Some(current) = current {
        let allowed_levels = serde_json::from_str(&current)
            .map_err(|_| "model reasoning allowed levels are invalid".to_string())?;
        return normalize_model_reasoning_allowed_levels(allowed_levels).map_err(str::to_string);
    }

    let legacy = legacy.map_or(Ok(BTreeMap::new()), |value| {
        serde_json::from_str::<BTreeMap<String, String>>(&value)
            .map_err(|_| "model reasoning overrides are invalid".to_string())
    })?;
    let allowed_levels = legacy
        .into_iter()
        .filter_map(|(model, effort)| {
            (!effort.eq_ignore_ascii_case("auto")).then_some((model, vec![effort]))
        })
        .collect();
    normalize_model_reasoning_allowed_levels(allowed_levels).map_err(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{model_reasoning_allowed_levels_from_metadata, normalize_model_ids};
    use std::collections::BTreeMap;

    #[test]
    fn normalizes_model_ids_without_losing_the_first_display_spelling() {
        assert_eq!(
            normalize_model_ids(vec![" GPT-5 ".to_string(), "gpt-5".to_string()]).unwrap(),
            vec!["GPT-5".to_string()],
        );
    }

    #[test]
    fn migrates_legacy_reasoning_defaults_without_inventing_auto() {
        assert_eq!(
            model_reasoning_allowed_levels_from_metadata(
                None,
                Some(r#"{"gpt-5":"HIGH","gpt-auto":"auto"}"#.to_string()),
            )
            .unwrap(),
            BTreeMap::from([("gpt-5".to_string(), vec!["high".to_string()])]),
        );
    }
}
