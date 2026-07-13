use super::{restart_or_rollback, runtime_from_store, sync_gateway_or_rollback};
use crate::local_pool::{
    accounts::proxy::COMMON_PROXY_SECRET_REF,
    error::{CommandError, ErrorCode, LocalPoolError},
    models::LocalPoolSnapshot,
    state::DesktopState,
    store::secret_store,
};
use reqwest::{redirect::Policy, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tauri::State;

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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetCommonProxyInput {
    proxy_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetAccountProxyPolicyInput {
    required: bool,
}

#[tauri::command]
pub async fn start_local_gateway(
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    let runtime = runtime_from_store(&state).await?;
    let port = state.store()?.gateway().port;
    state.gateway.start(runtime, port).await?;
    let enable_result = { state.store()?.set_gateway_enabled(true) };
    if let Err(error) = enable_result {
        state.gateway.stop().await;
        return Err(error.into());
    }
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn stop_local_gateway(
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    state.store()?.set_gateway_enabled(false)?;
    state.gateway.stop().await;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn restart_local_gateway(
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    if state.gateway.address().await.is_none() {
        return Err(gateway_not_running().into());
    }
    let gateway = state.store()?.gateway().clone();
    sync_gateway_or_rollback(&state, gateway).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn update_local_gateway_port(
    port: u16,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    let old_gateway = state.store()?.gateway().clone();
    if old_gateway.port == port {
        return state.snapshot().await.map_err(Into::into);
    }
    let mut gateway = old_gateway.clone();
    gateway.port = port;
    state.store()?.replace_gateway(gateway)?;
    sync_gateway_or_rollback(&state, old_gateway).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_common_proxy(
    input: SetCommonProxyInput,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    let next_secret = input
        .proxy_url
        .map(|value| zenith_relay_core::normalize_proxy_url(&value))
        .transpose()
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    let old_gateway = state.store()?.gateway().clone();
    let old_secret = secret_store::load(COMMON_PROXY_SECRET_REF)?;
    if old_gateway.common_proxy_configured == next_secret.is_some()
        && old_secret.as_deref() == next_secret.as_deref()
    {
        return state.snapshot().await.map_err(Into::into);
    }
    save_optional_proxy(next_secret.as_deref())?;
    let mut next_gateway = old_gateway.clone();
    next_gateway.common_proxy_configured = next_secret.is_some();
    if let Err(error) = state.store()?.replace_gateway(next_gateway) {
        restore_common_proxy(old_secret.as_deref())?;
        return Err(error.into());
    }
    restart_or_rollback(&state, || {
        restore_common_proxy(old_secret.as_deref())?;
        state.store()?.replace_gateway(old_gateway)
    })
    .await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_account_proxy_required(
    input: SetAccountProxyPolicyInput,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    let old_gateway = state.store()?.gateway().clone();
    if old_gateway.account_proxy_required == input.required {
        return state.snapshot().await.map_err(Into::into);
    }
    let mut next_gateway = old_gateway.clone();
    next_gateway.account_proxy_required = input.required;
    state.store()?.replace_gateway(next_gateway)?;
    restart_or_rollback(&state, || state.store()?.replace_gateway(old_gateway)).await?;
    state.snapshot().await.map_err(Into::into)
}

fn save_optional_proxy(value: Option<&str>) -> crate::local_pool::error::Result<()> {
    match value {
        Some(value) => secret_store::save(COMMON_PROXY_SECRET_REF, value),
        None => secret_store::delete(COMMON_PROXY_SECRET_REF),
    }
}

fn restore_common_proxy(value: Option<&str>) -> crate::local_pool::error::Result<()> {
    save_optional_proxy(value)
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
    let keys = state.store()?.keys().to_vec();
    let mut key = None;
    for candidate in &keys {
        if candidate.enabled && super::pool::has_usable_source(&state, candidate)? {
            key = Some(candidate);
            break;
        }
    }
    let key = key.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::Conflict,
            "no usable local gateway key is available",
        )
    })?;
    let secret = secret_store::load(&key.secret_ref)?.ok_or_else(|| {
        LocalPoolError::new(ErrorCode::NotFound, "local gateway key secret is missing")
    })?;
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(gateway_error)?;
    let base_url = format!("http://{address}/v1");
    let models_response = client
        .get(format!("{base_url}/models"))
        .bearer_auth(&secret)
        .send()
        .await
        .map_err(gateway_error)?;
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
        .find(|model| valid_model_id(model))
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
        .map_err(gateway_error)?;
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

pub async fn start_if_enabled(state: &DesktopState) -> Result<(), LocalPoolError> {
    let (enabled, port) = {
        let store = state.store()?;
        (store.gateway().enabled, store.gateway().port)
    };
    if enabled {
        state
            .gateway
            .start(runtime_from_store(state).await?, port)
            .await?;
    }
    Ok(())
}

fn gateway_not_running() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::GatewayUnavailable,
        "local gateway is not running",
    )
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
    while let Some(chunk) = response.chunk().await.map_err(gateway_error)? {
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

fn valid_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.chars().any(char::is_control)
        && !value.chars().any(char::is_whitespace)
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
    .into()
}

fn gateway_error(error: impl std::fmt::Display) -> CommandError {
    LocalPoolError::new(ErrorCode::GatewayUnavailable, error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_model_ids_are_bounded_and_single_line() {
        assert!(valid_model_id("gpt-test"));
        assert!(!valid_model_id(""));
        assert!(!valid_model_id("gpt test"));
        assert!(!valid_model_id("gpt\nsecret"));
        assert!(!valid_model_id(&"x".repeat(257)));
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
