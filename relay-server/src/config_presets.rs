use crate::{
    state::{AccountCredential, AppState},
    store::{configuration_revision, ConfigurationReplaceError},
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use zenith_relay_core::{
    is_valid_model_id, normalize_image_base_model, normalize_model_price_overrides,
    normalize_model_reasoning_allowed_levels, normalize_source_protocol_bindings,
    normalize_subscription_plan_order,
    protocol::{
        AccountPresetRule, ConfigurationPreset, ConfigurationPresetApplyInput,
        ConfigurationPresetApplyResult, ConfigurationPresetChange, ConfigurationPresetDocument,
        ConfigurationPresetPreview, ConfigurationPresetSettings, SourcePresetRule,
        CONFIGURATION_PRESET_FORMAT, CONFIGURATION_PRESET_SCHEMA_VERSION,
    },
    ApiModelPriceOverride, ProxyConfig,
};

#[derive(Debug)]
pub enum PresetError {
    Invalid(String),
    Missing(String),
    Stale(String),
    Store(String),
    Runtime(String),
}

pub fn document(state: &AppState) -> Result<ConfigurationPresetDocument, PresetError> {
    let settings = state
        .store
        .configuration_settings()
        .map_err(PresetError::Store)?;
    Ok(ConfigurationPresetDocument {
        revision: configuration_revision(&settings).map_err(PresetError::Store)?,
        preset: ConfigurationPreset {
            format: CONFIGURATION_PRESET_FORMAT.to_string(),
            schema_version: CONFIGURATION_PRESET_SCHEMA_VERSION,
            settings,
        },
    })
}

pub fn preview(
    state: &AppState,
    preset: ConfigurationPreset,
) -> Result<ConfigurationPresetPreview, PresetError> {
    let mut preset = normalize_preset(preset)?;
    resolve_references(state, &mut preset.settings)?;
    validate_references(state, &preset.settings)?;
    let current = state
        .store
        .configuration_settings()
        .map_err(PresetError::Store)?;
    let target = merge_settings(&current, &preset.settings)?;
    validate_references(state, &target)?;
    Ok(ConfigurationPresetPreview {
        base_revision: configuration_revision(&current).map_err(PresetError::Store)?,
        changes: configuration_diff(&current, &target)?,
        preset,
    })
}

pub async fn apply(
    state: &std::sync::Arc<AppState>,
    input: ConfigurationPresetApplyInput,
) -> Result<ConfigurationPresetApplyResult, PresetError> {
    let _guard = state.configuration_lock.lock().await;
    let preview = preview(state, input.preset)?;
    if preview.base_revision != input.base_revision {
        return Err(PresetError::Stale(preview.base_revision));
    }
    let current = state
        .store
        .configuration_settings()
        .map_err(PresetError::Store)?;
    let target = merge_settings(&current, &preview.preset.settings)?;
    let replacement = state
        .store
        .replace_configuration_if_revision(&input.base_revision, &target)
        .map_err(|error| match error {
            ConfigurationReplaceError::Stale { current_revision } => {
                PresetError::Stale(current_revision)
            }
            ConfigurationReplaceError::Invalid(message) => PresetError::Invalid(message),
            ConfigurationReplaceError::Store(message) => PresetError::Store(message),
        })?;
    state
        .rebuild_runtime_or_rollback(|| state.store.restore_configuration(&replacement.previous))
        .await
        .map_err(PresetError::Runtime)?;
    Ok(ConfigurationPresetApplyResult {
        previous_revision: replacement.previous_revision,
        revision: replacement.revision,
        changes: preview.changes,
    })
}

fn normalize_preset(mut preset: ConfigurationPreset) -> Result<ConfigurationPreset, PresetError> {
    if preset.format != CONFIGURATION_PRESET_FORMAT {
        return Err(PresetError::Invalid(
            "configuration preset format is unsupported".to_string(),
        ));
    }
    if !(2..=CONFIGURATION_PRESET_SCHEMA_VERSION).contains(&preset.schema_version) {
        return Err(PresetError::Invalid(format!(
            "configuration preset schema {} is unsupported",
            preset.schema_version
        )));
    }
    normalize_source_rules(&mut preset.settings.sources)?;
    normalize_account_rules(&mut preset.settings.accounts)?;
    preset.settings.routing.subscription_plan_order =
        normalize_subscription_plan_order(preset.settings.routing.subscription_plan_order)
            .map_err(|message| PresetError::Invalid(message.to_string()))?;
    preset.settings.routing.image_base_model =
        normalize_image_base_model(preset.settings.routing.image_base_model)
            .map_err(|error| PresetError::Invalid(error.to_string()))?;
    if !(1..=8).contains(&preset.settings.routing.max_retry_candidates)
        || !(1..=8).contains(&preset.settings.routing.cooldown_after_failures)
        || !(10..=20).contains(&preset.settings.quota.request_timeout_seconds)
    {
        return Err(PresetError::Invalid(
            "configuration preset policy is invalid".to_string(),
        ));
    }
    preset.settings.hidden_models = normalize_models(preset.settings.hidden_models)?;
    preset.settings.model_price_overrides =
        normalize_prices(preset.settings.model_price_overrides)?;
    preset.settings.model_reasoning_allowed_levels =
        normalize_model_reasoning_allowed_levels(preset.settings.model_reasoning_allowed_levels)
            .map_err(|message| PresetError::Invalid(format!("configuration preset {message}")))?;
    Ok(preset)
}

fn normalize_source_rules(rules: &mut [SourcePresetRule]) -> Result<(), PresetError> {
    if rules.len() > 2_048 {
        return Err(PresetError::Invalid(
            "configuration preset contains too many sources".to_string(),
        ));
    }
    let mut ids = BTreeSet::new();
    for rule in rules.iter_mut() {
        validate_id(&rule.id, "source")?;
        rule.name = rule.name.trim().to_string();
        rule.base_url = rule.base_url.trim().trim_end_matches('/').to_string();
        if !ids.insert(rule.id.clone())
            || rule.weight == 0
            || rule.name.is_empty()
            || rule.name.len() > 256
            || rule.name.chars().any(char::is_control)
            || url::Url::parse(&rule.base_url).is_err()
        {
            return Err(PresetError::Invalid(
                "configuration preset source rule is invalid".to_string(),
            ));
        }
        rule.allowed_models = normalize_models(std::mem::take(&mut rule.allowed_models))?;
        rule.excluded_models = normalize_models(std::mem::take(&mut rule.excluded_models))?;
        rule.model_price_overrides =
            normalize_prices(std::mem::take(&mut rule.model_price_overrides))?;
        if !rule.protocol_bindings.is_empty() {
            rule.protocol_bindings = normalize_source_protocol_bindings(
                std::mem::take(&mut rule.protocol_bindings),
                rule.wire_api,
                &[],
            )
            .map_err(|error| PresetError::Invalid(error.to_string()))?;
        }
    }
    rules.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(())
}

fn normalize_account_rules(rules: &mut [AccountPresetRule]) -> Result<(), PresetError> {
    if rules.len() > 2_048 {
        return Err(PresetError::Invalid(
            "configuration preset contains too many accounts".to_string(),
        ));
    }
    let mut ids = BTreeSet::new();
    for rule in rules.iter_mut() {
        validate_id(&rule.id, "account")?;
        if !ids.insert(rule.id.clone())
            || rule.weight == 0
            || invalid_reference(&rule.identity_hint)
        {
            return Err(PresetError::Invalid(
                "configuration preset account rule is invalid".to_string(),
            ));
        }
        if rule.proxy_id.as_deref().is_some_and(invalid_reference)
            || rule.proxy_id.is_some() && rule.bypass_common_proxy
        {
            return Err(PresetError::Invalid(
                "configuration preset proxy reference is invalid".to_string(),
            ));
        }
        rule.allowed_models = normalize_models(std::mem::take(&mut rule.allowed_models))?;
        rule.excluded_models = normalize_models(std::mem::take(&mut rule.excluded_models))?;
    }
    rules.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(())
}

fn resolve_references(
    state: &AppState,
    settings: &mut ConfigurationPresetSettings,
) -> Result<(), PresetError> {
    let sources = state.store.sources().map_err(PresetError::Store)?;
    for rule in &mut settings.sources {
        let record = sources
            .iter()
            .find(|record| {
                record.id == rule.id
                    && record.wire_api == rule.wire_api
                    && record.base_url.trim_end_matches('/') == rule.base_url
            })
            .or_else(|| {
                let mut matches = sources.iter().filter(|record| {
                    record.wire_api == rule.wire_api
                        && record.base_url.trim_end_matches('/') == rule.base_url
                });
                let first = matches.next();
                if matches.next().is_none() {
                    return first;
                }
                let mut named = sources.iter().filter(|record| {
                    record.name == rule.name
                        && record.wire_api == rule.wire_api
                        && record.base_url.trim_end_matches('/') == rule.base_url
                });
                let first = named.next();
                (named.next().is_none()).then_some(first).flatten()
            })
            .ok_or_else(|| {
                PresetError::Missing(format!(
                    "referenced source {} does not exist or is ambiguous",
                    rule.name
                ))
            })?;
        rule.id = record.id.clone();
        rule.name = record.name.clone();
        rule.base_url = record.base_url.trim_end_matches('/').to_string();
        rule.wire_api = record.wire_api;
    }
    settings
        .sources
        .sort_by(|left, right| left.id.cmp(&right.id));

    let accounts = state.store.accounts().map_err(PresetError::Store)?;
    for rule in &mut settings.accounts {
        let record = accounts
            .iter()
            .find(|record| record.id == rule.id && record.identity_hint == rule.identity_hint)
            .or_else(|| {
                let mut matches = accounts
                    .iter()
                    .filter(|record| record.identity_hint == rule.identity_hint);
                let first = matches.next();
                (matches.next().is_none()).then_some(first).flatten()
            })
            .ok_or_else(|| {
                PresetError::Missing(format!(
                    "referenced account {} does not exist or is ambiguous",
                    rule.identity_hint
                ))
            })?;
        rule.id = record.id.clone();
        rule.identity_hint = record.identity_hint.clone();
    }
    settings
        .accounts
        .sort_by(|left, right| left.id.cmp(&right.id));
    Ok(())
}

fn validate_id(value: &str, kind: &str) -> Result<(), PresetError> {
    if invalid_reference(value) {
        return Err(PresetError::Invalid(format!(
            "configuration preset {kind} reference is invalid"
        )));
    }
    Ok(())
}

fn invalid_reference(value: &str) -> bool {
    value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn normalize_models(models: Vec<String>) -> Result<Vec<String>, PresetError> {
    if models.len() > 4_096 {
        return Err(PresetError::Invalid(
            "configuration preset model list is too large".to_string(),
        ));
    }
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for model in models {
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        if !is_valid_model_id(model) {
            return Err(PresetError::Invalid(
                "configuration preset model id is invalid".to_string(),
            ));
        }
        if seen.insert(model.to_ascii_lowercase()) {
            normalized.push(model.to_string());
        }
    }
    Ok(normalized)
}

fn normalize_prices(
    prices: BTreeMap<String, ApiModelPriceOverride>,
) -> Result<BTreeMap<String, ApiModelPriceOverride>, PresetError> {
    normalize_model_price_overrides(prices)
        .map_err(|message| PresetError::Invalid(format!("configuration preset {message}")))
}

fn validate_references(
    state: &AppState,
    settings: &ConfigurationPresetSettings,
) -> Result<(), PresetError> {
    let sources = state
        .store
        .sources()
        .map_err(PresetError::Store)?
        .into_iter()
        .map(|record| (record.id.clone(), record))
        .collect::<HashMap<_, _>>();
    for rule in &settings.sources {
        let record = sources.get(&rule.id).ok_or_else(|| {
            PresetError::Missing(format!("referenced source {} does not exist", rule.id))
        })?;
        if state
            .vault
            .load(&record.secret_ref)
            .map_err(PresetError::Store)?
            .is_none()
        {
            return Err(PresetError::Missing(format!(
                "referenced source {} has no stored credential",
                rule.id
            )));
        }
    }
    let accounts = state
        .store
        .accounts()
        .map_err(PresetError::Store)?
        .into_iter()
        .map(|record| (record.id.clone(), record))
        .collect::<HashMap<_, _>>();
    for rule in &settings.accounts {
        let record = accounts.get(&rule.id).ok_or_else(|| {
            PresetError::Missing(format!("referenced account {} does not exist", rule.id))
        })?;
        let credential = state
            .vault
            .load(&record.secret_ref)
            .map_err(PresetError::Store)?
            .ok_or_else(|| {
                PresetError::Missing(format!(
                    "referenced account {} has no stored credential",
                    rule.id
                ))
            })?;
        serde_json::from_str::<AccountCredential>(&credential).map_err(|_| {
            PresetError::Missing(format!(
                "referenced account {} has an invalid credential",
                rule.id
            ))
        })?;
    }
    for proxy_id in settings
        .accounts
        .iter()
        .filter_map(|rule| rule.proxy_id.as_deref())
        .chain(settings.quota.common_proxy_id.as_deref())
    {
        let record = state
            .store
            .proxy(proxy_id)
            .map_err(PresetError::Store)?
            .ok_or_else(|| {
                PresetError::Missing(format!("referenced proxy {proxy_id} does not exist"))
            })?;
        let secret = state
            .vault
            .load(&record.secret_ref)
            .map_err(PresetError::Store)?
            .ok_or_else(|| {
                PresetError::Missing(format!("referenced proxy {proxy_id} has no stored secret"))
            })?;
        ProxyConfig::parse(&secret)
            .map_err(|_| PresetError::Missing(format!("referenced proxy {proxy_id} is invalid")))?;
    }
    Ok(())
}

fn merge_settings(
    current: &ConfigurationPresetSettings,
    requested: &ConfigurationPresetSettings,
) -> Result<ConfigurationPresetSettings, PresetError> {
    let mut merged = current.clone();
    let source_indexes = merged
        .sources
        .iter()
        .enumerate()
        .map(|(index, rule)| (rule.id.clone(), index))
        .collect::<HashMap<_, _>>();
    for rule in &requested.sources {
        let index = source_indexes.get(&rule.id).copied().ok_or_else(|| {
            PresetError::Missing(format!("referenced source {} does not exist", rule.id))
        })?;
        merged.sources[index] = rule.clone();
    }
    let account_indexes = merged
        .accounts
        .iter()
        .enumerate()
        .map(|(index, rule)| (rule.id.clone(), index))
        .collect::<HashMap<_, _>>();
    for rule in &requested.accounts {
        let index = account_indexes.get(&rule.id).copied().ok_or_else(|| {
            PresetError::Missing(format!("referenced account {} does not exist", rule.id))
        })?;
        merged.accounts[index] = rule.clone();
    }
    merged.routing = requested.routing.clone();
    merged.quota = requested.quota.clone();
    merged.hidden_models = requested.hidden_models.clone();
    merged.model_price_overrides = requested.model_price_overrides.clone();
    if requested.model_reasoning_allowed_levels_present {
        merged.model_reasoning_allowed_levels = requested.model_reasoning_allowed_levels.clone();
    }
    merged.model_reasoning_allowed_levels_present = true;
    Ok(merged)
}

fn configuration_diff(
    before: &ConfigurationPresetSettings,
    after: &ConfigurationPresetSettings,
) -> Result<Vec<ConfigurationPresetChange>, PresetError> {
    let before = serde_json::to_value(before).map_err(|_| {
        PresetError::Store("configuration preview could not be created".to_string())
    })?;
    let after = serde_json::to_value(after).map_err(|_| {
        PresetError::Store("configuration preview could not be created".to_string())
    })?;
    let mut changes = Vec::new();
    diff_value("", &before, &after, &mut changes);
    Ok(changes)
}

fn diff_value(
    path: &str,
    before: &Value,
    after: &Value,
    changes: &mut Vec<ConfigurationPresetChange>,
) {
    if before == after {
        return;
    }
    match (before, after) {
        (Value::Object(before), Value::Object(after)) => {
            for key in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
                let child = format!("{path}/{}", pointer_segment(key));
                diff_value(
                    &child,
                    before.get(key).unwrap_or(&Value::Null),
                    after.get(key).unwrap_or(&Value::Null),
                    changes,
                );
            }
        }
        (Value::Array(before), Value::Array(after)) if before.len() == after.len() => {
            for (index, (before, after)) in before.iter().zip(after).enumerate() {
                diff_value(&format!("{path}/{index}"), before, after, changes);
            }
        }
        _ => changes.push(ConfigurationPresetChange {
            path: if path.is_empty() {
                "/".to_string()
            } else {
                path.to_string()
            },
            before: before.clone(),
            after: after.clone(),
        }),
    }
}

fn pointer_segment(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::{configuration_diff, merge_settings, normalize_preset};
    use serde_json::json;
    use std::collections::BTreeMap;
    use zenith_relay_core::protocol::{
        ConfigurationPreset, ConfigurationPresetSettings, PresetQuotaPolicy, PresetRoutingPolicy,
        CONFIGURATION_PRESET_FORMAT,
    };

    fn settings() -> ConfigurationPresetSettings {
        ConfigurationPresetSettings {
            sources: Vec::new(),
            accounts: Vec::new(),
            routing: PresetRoutingPolicy {
                max_retry_candidates: 3,
                cooldown_after_failures: 3,
                keep_last_candidate_available: true,
                routing_strategy: Default::default(),
                subscription_plan_order: Vec::new(),
                default_service_tier: Default::default(),
                image_base_model: None,
            },
            quota: PresetQuotaPolicy {
                request_timeout_seconds: 20,
                account_proxy_required: false,
                common_proxy_id: None,
            },
            hidden_models: Vec::new(),
            model_price_overrides: Default::default(),
            model_reasoning_allowed_levels: Default::default(),
            model_reasoning_allowed_levels_present: true,
        }
    }

    #[test]
    fn diff_reports_only_changed_leaf() {
        let before = settings();
        let mut after = before.clone();
        after.routing.max_retry_candidates = 4;

        let changes = configuration_diff(&before, &after).unwrap();

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "/routing/maxRetryCandidates");
    }

    #[test]
    fn preset_rejects_an_invalid_cooldown_threshold() {
        let mut preset = zenith_relay_core::protocol::ConfigurationPreset {
            format: zenith_relay_core::protocol::CONFIGURATION_PRESET_FORMAT.to_string(),
            schema_version: zenith_relay_core::protocol::CONFIGURATION_PRESET_SCHEMA_VERSION,
            settings: settings(),
        };
        preset.settings.routing.cooldown_after_failures = 0;

        assert!(matches!(
            super::normalize_preset(preset),
            Err(super::PresetError::Invalid(_))
        ));
    }

    #[test]
    fn schema_two_omitted_reasoning_levels_preserve_current_configuration() {
        let mut current = settings();
        current.model_reasoning_allowed_levels =
            BTreeMap::from([("gpt-test".to_string(), vec!["medium".to_string()])]);
        let mut omitted_settings = serde_json::to_value(&current).unwrap();
        omitted_settings
            .as_object_mut()
            .unwrap()
            .remove("modelReasoningAllowedLevels");
        let omitted: ConfigurationPreset = serde_json::from_value(json!({
            "format": CONFIGURATION_PRESET_FORMAT,
            "schemaVersion": 2,
            "settings": omitted_settings,
        }))
        .unwrap();

        assert!(!omitted.settings.model_reasoning_allowed_levels_present);
        assert!(serde_json::to_value(&omitted).unwrap()["settings"]
            .get("modelReasoningAllowedLevels")
            .is_none());
        let merged =
            merge_settings(&current, &normalize_preset(omitted).unwrap().settings).unwrap();
        assert_eq!(
            merged.model_reasoning_allowed_levels,
            current.model_reasoning_allowed_levels
        );
        assert!(merged.model_reasoning_allowed_levels_present);

        let mut explicit_settings = serde_json::to_value(&current).unwrap();
        explicit_settings["modelReasoningAllowedLevels"] = json!({});
        let explicit_empty: ConfigurationPreset = serde_json::from_value(json!({
            "format": CONFIGURATION_PRESET_FORMAT,
            "schemaVersion": 2,
            "settings": explicit_settings,
        }))
        .unwrap();

        assert!(
            explicit_empty
                .settings
                .model_reasoning_allowed_levels_present
        );
        assert!(merge_settings(
            &current,
            &normalize_preset(explicit_empty).unwrap().settings
        )
        .unwrap()
        .model_reasoning_allowed_levels
        .is_empty());
    }
}
