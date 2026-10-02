use crate::local_pool::{
    error::{CommandError, ErrorCode, ErrorDiagnostics, LocalPoolError},
    state::DesktopState,
    store::secret_store,
};
use reqwest::{redirect::Policy, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tauri::State;
use zenith_relay_core::is_valid_model_token;

use super::super::remote_server::remote_error;
use super::lifecycle::gateway_not_running;

const MAX_DIAGNOSTIC_BYTES: usize = 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayDiagnostic {
    pub stream: bool,
    pub model: String,
    pub latency_ms: u64,
    pub bytes_received: usize,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

#[tauri::command]
pub async fn diagnose_local_gateway(
    stream: bool,
    state: State<'_, DesktopState>,
) -> Result<GatewayDiagnostic, CommandError> {
    let address = state
        .gateway
        .address()
        .await
        .ok_or_else(gateway_not_running)?;
    if !super::super::pool::has_usable_pool_candidate(&state)? {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "no usable internal profile credential is available",
        )
        .into());
    }
    let key = super::super::pool::ensure_system_gateway_key(&state)?;
    let secret = secret_store::load(&key.secret_ref)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::NotFound,
            "internal profile credential is missing",
        )
    })?;
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(remote_error)?;
    let base_url = format!("http://{address}/v1");
    let models_response = client
        .get(format!("{base_url}/models"))
        .bearer_auth(&secret)
        .send()
        .await
        .map_err(remote_error)?;
    let models_status = models_response.status();
    let models_body = read_limited(models_response).await?;
    if !models_status.is_success() {
        return Err(status_error("model diagnostic", models_status));
    }
    let models: ModelsResponse = serde_json::from_slice(&models_body).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            "model diagnostic returned invalid JSON",
        )
    })?;
    let model = models
        .data
        .into_iter()
        .map(|model| model.id)
        .find(|model| is_valid_model_token(model))
        .ok_or_else(|| {
            LocalPoolError::new(ErrorCode::Conflict, "local gateway exposes no usable model")
        })?;

    let started = Instant::now();
    let response = client
        .post(format!("{base_url}/responses"))
        .bearer_auth(&secret)
        .json(&serde_json::json!({
            "model": model,
            "input": "Reply with OK.",
            "stream": stream,
            "max_output_tokens": 8,
            "tools": []
        }))
        .send()
        .await
        .map_err(remote_error)?;
    let status = response.status();
    let body = read_limited(response).await?;
    if !status.is_success() {
        return Err(status_error("request diagnostic", status));
    }
    if stream {
        let text = std::str::from_utf8(&body).map_err(|_| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "stream diagnostic returned invalid text",
            )
        })?;
        if !text.contains("response.completed") && !text.contains("[DONE]") {
            return Err(LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "stream diagnostic did not reach a terminal event",
            )
            .into());
        }
    } else if !valid_diagnostic_response(&body) {
        return Err(LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            "request diagnostic returned invalid JSON",
        )
        .into());
    }
    Ok(GatewayDiagnostic {
        stream,
        model,
        latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        bytes_received: body.len(),
    })
}

async fn read_limited(mut response: Response) -> Result<Vec<u8>, CommandError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_DIAGNOSTIC_BYTES as u64)
    {
        return Err(LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            "diagnostic response exceeds the size limit",
        )
        .into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(remote_error)? {
        if body.len().saturating_add(chunk.len()) > MAX_DIAGNOSTIC_BYTES {
            return Err(LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "diagnostic response exceeds the size limit",
            )
            .into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn valid_diagnostic_response(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body).is_ok_and(|value| {
        value.is_object() && value.get("error").is_none_or(serde_json::Value::is_null)
    })
}

fn status_error(stage: &str, status: StatusCode) -> CommandError {
    LocalPoolError::new(
        ErrorCode::GatewayUnavailable,
        format!("{stage} failed with HTTP {}", status.as_u16()),
    )
    .with_diagnostic(ErrorDiagnostics {
        reason: Some(stage.to_string()),
        status: Some(status.as_u16()),
        retryable: Some(status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS),
        ..ErrorDiagnostics::default()
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_model_ids_are_bounded_and_single_line() {
        assert!(is_valid_model_token("gpt-test"));
        assert!(!is_valid_model_token(""));
        assert!(!is_valid_model_token("gpt test"));
        assert!(!is_valid_model_token("gpt\nsecret"));
        assert!(!is_valid_model_token(&"x".repeat(257)));
    }

    #[test]
    fn diagnostic_accepts_nullable_error_but_rejects_error_objects() {
        assert!(valid_diagnostic_response(br#"{"error":null,"output":[]}"#));
        assert!(valid_diagnostic_response(br#"{"output":[]}"#));
        assert!(!valid_diagnostic_response(
            br#"{"error":{"message":"failed"}}"#
        ));
    }
}
