use super::super::{runtime_error, store_error, ManagementError};
use crate::state::{AppState, SYSTEM_GATEWAY_KEY_ID};
use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tower::ServiceExt;
use zenith_relay_core::error_codes;
use zenith_relay_core::{is_valid_model_token, protocol::GatewayDiagnostic};

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
            error_codes::GATEWAY_STOPPED,
            "personal pool gateway is stopped",
            "diagnostics",
            true,
        ));
    }
    let runtime = state.runtime().map_err(runtime_error)?.ok_or_else(|| {
        ManagementError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            error_codes::RUNTIME_UNAVAILABLE,
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
            (key.id == SYSTEM_GATEWAY_KEY_ID && key.system && key.enabled)
                .then(|| state.vault.load(&key.secret_ref).ok().flatten())
                .flatten()
        })
        .ok_or_else(|| {
            ManagementError::validation(
                error_codes::DIAGNOSTIC_KEY_UNAVAILABLE,
                "internal profile credential is unavailable",
            )
        })?;

    let models =
        internal_gateway_request(runtime.clone(), "GET", "/v1/models", &secret, Body::empty())
            .await?;
    let model_id = serde_json::from_slice::<Value>(&models)
        .ok()
        .and_then(|response_payload| {
            response_payload
                .get("data")
                .and_then(Value::as_array)
                .cloned()
        })
        .into_iter()
        .flatten()
        .filter_map(|model_record| {
            model_record
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .find(|model_id| is_valid_model_token(model_id))
        .ok_or_else(|| {
            ManagementError::validation(
                error_codes::DIAGNOSTIC_MODEL_UNAVAILABLE,
                "managed profile exposes no usable model",
            )
        })?;
    let diagnostic_request_body = serde_json::to_vec(&serde_json::json!({
        "model": model_id,
        "input": "Reply with OK.",
        "stream": input.stream,
        "max_output_tokens": 8,
        "tools": []
    }))
    .map_err(|_| {
        ManagementError::internal(error_codes::DIAGNOSTIC_FAILED, "diagnostic request failed")
    })?;
    let started = Instant::now();
    let response = internal_gateway_request(
        runtime,
        "POST",
        "/v1/responses",
        &secret,
        Body::from(diagnostic_request_body),
    )
    .await?;
    if input.stream {
        let text = std::str::from_utf8(&response).map_err(|_| {
            ManagementError::internal(
                error_codes::DIAGNOSTIC_INVALID,
                "stream diagnostic was invalid",
            )
        })?;
        if !text.contains("response.completed") && !text.contains("[DONE]") {
            return Err(ManagementError::internal(
                error_codes::DIAGNOSTIC_INCOMPLETE,
                "stream diagnostic did not reach a terminal event",
            ));
        }
    } else if !serde_json::from_slice::<Value>(&response).is_ok_and(|response_payload| {
        response_payload.is_object() && response_payload.get("error").is_none_or(Value::is_null)
    }) {
        return Err(ManagementError::internal(
            error_codes::DIAGNOSTIC_INVALID,
            "non-stream diagnostic was invalid",
        ));
    }
    Ok(Json(GatewayDiagnostic {
        stream: input.stream,
        model: model_id,
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
    request_body: Body,
) -> Result<Vec<u8>, ManagementError> {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "127.0.0.1")
        .header(header::AUTHORIZATION, format!("Bearer {secret}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(request_body)
        .map_err(|_| {
            ManagementError::internal(error_codes::DIAGNOSTIC_FAILED, "diagnostic request failed")
        })?;
    let response = zenith_relay_core::gateway::router(runtime)
        .oneshot(request)
        .await
        .map_err(|_| {
            ManagementError::internal(error_codes::DIAGNOSTIC_FAILED, "diagnostic request failed")
        })?;
    if !response.status().is_success() {
        return Err(ManagementError::new(
            StatusCode::BAD_GATEWAY,
            error_codes::DIAGNOSTIC_UPSTREAM_FAILED,
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
                error_codes::DIAGNOSTIC_TOO_LARGE,
                "diagnostic response exceeded the limit",
            )
        })
}
