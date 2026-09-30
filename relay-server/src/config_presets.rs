use crate::{
    state::AppState,
    store::{configuration_revision, ConfigurationReplaceError},
};
use serde_json::Value;
use std::collections::BTreeSet;
use zenith_relay_core::{
    merge_configuration_preset_settings, normalize_configuration_preset,
    protocol::{
        ConfigurationPreset, ConfigurationPresetApplyInput, ConfigurationPresetApplyResult,
        ConfigurationPresetChange, ConfigurationPresetDocument, ConfigurationPresetPreview,
        ConfigurationPresetSettings, CONFIGURATION_PRESET_FORMAT,
        CONFIGURATION_PRESET_SCHEMA_VERSION,
    },
};

mod members;

use members::{resolve_references, validate_references};

#[derive(Debug)]
pub enum PresetError {
    Invalid(String),
    Missing(String),
    Stale(String),
    Store(String),
    Runtime(String),
}

pub fn document(state: &AppState) -> Result<ConfigurationPresetDocument, PresetError> {
    let mut settings = state
        .store
        .configuration_settings()
        .map_err(PresetError::Store)?;
    let revision = configuration_revision(&settings).map_err(PresetError::Store)?;
    settings.routing.pool_routing = Some(settings.resolved_pool_routing());
    Ok(ConfigurationPresetDocument {
        revision,
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

fn normalize_preset(preset: ConfigurationPreset) -> Result<ConfigurationPreset, PresetError> {
    normalize_configuration_preset(preset).map_err(PresetError::Invalid)
}

fn merge_settings(
    current: &ConfigurationPresetSettings,
    requested: &ConfigurationPresetSettings,
) -> Result<ConfigurationPresetSettings, PresetError> {
    merge_configuration_preset_settings(current, requested).map_err(PresetError::Missing)
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
mod tests;
