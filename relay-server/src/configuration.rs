use crate::{
    state::{AccountCredential, AppState},
    store::{
        configuration_revision, ConfigurationReplaceError, MAX_MODEL_PRICE_MICRO_USD_PER_MILLION,
    },
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use zenith_relay_core::{
    normalize_image_base_model, normalize_subscription_plan_order,
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
    if let Err(error) = state.rebuild_runtime().await {
        state
            .store
            .restore_configuration(&replacement.previous)
            .map_err(|rollback| {
                PresetError::Store(format!(
                    "runtime rebuild failed: {error}; configuration rollback failed: {rollback}"
                ))
            })?;
        state.rebuild_runtime().await.map_err(|rollback| {
            PresetError::Runtime(format!(
                "runtime rebuild failed: {error}; previous runtime could not be restored: {rollback}"
            ))
        })?;
        return Err(PresetError::Runtime(error));
    }
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
    if preset.schema_version != CONFIGURATION_PRESET_SCHEMA_VERSION {
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
        || !(120..=3_600).contains(&preset.settings.quota.refresh_interval_seconds)
        || !(10..=20).contains(&preset.settings.quota.request_timeout_seconds)
    {
        return Err(PresetError::Invalid(
            "configuration preset policy is invalid".to_string(),
        ));
    }
    preset.settings.hidden_models = normalize_models(preset.settings.hidden_models)?;
    preset.settings.model_price_overrides =
        normalize_prices(preset.settings.model_price_overrides)?;
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
        if rule.proxy_id.as_deref().is_some_and(invalid_reference) {
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
        if model.len() > 256 || model.chars().any(char::is_control) {
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
    let mut normalized = BTreeMap::new();
    for (model, price) in prices {
        let model = model.trim();
        if model.is_empty()
            || model.len() > 256
            || model.chars().any(char::is_control)
            || price.input_micro_usd_per_million > MAX_MODEL_PRICE_MICRO_USD_PER_MILLION
            || price
                .cached_input_micro_usd_per_million
                .is_some_and(|value| value > MAX_MODEL_PRICE_MICRO_USD_PER_MILLION)
            || price.output_micro_usd_per_million > MAX_MODEL_PRICE_MICRO_USD_PER_MILLION
        {
            return Err(PresetError::Invalid(
                "configuration preset model price is invalid".to_string(),
            ));
        }
        normalized.insert(model.to_ascii_lowercase(), price);
    }
    Ok(normalized)
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
    use super::configuration_diff;
    use zenith_relay_core::protocol::{
        ConfigurationPresetSettings, PresetQuotaPolicy, PresetRoutingPolicy,
    };

    fn settings() -> ConfigurationPresetSettings {
        ConfigurationPresetSettings {
            sources: Vec::new(),
            accounts: Vec::new(),
            routing: PresetRoutingPolicy {
                max_retry_candidates: 3,
                routing_strategy: Default::default(),
                subscription_plan_order: Vec::new(),
                default_service_tier: Default::default(),
                image_base_model: None,
            },
            quota: PresetQuotaPolicy {
                refresh_interval_seconds: 300,
                request_timeout_seconds: 20,
                use_free_accounts: false,
                account_proxy_required: false,
                common_proxy_id: None,
            },
            hidden_models: Vec::new(),
            model_price_overrides: Default::default(),
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
}
