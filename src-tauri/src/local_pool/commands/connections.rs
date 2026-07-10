use super::core_error;
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    models::{LocalGatewayKeyRecord, ProviderSourceRecord},
    state::DesktopState,
    store::secret_store,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;
use uuid::Uuid;
use zenith_relay_core::{GatewayRuntime, LocalGatewayKey, ProviderSource, WireApi};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSourceInput {
    name: String,
    base_url: String,
    api_key: String,
    #[serde(default = "responses_wire_api")]
    wire_api: WireApi,
    #[serde(default)]
    models: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedLocalKey {
    pub key: LocalGatewayKeyRecord,
    pub secret: String,
}

#[tauri::command]
pub async fn create_local_source(
    input: CreateSourceInput,
    state: State<'_, DesktopState>,
) -> Result<ProviderSourceRecord, CommandError> {
    let _setup = state.setup_guard().await;
    if !state.store()?.sources().is_empty() {
        return Err(
            LocalPoolError::new(ErrorCode::Conflict, "a local source already exists").into(),
        );
    }
    let id = format!("source_{}", Uuid::new_v4().simple());
    let secret_ref = format!("source:{id}");
    let mut runtime_source = ProviderSource {
        id: id.clone(),
        name: input.name.trim().to_string(),
        base_url: input.base_url.trim().to_string(),
        api_key: input.api_key.trim().to_string(),
        wire_api: input.wire_api,
        models: input.models,
    };
    runtime_source.validate().map_err(core_error)?;
    let runtime = GatewayRuntime::new(
        runtime_source.clone(),
        LocalGatewayKey {
            id: "discovery".into(),
            secret: "discovery-only-local-key".into(),
        },
        Arc::new(|_| {}),
    )
    .map_err(core_error)?;
    runtime_source.models = runtime.discover_models().await.map_err(core_error)?;
    if runtime_source.models.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "source did not expose any configured models",
        )
        .into());
    }

    secret_store::save(&secret_ref, &runtime_source.api_key)?;
    let record = ProviderSourceRecord {
        id,
        name: runtime_source.name,
        enabled: true,
        base_url: runtime_source.base_url,
        secret_ref: secret_ref.clone(),
        wire_api: runtime_source.wire_api,
        models: runtime_source.models,
        last_test_at: Some(Utc::now().to_rfc3339()),
        last_test_status: Some("ok".into()),
        last_error: None,
    };
    if let Err(error) = state.store()?.upsert_source(record.clone()) {
        if let Err(cleanup_error) = secret_store::delete(&secret_ref) {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!(
                    "{}; source secret cleanup failed: {}",
                    error.message, cleanup_error.message
                ),
            )
            .into());
        }
        return Err(error.into());
    }
    Ok(record)
}

#[tauri::command]
pub async fn test_local_source(
    source_id: String,
    state: State<'_, DesktopState>,
) -> Result<ProviderSourceRecord, CommandError> {
    let source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    let api_key = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    let runtime = GatewayRuntime::new(
        ProviderSource {
            id: source.id.clone(),
            name: source.name.clone(),
            base_url: source.base_url.clone(),
            api_key,
            wire_api: source.wire_api,
            models: source.models.clone(),
        },
        LocalGatewayKey {
            id: "discovery".into(),
            secret: "discovery-only-local-key".into(),
        },
        Arc::new(|_| {}),
    )
    .map_err(core_error)?;
    let models = match runtime.discover_models().await {
        Ok(models) => models,
        Err(error) => {
            let error = core_error(error);
            let mut failed = source.clone();
            failed.last_test_at = Some(Utc::now().to_rfc3339());
            failed.last_test_status = Some("error".into());
            failed.last_error = Some(error.message.clone());
            state.store()?.upsert_source(failed)?;
            return Err(error.into());
        }
    };
    let mut updated = source;
    updated.models = models;
    updated.last_test_at = Some(Utc::now().to_rfc3339());
    updated.last_test_status = Some("ok".into());
    updated.last_error = None;
    state.store()?.upsert_source(updated.clone())?;
    Ok(updated)
}

#[tauri::command]
pub async fn create_local_gateway_key(
    label: String,
    state: State<'_, DesktopState>,
) -> Result<GeneratedLocalKey, CommandError> {
    let _setup = state.setup_guard().await;
    if !state.store()?.keys().is_empty() {
        return Err(
            LocalPoolError::new(ErrorCode::Conflict, "a local gateway key already exists").into(),
        );
    }
    let id = format!("key_{}", Uuid::new_v4().simple());
    let secret = format!("zlr_{}", Uuid::new_v4().simple());
    let secret_ref = format!("key:{id}");
    secret_store::save(&secret_ref, &secret)?;
    let record = LocalGatewayKeyRecord {
        id,
        label: if label.trim().is_empty() {
            "Default".into()
        } else {
            label.trim().to_string()
        },
        enabled: true,
        secret_ref: secret_ref.clone(),
        created_at: Utc::now().to_rfc3339(),
        last_used_at: None,
    };
    if let Err(error) = state.store()?.upsert_key(record.clone()) {
        if let Err(cleanup_error) = secret_store::delete(&secret_ref) {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!(
                    "{}; local key secret cleanup failed: {}",
                    error.message, cleanup_error.message
                ),
            )
            .into());
        }
        return Err(error.into());
    }
    Ok(GeneratedLocalKey {
        key: record,
        secret,
    })
}

fn responses_wire_api() -> WireApi {
    WireApi::Responses
}
