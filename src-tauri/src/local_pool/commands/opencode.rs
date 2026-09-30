use crate::{
    launcher::restart_opencode,
    local_pool::{
        error::{CommandError, ErrorCode, LocalPoolError},
        models::ProviderSourceRecord,
        state::DesktopState,
        store::secret_store,
    },
    platform::default_opencode_config_path,
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{collections::HashSet, fs};
use tauri::State;
use zenith_relay_core::{
    model_metadata::ModelMetadataCatalog, protocol::ModelSummary, SourceAdapter, WireApi,
};

const PROVIDER_ID: &str = "zenith-relay";
// OpenCode uses the official OpenAI AI SDK so requests use Relay's Responses
// contract, which preserves tool calls across adapters.
const PROVIDER_NPM: &str = "@ai-sdk/openai";

mod config;
mod models;
mod protocols;
mod refresh;

use config::*;
use models::*;
pub(in crate::local_pool) use refresh::refresh_active_opencode_catalog;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeConfigStatus {
    pub configured: bool,
    pub model_count: usize,
    pub has_backup: bool,
    pub backup_created_at_ms: Option<u64>,
    pub backup_name: Option<String>,
    pub path: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeConnectionResult {
    pub path: String,
    pub model_count: usize,
    pub backup_created: bool,
}

fn config_status_for(state: &DesktopState) -> Result<OpenCodeConfigStatus, LocalPoolError> {
    let path = default_opencode_config_path();
    let config = read_config(&path)?;
    let providers = config
        .get("provider")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|providers| providers.iter())
        .filter(|(id, _)| protocols::managed_id(id))
        .collect::<Vec<_>>();
    let model_count = providers
        .iter()
        .filter_map(|(_, value)| value.get("models"))
        .filter_map(Value::as_object)
        .map(Map::len)
        .sum();
    let has_backup = backup_path(state).exists() || missing_marker_path(state).exists();
    Ok(OpenCodeConfigStatus {
        configured: !providers.is_empty(),
        model_count,
        has_backup,
        backup_created_at_ms: backup_created_at_ms(state),
        backup_name: has_backup.then(|| backup_name(state)).flatten(),
        path: path.display().to_string(),
    })
}

#[tauri::command]
pub fn get_opencode_config_status(
    state: State<'_, DesktopState>,
) -> Result<OpenCodeConfigStatus, CommandError> {
    config_status_for(&state).map_err(Into::into)
}

/// Preserve the exact OpenCode file before Relay changes it. The operation is
/// intentionally one-shot: an explicit snapshot must never replace the
/// original recovery point created before the first Relay write.
#[tauri::command]
pub async fn create_opencode_snapshot(
    state: State<'_, DesktopState>,
    name: String,
) -> Result<bool, CommandError> {
    let _mutation = state.setup_guard().await;
    let name = normalize_snapshot_name(&name).map_err(CommandError::from)?;
    backup_original_config(&state, &default_opencode_config_path(), Some(&name)).map_err(Into::into)
}

#[tauri::command]
pub async fn connect_opencode_to_local_gateway(
    state: State<'_, DesktopState>,
) -> Result<OpenCodeConnectionResult, CommandError> {
    let _mutation = state.setup_guard().await;
    let path = default_opencode_config_path();
    let prepared = super::state::build_local_runtime_state(&state)
        .await
        .map_err(|error| LocalPoolError::new(error.code, error.message))?;
    let key = super::pool::ensure_system_gateway_key(&state)?;
    if !key.enabled || !super::pool::has_usable_pool_candidate(&state)? {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "managed pool is not available for any enabled candidate",
        )
        .into());
    }
    let secret = super::pool::ensure_local_gateway_key_secret(&key)?;
    let models = model_ids(&prepared.gateway.models);
    let mut config = read_config(&path)?;
    let backup_created = backup_original_config(&state, &path, None)?;
    apply_managed_provider(&mut config, &prepared.gateway.base_url, &secret, &models)?;
    write_config(&path, &config)?;
    Ok(OpenCodeConnectionResult {
        path: path.display().to_string(),
        model_count: models.len(),
        backup_created,
    })
}

/// Configure OpenCode to use one API source directly. This is intentionally
/// separate from the pool connection: selecting a source in the Connections
/// table must not silently fall back to the local pool and must preserve the
/// source's exact endpoint and credential.
#[tauri::command]
pub async fn launch_opencode_source(
    source_id: String,
    state: State<'_, DesktopState>,
) -> Result<OpenCodeConnectionResult, CommandError> {
    let _mutation = state.setup_guard().await;
    let source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    if !source.enabled {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source must be enabled before launching OpenCode",
        )
        .into());
    }
    let models = source_opencode_models(&source)?;
    let secret = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    let path = default_opencode_config_path();
    let mut config = read_config(&path)?;
    let backup_created = backup_original_config(&state, &path, None)?;
    protocols::apply_source(
        &mut config,
        &source,
        &secret,
        &state.model_metadata_catalog(),
        true,
    )?;
    write_config(&path, &config)?;
    restart_opencode().map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to restart OpenCode: {error}"),
        )
    })?;
    Ok(OpenCodeConnectionResult {
        path: path.display().to_string(),
        model_count: models.len(),
        backup_created,
    })
}

fn source_opencode_models(source: &ProviderSourceRecord) -> Result<Vec<String>, LocalPoolError> {
    let native_models = source
        .effective_protocol_bindings()
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?
        .into_iter()
        .filter(|binding| binding.adapter == SourceAdapter::Native)
        .flat_map(|binding| binding.model_ids)
        .map(|model| zenith_relay_core::model_id_key(&model))
        .collect::<HashSet<_>>();
    let models = zenith_relay_core::normalize_model_ids(
        source
            .models
            .iter()
            .filter(|model| native_models.contains(&zenith_relay_core::model_id_key(model))),
    );
    if models.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source has no compatible native API models",
        ));
    }
    Ok(models)
}

#[tauri::command]
pub fn restart_opencode_app() -> Result<(), CommandError> {
    restart_opencode().map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to restart OpenCode: {error}"),
        )
        .into()
    })
}

#[tauri::command]
pub async fn restore_opencode_config(state: State<'_, DesktopState>) -> Result<bool, CommandError> {
    let _mutation = state.setup_guard().await;
    let path = default_opencode_config_path();
    let backup = backup_path(&state);
    if backup.exists() {
        if !current_config_is_managed(&path)? {
            return Err(LocalPoolError::new(
                ErrorCode::ProfileRestoreBlocked,
                "OpenCode config was changed outside Relay; restore was blocked",
            )
            .into());
        }
        restore_original_config_preserving_user_changes(&backup, &path)?;
        fs::remove_file(&backup)
            .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
        let _ = fs::remove_file(backup_name_path(&state));
        return Ok(true);
    }
    if missing_marker_path(&state).exists() {
        let mut config = read_config(&path)?;
        let managed = remove_managed_configuration(&mut config);
        if managed {
            if config.is_empty() {
                fs::remove_file(&path)
                    .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
            } else {
                write_config(&path, &config)?;
            }
        }
        fs::remove_file(missing_marker_path(&state))
            .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
        let _ = fs::remove_file(backup_name_path(&state));
        return Ok(managed);
    }
    Ok(false)
}

#[cfg(test)]
mod tests;
