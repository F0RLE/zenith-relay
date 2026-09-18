use zenith_relay_core::error_codes;
mod accounts;
mod automations;
mod error;
mod gateway;
mod imports;
mod keys;
mod models;
mod pricing;
mod proxies;
mod quota;
mod routing;
mod sources;
mod usage;

pub use error::ManagementError;

use crate::config_presets::{self, PresetError};
use crate::state::{AppState, ServerAccountRecord};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use std::sync::Arc;
use zenith_relay_core::protocol::{
    AccountSummary, ConfigurationPresetApplyInput, ConfigurationPresetApplyResult,
    ConfigurationPresetDocument, ConfigurationPresetPreview, ConfigurationPresetPreviewInput,
    HealthResponse, RuntimeStateSnapshot,
};
use zenith_relay_core::CandidateRuntimeSnapshot;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/capabilities", get(capabilities))
        .route("/state", get(state_snapshot))
        .route("/configuration/preset", get(configuration_preset))
        .route(
            "/configuration/preset/preview",
            post(preview_configuration_preset),
        )
        .route(
            "/configuration/preset/apply",
            post(apply_configuration_preset),
        )
        .route("/routing/runtime", get(runtime_order))
        .merge(sources::routes())
        .merge(accounts::routes())
        .merge(imports::routes())
        .merge(proxies::routes())
        .merge(keys::routes())
        .merge(quota::routes())
        .merge(pricing::routes())
        .merge(routing::routes())
        .merge(models::routes())
        .merge(usage::routes())
        .merge(gateway::routes())
        .merge(automations::routes())
}

const MAX_SECRET_BYTES: usize = 64 * 1024;

pub async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        server_id: state.capabilities.server_id.clone(),
        started_at_ms: state.started_at_ms,
    })
}

pub async fn capabilities(
    State(state): State<Arc<AppState>>,
) -> Json<zenith_relay_core::protocol::Capabilities> {
    Json(state.capabilities.clone())
}

pub async fn state_snapshot(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    if let Ok(Some(runtime)) = state.runtime() {
        runtime.prefetch_source_model_metadata();
    }
    Ok(Json(state.snapshot().map_err(store_error)?))
}

pub async fn configuration_preset(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ConfigurationPresetDocument>, ManagementError> {
    config_presets::document(&state)
        .map(Json)
        .map_err(preset_error)
}

pub async fn preview_configuration_preset(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ConfigurationPresetPreviewInput>,
) -> Result<Json<ConfigurationPresetPreview>, ManagementError> {
    config_presets::preview(&state, input.preset)
        .map(Json)
        .map_err(preset_error)
}

pub async fn apply_configuration_preset(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ConfigurationPresetApplyInput>,
) -> Result<Json<ConfigurationPresetApplyResult>, ManagementError> {
    config_presets::apply(&state, input)
        .await
        .map(Json)
        .map_err(preset_error)
}

pub async fn runtime_order(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<CandidateRuntimeSnapshot>>, ManagementError> {
    Ok(Json(state.runtime_order().map_err(runtime_error)?))
}

fn find_account(state: &AppState, id: &str) -> Result<ServerAccountRecord, ManagementError> {
    state
        .store
        .accounts()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::ACCOUNT_NOT_FOUND, "account not found")
        })
}

fn account_summary(
    state: &AppState,
    record: &ServerAccountRecord,
) -> Result<AccountSummary, ManagementError> {
    state
        .snapshot()
        .map_err(store_error)?
        .accounts
        .into_iter()
        .find(|value| value.id == record.id)
        .ok_or_else(|| {
            ManagementError::internal(error_codes::SNAPSHOT_MISSING, "account snapshot missing")
        })
}

fn validate_secret(value: &str, name: &str) -> Result<(), ManagementError> {
    if value.is_empty()
        || value.len() > MAX_SECRET_BYTES
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(validation_error(format!("{name} is invalid")))
    } else {
        Ok(())
    }
}

fn clean_label(value: &str, name: &str) -> Result<String, ManagementError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        Err(validation_error(format!("{name} is invalid")))
    } else {
        Ok(value.to_string())
    }
}

pub(super) use zenith_relay_core::normalize_model_ids as normalized_values;

fn default_weight() -> u32 {
    1
}

fn valid_weight(value: u32) -> Result<u32, ManagementError> {
    (value > 0)
        .then_some(value)
        .ok_or_else(|| validation_error("weight must be positive"))
}

fn validation_error(message: impl Into<String>) -> ManagementError {
    ManagementError::validation(error_codes::INVALID_REQUEST, message)
}

fn preset_error(error: PresetError) -> ManagementError {
    match error {
        PresetError::Invalid(message) => {
            ManagementError::validation(error_codes::CONFIGURATION_PRESET_INVALID, message)
        }
        PresetError::Missing(message) => ManagementError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            error_codes::CONFIGURATION_REFERENCE_MISSING,
            message,
            "configuration",
            false,
        ),
        PresetError::Stale(current_revision) => ManagementError::new(
            StatusCode::CONFLICT,
            error_codes::CONFIGURATION_REVISION_STALE,
            format!("server configuration changed; current revision is {current_revision}"),
            "configuration",
            true,
        ),
        PresetError::Store(_) => ManagementError::internal(
            error_codes::CONFIGURATION_STORE_FAILED,
            "server configuration could not be read or saved",
        ),
        PresetError::Runtime(_) => ManagementError::internal(
            error_codes::CONFIGURATION_RUNTIME_FAILED,
            "server configuration could not be activated",
        ),
    }
}

fn store_error(_error: String) -> ManagementError {
    ManagementError::internal(error_codes::STORE_FAILED, "server storage operation failed")
}

fn vault_error(_error: String) -> ManagementError {
    ManagementError::internal(
        error_codes::VAULT_FAILED,
        "encrypted secret storage operation failed",
    )
}

fn runtime_error(_error: String) -> ManagementError {
    ManagementError::internal(
        error_codes::RUNTIME_RELOAD_FAILED,
        "runtime could not reload the new state",
    )
}
