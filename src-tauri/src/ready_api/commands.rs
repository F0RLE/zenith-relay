use super::{client::*, models::*, top_up};
use crate::{
    codex_config::{
        deactivate_provider, enable_provider, ensure_provider_on_launch, load_api_key_for_launch,
        provider_has_token, reset_provider,
    },
    key_storage::{load_saved_app_key, save_app_key},
    launcher::{is_codex_running, launch_codex, launch_codex_with_profile},
    local_pool,
    platform::{default_codex_home, platform_name, system_locale},
    tray::close_main_window,
};
use std::{collections::BTreeSet, time::Duration};
use tauri::{AppHandle, Emitter};
use tauri_plugin_opener::OpenerExt;

const OPENAI_API_KEYS_URL: &str = "https://platform.openai.com/api-keys";
const OPENROUTER_API_KEYS_URL: &str = "https://openrouter.ai/settings/keys";
const MAX_MODELS_RESPONSE_BYTES: usize = 1024 * 1024;

#[tauri::command]
pub(super) fn get_state(state: tauri::State<'_, local_pool::DesktopState>) -> UiState {
    let _ = ensure_provider_on_launch(&state.ready_api_backup_root());
    UiState {
        provider_active: provider_has_token(),
        codex_running: is_codex_running(),
        has_saved_api_key: load_saved_app_key()
            .or_else(load_api_key_for_launch)
            .is_some_and(|value| !value.trim().is_empty()),
    }
}

#[tauri::command]
pub(super) fn get_platform() -> &'static str {
    platform_name()
}

#[tauri::command]
pub(super) fn get_system_locale() -> Option<String> {
    system_locale()
}

#[tauri::command]
pub(super) async fn get_saved_key_models() -> Result<Vec<String>, String> {
    let api_key = stored_api_key()?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "Models request could not be initialized.".to_string())?;
    let response = api_get(&client, "/models", &api_key).await?;
    if !response.status().is_success() {
        return Err(api_error_message(response, "Models request failed.").await);
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MODELS_RESPONSE_BYTES as u64)
    {
        return Err("Models response is too large.".to_string());
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Models response could not be read.".to_string())?
    {
        if body.len().saturating_add(chunk.len()) > MAX_MODELS_RESPONSE_BYTES {
            return Err("Models response is too large.".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    parse_model_ids(&body)
}

pub(super) fn parse_model_ids(body: &[u8]) -> Result<Vec<String>, String> {
    let response: ModelsResponse =
        serde_json::from_slice(body).map_err(|_| "Models response is invalid.".to_string())?;
    let models = response
        .data
        .into_iter()
        .map(|model| model.id)
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 256
                && !id.chars().any(char::is_control)
                && !id.chars().any(char::is_whitespace)
        })
        .take(2_048)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if models.is_empty() {
        Err("Models response contains no usable models.".to_string())
    } else {
        Ok(models)
    }
}

#[tauri::command]
pub(super) async fn save_key(
    api_key: String,
    activate: Option<bool>,
    app: AppHandle,
    state: tauri::State<'_, local_pool::DesktopState>,
) -> Result<String, String> {
    let api_key = normalize_api_key(&api_key)?;
    if activate == Some(false) {
        save_app_key(&api_key)?;
        let _ = app.emit("zenith-state-changed", ());
        return Ok("API key saved.".to_string());
    }
    let stopped = local_pool::commands::profiles::prepare_ready_api_profile(&state)
        .await
        .map_err(|error| error.message)?;
    let result = activate_ready_api_with_history(&api_key, true, &state);
    let result = finish_ready_api_profile_change(stopped, result);
    let _ = app.emit("zenith-state-changed", ());
    result
}

#[tauri::command]
pub(super) async fn activate_ready_api_profile(
    app: AppHandle,
    state: tauri::State<'_, local_pool::DesktopState>,
) -> Result<String, String> {
    if provider_has_token() {
        return Ok("Ready API profile is already active.".to_string());
    }
    let api_key = stored_api_key()?;
    let stopped = local_pool::commands::profiles::prepare_ready_api_profile(&state)
        .await
        .map_err(|error| error.message)?;
    let result = activate_ready_api_with_history(&api_key, false, &state);
    let result = finish_ready_api_profile_change(stopped, result);
    let _ = app.emit("zenith-state-changed", ());
    result
}

#[tauri::command]
pub(super) async fn deactivate_ready_api_profile(
    app: AppHandle,
    state: tauri::State<'_, local_pool::DesktopState>,
) -> Result<String, String> {
    if !provider_has_token() {
        return Ok("Chat profile is already active.".to_string());
    }
    let api_key = stored_api_key()?;
    let stopped = local_pool::commands::profiles::prepare_ready_api_profile(&state)
        .await
        .map_err(|error| error.message)?;
    let result = deactivate_ready_api_with_history(&api_key, false, &state);
    let result = finish_ready_api_profile_change(stopped, result);
    let _ = app.emit("zenith-state-changed", ());
    result
}

#[tauri::command]
pub(super) fn open_api_key_page(provider: String, app: AppHandle) -> Result<(), String> {
    let url = api_key_page_url(&provider).ok_or_else(|| "Unsupported API provider.".to_string())?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| error.to_string())
}

pub(super) fn api_key_page_url(provider: &str) -> Option<&'static str> {
    match provider {
        "zenith" => Some(top_up::BOT_URL),
        "openai" => Some(OPENAI_API_KEYS_URL),
        "openrouter" => Some(OPENROUTER_API_KEYS_URL),
        _ => None,
    }
}

#[tauri::command]
pub(super) async fn reset_key(
    app: AppHandle,
    state: tauri::State<'_, local_pool::DesktopState>,
) -> Result<String, String> {
    let active = provider_has_token();
    let api_key = active.then(stored_api_key).transpose()?;
    let stopped = if active {
        local_pool::commands::profiles::prepare_ready_api_profile(&state)
            .await
            .map_err(|error| error.message)?
    } else {
        false
    };
    let result = match api_key {
        Some(api_key) => deactivate_ready_api_with_history(&api_key, true, &state),
        None => reset_provider(&state.ready_api_backup_root()),
    };
    let result = finish_ready_api_profile_change(stopped, result);
    let _ = app.emit("zenith-state-changed", ());
    result
}

pub(super) fn finish_ready_api_profile_change(
    stopped: bool,
    result: Result<(), String>,
) -> Result<String, String> {
    let restart = stopped.then(launch_codex_with_profile).transpose();
    match (result, restart) {
        (Ok(()), Ok(_)) => Ok("ChatGPT profile updated.".to_string()),
        (Err(error), Ok(_)) => Err(error),
        (Ok(()), Err(restart_error)) => Err(format!(
            "ChatGPT profile was updated but could not be restarted: {restart_error}"
        )),
        (Err(error), Err(restart_error)) => Err(format!(
            "{error}; failed to restart ChatGPT: {restart_error}"
        )),
    }
}

pub(super) fn activate_ready_api_with_history(
    api_key: &str,
    save_key: bool,
    state: &local_pool::DesktopState,
) -> Result<(), String> {
    enable_provider(api_key, &state.ready_api_backup_root())?;
    let result = (|| {
        if save_key {
            save_app_key(api_key)?;
        }
        let backup = local_pool::commands::profiles::synchronize_codex_history(
            state,
            &default_codex_home(),
            local_pool::commands::profiles::CodexHistoryProvider::ReadyApi,
        )?;
        local_pool::commands::profiles::discard_codex_history_backup(state, backup.as_deref());
        Ok(())
    })();
    result.map_err(|error| {
        profile_change_with_rollback(error, deactivate_provider(&state.ready_api_backup_root()))
    })
}

pub(super) fn deactivate_ready_api_with_history(
    api_key: &str,
    forget_key: bool,
    state: &local_pool::DesktopState,
) -> Result<(), String> {
    if forget_key {
        reset_provider(&state.ready_api_backup_root())?;
    } else {
        deactivate_provider(&state.ready_api_backup_root())?;
    }
    let backup = local_pool::commands::profiles::synchronize_codex_history(
        state,
        &default_codex_home(),
        local_pool::commands::profiles::CodexHistoryProvider::ChatGpt,
    )
    .map_err(|error| {
        profile_change_with_rollback(
            error,
            enable_provider(api_key, &state.ready_api_backup_root()),
        )
    })?;
    local_pool::commands::profiles::discard_codex_history_backup(state, backup.as_deref());
    Ok(())
}

pub(super) fn profile_change_with_rollback(error: String, rollback: Result<(), String>) -> String {
    match rollback {
        Ok(()) => error,
        Err(rollback) => format!("{error}; profile rollback failed: {rollback}"),
    }
}

#[tauri::command]
pub(super) fn launch_saved_codex(
    app: AppHandle,
    state: tauri::State<'_, local_pool::DesktopState>,
) -> Result<String, String> {
    let _ = ensure_provider_on_launch(&state.ready_api_backup_root());
    if !provider_has_token() {
        return Err("Сначала сохраните API key.".to_string());
    }
    if !is_codex_running() {
        let backup = local_pool::commands::profiles::synchronize_codex_history(
            &state,
            &default_codex_home(),
            local_pool::commands::profiles::CodexHistoryProvider::ReadyApi,
        )?;
        local_pool::commands::profiles::discard_codex_history_backup(&state, backup.as_deref());
    }
    let message = launch_codex();
    close_main_window(&app);
    let _ = app.emit("zenith-state-changed", ());
    Ok(message)
}
