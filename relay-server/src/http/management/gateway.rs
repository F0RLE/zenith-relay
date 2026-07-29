use super::{runtime_error, store_error, ManagementError};
use crate::state::AppState;
use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tower::ServiceExt;
use zenith_relay_core::protocol::{GatewayDiagnostic, RuntimeStateSnapshot};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/diagnostics", post(diagnose_gateway))
        .route("/gateway/start", post(start_gateway))
        .route("/gateway/stop", post(stop_gateway))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticInput {
    #[serde(default)]
    stream: bool,
}

pub async fn diagnose_gateway(
    State(state): State<Arc<AppState>>,
    Json(input): Json<DiagnosticInput>,
) -> Result<Json<GatewayDiagnostic>, ManagementError> {
    if !state.store.gateway_enabled().map_err(store_error)? {
        return Err(ManagementError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway_stopped",
            "personal pool gateway is stopped",
            "diagnostics",
            true,
        ));
    }
    let runtime = state.runtime().map_err(runtime_error)?.ok_or_else(|| {
        ManagementError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "runtime_unavailable",
            "personal pool runtime is unavailable",
            "diagnostics",
            true,
        )
    })?;
    let secret = state
        .store
        .keys()
        .map_err(store_error)?
        .into_iter()
        .find_map(|key| {
            key.enabled
                .then(|| state.vault.load(&key.secret_ref).ok().flatten())
                .flatten()
        })
        .ok_or_else(|| {
            ManagementError::validation(
                "diagnostic_key_unavailable",
                "no enabled personal pool key is available",
            )
        })?;

    let models =
        internal_gateway_request(runtime.clone(), "GET", "/v1/models", &secret, Body::empty())
            .await?;
    let model = serde_json::from_slice::<Value>(&models)
        .ok()
        .and_then(|value| value.get("data").and_then(Value::as_array).cloned())
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
        .find(|id| valid_diagnostic_model(id))
        .ok_or_else(|| {
            ManagementError::validation(
                "diagnostic_model_unavailable",
                "personal pool key exposes no usable model",
            )
        })?;
    let body = serde_json::to_vec(&serde_json::json!({
        "model": model,
        "input": "Reply with OK.",
        "stream": input.stream,
        "max_output_tokens": 8,
        "tools": []
    }))
    .map_err(|_| ManagementError::internal("diagnostic_failed", "diagnostic request failed"))?;
    let started = Instant::now();
    let response =
        internal_gateway_request(runtime, "POST", "/v1/responses", &secret, Body::from(body))
            .await?;
    if input.stream {
        let text = std::str::from_utf8(&response).map_err(|_| {
            ManagementError::internal("diagnostic_invalid", "stream diagnostic was invalid")
        })?;
        if !text.contains("response.completed") && !text.contains("[DONE]") {
            return Err(ManagementError::internal(
                "diagnostic_incomplete",
                "stream diagnostic did not reach a terminal event",
            ));
        }
    } else if !serde_json::from_slice::<Value>(&response)
        .is_ok_and(|value| value.is_object() && value.get("error").is_none_or(Value::is_null))
    {
        return Err(ManagementError::internal(
            "diagnostic_invalid",
            "non-stream diagnostic was invalid",
        ));
    }
    Ok(Json(GatewayDiagnostic {
        stream: input.stream,
        model,
        latency_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        bytes_received: response.len(),
    }))
}

const MAX_DIAGNOSTIC_BYTES: usize = 1024 * 1024;

async fn internal_gateway_request(
    runtime: Arc<zenith_relay_core::GatewayRuntime>,
    method: &str,
    uri: &str,
    secret: &str,
    body: Body,
) -> Result<Vec<u8>, ManagementError> {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "127.0.0.1")
        .header(header::AUTHORIZATION, format!("Bearer {secret}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .map_err(|_| ManagementError::internal("diagnostic_failed", "diagnostic request failed"))?;
    let response = zenith_relay_core::gateway::router(runtime)
        .oneshot(request)
        .await
        .map_err(|_| ManagementError::internal("diagnostic_failed", "diagnostic request failed"))?;
    if !response.status().is_success() {
        return Err(ManagementError::new(
            StatusCode::BAD_GATEWAY,
            "diagnostic_upstream_failed",
            format!("diagnostic failed with HTTP {}", response.status().as_u16()),
            "diagnostics",
            true,
        ));
    }
    to_bytes(response.into_body(), MAX_DIAGNOSTIC_BYTES)
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|_| {
            ManagementError::internal(
                "diagnostic_too_large",
                "diagnostic response exceeded the limit",
            )
        })
}

fn valid_diagnostic_model(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.chars().any(char::is_control)
        && !value.chars().any(char::is_whitespace)
}

pub async fn start_gateway(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    state.store.set_gateway_enabled(true).map_err(store_error)?;
    state.rebuild_runtime().await.map_err(runtime_error)?;
    Ok(Json(state.snapshot().map_err(store_error)?))
}

pub async fn stop_gateway(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    state
        .store
        .set_gateway_enabled(false)
        .map_err(store_error)?;
    Ok(Json(state.snapshot().map_err(store_error)?))
}
