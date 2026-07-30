use super::{
    clean_label, default_weight, normalized_values, runtime_error, store_error, valid_weight,
    validate_secret, validation_error, vault_error, ManagementError,
};
use crate::state::{AppState, SourceRecord};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use zenith_relay_core::protocol::SourceSummary;
use zenith_relay_core::{
    discover_source_models, fetch_source_provider_stats, normalize_model_price_overrides,
    source_points_to_gateway, ApiModelPriceOverride, ProviderSource, WireApi,
};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sources", get(list_sources).post(create_source))
        .route("/sources/{id}", patch(update_source).delete(delete_source))
        .route("/sources/{id}/test", post(test_source))
        .route("/sources/{id}/stats", get(source_stats))
}

pub async fn list_sources(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<SourceSummary>>, ManagementError> {
    Ok(Json(state.snapshot().map_err(store_error)?.sources))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInput {
    name: String,
    base_url: String,
    api_key: String,
    wire_api: WireApi,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    allowed_models: Vec<String>,
    #[serde(default)]
    excluded_models: Vec<String>,
    #[serde(default)]
    priority: i32,
    #[serde(default = "default_weight")]
    weight: u32,
    #[serde(default)]
    recovery_delay_seconds: u64,
    #[serde(default)]
    model_price_overrides: BTreeMap<String, ApiModelPriceOverride>,
}

pub async fn create_source(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SourceInput>,
) -> Result<(StatusCode, Json<SourceSummary>), ManagementError> {
    validate_secret(&input.api_key, "source API key")?;
    let api_key = input.api_key.clone();
    let id = format!("source_{}", uuid::Uuid::new_v4().simple());
    let secret_ref = format!("source:{id}");
    let mut record = source_record(id, secret_ref.clone(), input)?;
    ensure_not_server_self_source(&state, &record.base_url)?;
    record.models = discover_models(&record, &api_key).await?;
    state
        .vault
        .save(&secret_ref, &api_key)
        .map_err(vault_error)?;
    if let Err(error) = state.store.save_source(&record) {
        let _ = state.vault.delete(&secret_ref);
        return Err(store_error(error));
    }
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.store.delete_source(&record.id);
        let _ = state.vault.delete(&secret_ref);
        return Err(runtime_error(error));
    }
    Ok((StatusCode::CREATED, Json(source_summary(&state, &record)?)))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePatch {
    name: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    wire_api: Option<WireApi>,
    models: Option<Vec<String>>,
    allowed_models: Option<Vec<String>>,
    excluded_models: Option<Vec<String>>,
    enabled: Option<bool>,
    in_pool: Option<bool>,
    draining: Option<bool>,
    priority: Option<i32>,
    weight: Option<u32>,
    recovery_delay_seconds: Option<u64>,
    #[serde(default)]
    model_price_overrides: Option<BTreeMap<String, ApiModelPriceOverride>>,
}

pub async fn update_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<SourcePatch>,
) -> Result<Json<SourceSummary>, ManagementError> {
    let mut record = find_source(&state, &id)?;
    let old_record = record.clone();
    let old_secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found("source_secret_missing", "source secret missing")
        })?;
    if let Some(value) = input.name {
        record.name = clean_label(&value, "source name")?;
    }
    if let Some(value) = input.base_url {
        record.base_url = value.trim().to_string();
    }
    if let Some(value) = input.wire_api {
        record.wire_api = value;
    }
    if let Some(value) = input.models {
        record.models = normalized_values(value);
    }
    if let Some(value) = input.allowed_models {
        record.allowed_models = normalized_values(value);
    }
    if let Some(value) = input.excluded_models {
        record.excluded_models = normalized_values(value);
    }
    if let Some(value) = input.enabled {
        record.enabled = value;
    }
    if let Some(value) = input.in_pool {
        record.in_pool = value;
    }
    if let Some(value) = input.draining {
        record.draining = value;
    }
    if let Some(value) = input.priority {
        record.priority = value;
    }
    if let Some(value) = input.weight {
        record.weight = valid_weight(value)?;
    }
    if let Some(value) = input.recovery_delay_seconds {
        record.recovery_delay_seconds = valid_recovery_delay(value)?;
    }
    if let Some(value) = input.model_price_overrides {
        record.model_price_overrides = normalize_source_prices(value)?;
    }
    validate_source_record(&record, input.api_key.as_deref().unwrap_or(&old_secret))?;
    ensure_not_server_self_source(&state, &record.base_url)?;
    if let Some(secret) = input.api_key.as_deref() {
        validate_secret(secret, "source API key")?;
        state
            .vault
            .save(&record.secret_ref, secret)
            .map_err(vault_error)?;
    }
    if let Err(error) = state.store.save_source(&record) {
        let _ = state.vault.save(&record.secret_ref, &old_secret);
        return Err(store_error(error));
    }
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.store.save_source(&old_record);
        let _ = state.vault.save(&old_record.secret_ref, &old_secret);
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(Json(source_summary(&state, &record)?))
}

pub async fn delete_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    let record = find_source(&state, &id)?;
    let secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found("source_secret_missing", "source secret missing")
        })?;
    let old_keys = state.store.keys().map_err(store_error)?;
    let mut scoped_keys = old_keys.clone();
    for key in &mut scoped_keys {
        if let Some(ids) = &mut key.source_ids {
            ids.retain(|value| value != &id);
        }
        state.store.save_key(key).map_err(store_error)?;
    }
    state.store.delete_source(&id).map_err(store_error)?;
    state
        .vault
        .delete(&record.secret_ref)
        .map_err(vault_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.vault.save(&record.secret_ref, &secret);
        let _ = state.store.save_source(&record);
        for key in old_keys {
            let _ = state.store.save_key(&key);
        }
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn test_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SourceSummary>, ManagementError> {
    let mut record = find_source(&state, &id)?;
    let previous = record.clone();
    let api_key = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found("source_secret_missing", "source secret missing")
        })?;
    ensure_not_server_self_source(&state, &record.base_url)?;
    record.models = match discover_models(&record, &api_key).await {
        Ok(models) => models,
        Err(error) => {
            record.last_error_code = Some(error.code.clone());
            state.store.save_source(&record).map_err(store_error)?;
            return Err(error);
        }
    };
    record.last_error_code = None;
    state.store.save_source(&record).map_err(store_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.store.save_source(&previous);
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(Json(source_summary(&state, &record)?))
}

pub async fn source_stats(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<zenith_relay_core::SourceProviderStats>, ManagementError> {
    let record = find_source(&state, &id)?;
    let api_key = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found("source_secret_missing", "source secret is missing")
        })?;
    fetch_source_provider_stats(&record.base_url, &api_key)
        .await
        .map(Json)
        .map_err(|_| {
            ManagementError::new(
                StatusCode::BAD_GATEWAY,
                "source_stats_unavailable",
                "source stats are unavailable",
                "source",
                true,
            )
        })
}

fn source_record(
    id: String,
    secret_ref: String,
    input: SourceInput,
) -> Result<SourceRecord, ManagementError> {
    let record = SourceRecord {
        id,
        name: clean_label(&input.name, "source name")?,
        enabled: true,
        in_pool: false,
        draining: false,
        base_url: input.base_url.trim().to_string(),
        secret_ref,
        wire_api: input.wire_api,
        models: normalized_values(input.models),
        allowed_models: normalized_values(input.allowed_models),
        excluded_models: normalized_values(input.excluded_models),
        priority: input.priority,
        weight: valid_weight(input.weight)?,
        recovery_delay_seconds: valid_recovery_delay(input.recovery_delay_seconds)?,
        model_price_overrides: normalize_source_prices(input.model_price_overrides)?,
        last_error_code: None,
    };
    validate_source_record(&record, &input.api_key)?;
    Ok(record)
}

fn validate_source_record(record: &SourceRecord, api_key: &str) -> Result<(), ManagementError> {
    valid_recovery_delay(record.recovery_delay_seconds)?;
    normalize_source_prices(record.model_price_overrides.clone())?;
    if matches!(record.wire_api, WireApi::Messages) {
        return Err(ManagementError::validation(
            "source_protocol_unsupported",
            "source protocol is not supported",
        ));
    }
    ProviderSource {
        id: record.id.clone(),
        name: record.name.clone(),
        base_url: record.base_url.clone(),
        api_key: api_key.to_string(),
        wire_api: record.wire_api,
        models: record.models.clone(),
    }
    .validate()
    .map_err(|error| validation_error(error.to_string()))
}

fn valid_recovery_delay(value: u64) -> Result<u64, ManagementError> {
    (value <= 24 * 60 * 60).then_some(value).ok_or_else(|| {
        ManagementError::validation(
            "source_recovery_delay_invalid",
            "source recovery delay must not exceed 24 hours",
        )
    })
}

fn normalize_source_prices(
    prices: BTreeMap<String, ApiModelPriceOverride>,
) -> Result<BTreeMap<String, ApiModelPriceOverride>, ManagementError> {
    normalize_model_price_overrides(prices)
        .map_err(|message| ManagementError::validation("source_model_price_invalid", message))
}

async fn discover_models(
    record: &SourceRecord,
    api_key: &str,
) -> Result<Vec<String>, ManagementError> {
    let source = ProviderSource {
        id: record.id.clone(),
        name: record.name.clone(),
        base_url: record.base_url.clone(),
        api_key: api_key.to_string(),
        wire_api: record.wire_api,
        models: record.models.clone(),
    };
    let models = discover_source_models(&source).await.map_err(|_| {
        ManagementError::new(
            StatusCode::BAD_GATEWAY,
            "source_test_failed",
            "source model discovery failed",
            "upstream",
            true,
        )
    })?;
    if models.is_empty() {
        return Err(ManagementError::validation(
            "source_models_empty",
            "source did not expose any models",
        ));
    }
    Ok(models)
}

fn find_source(state: &AppState, id: &str) -> Result<SourceRecord, ManagementError> {
    state
        .store
        .sources()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or_else(|| ManagementError::not_found("source_not_found", "source not found"))
}

fn source_summary(
    state: &AppState,
    record: &SourceRecord,
) -> Result<SourceSummary, ManagementError> {
    state
        .snapshot()
        .map_err(store_error)?
        .sources
        .into_iter()
        .find(|value| value.id == record.id)
        .ok_or_else(|| ManagementError::internal("snapshot_missing", "source snapshot missing"))
}

fn ensure_not_server_self_source(
    state: &AppState,
    source_base_url: &str,
) -> Result<(), ManagementError> {
    let gateway_base_url = format!(
        "{}/v1",
        state.config.public_base_url.as_str().trim_end_matches('/')
    );
    if source_points_to_gateway(source_base_url, &gateway_base_url) {
        return Err(ManagementError::validation(
            "source_self_route",
            "source base URL must not point back to this Relay gateway",
        ));
    }
    Ok(())
}
