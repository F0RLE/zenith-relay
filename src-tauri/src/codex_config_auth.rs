use super::{
    config_uses_zenith_provider, default_codex_home, is_zenith_customer_key, remove_if_unchanged,
    replace_if_unchanged, rollback_file, with_cleanup, AUTH_FILE, LEGACY_PROVIDER_ID, PROVIDER_ID,
};
use crate::{
    files::{escape_json_string, unquote_toml_string},
    key_storage::{
        delete_previous_codex_auth, load_previous_codex_auth, load_saved_app_key,
        save_previous_codex_auth,
    },
};
use std::path::Path;

pub(crate) fn load_api_key_for_launch() -> Option<String> {
    load_saved_app_key()
        .or_else(load_zenith_key_from_codex_config)
        .or_else(load_zenith_auth_key_if_configured)
}

pub(super) fn load_zenith_auth_key_if_configured() -> Option<String> {
    let config_path = default_codex_home().join(super::CONFIG_FILE);
    let config = std::fs::read_to_string(config_path).ok()?;
    zenith_auth_key_if_configured(&config, load_codex_auth_key())
}

pub(super) fn zenith_auth_key_if_configured(config: &str, key: Option<String>) -> Option<String> {
    config_uses_zenith_provider(config)
        .then_some(key)
        .flatten()
        .filter(|key| is_zenith_customer_key(key))
}

pub(super) fn load_zenith_key_from_codex_config() -> Option<String> {
    let config_path = default_codex_home().join(super::CONFIG_FILE);
    let content = std::fs::read_to_string(config_path).ok()?;
    let mut in_zenith = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_zenith = trimmed == format!("[model_providers.{PROVIDER_ID}]")
                || trimmed == format!("[model_providers.{LEGACY_PROVIDER_ID}]");
            continue;
        }
        if in_zenith {
            if let Some(value) = trimmed.strip_prefix("experimental_bearer_token = ") {
                let key = unquote_toml_string(value.trim())?;
                if !key.is_empty() {
                    return Some(key);
                }
            }
        }
    }

    None
}

fn load_codex_auth_key() -> Option<String> {
    let auth_path = default_codex_home().join(AUTH_FILE);
    let content = std::fs::read_to_string(auth_path).ok()?;
    let auth: serde_json::Value = serde_json::from_str(&content).ok()?;
    let key = auth
        .get("OPENAI_API_KEY")
        .and_then(serde_json::Value::as_str)?
        .trim()
        .to_string();
    (!key.is_empty()).then_some(key)
}

pub(super) fn codex_auth_content(api_key: &str) -> String {
    format!(
        "{{\n  \"OPENAI_API_KEY\": \"{}\",\n  \"auth_mode\": \"apikey\"\n}}\n",
        escape_json_string(api_key)
    )
}

pub(super) fn save_previous_auth_if_needed(
    config_before_enable: &str,
    auth_before_enable: Option<&str>,
) -> Result<bool, String> {
    if load_previous_codex_auth().is_some() {
        return Ok(false);
    }
    let Some(content) = auth_before_enable else {
        return Ok(false);
    };
    if content.trim().is_empty() {
        return Ok(false);
    }
    let Ok(auth) = serde_json::from_str::<serde_json::Value>(content) else {
        return Ok(false);
    };
    if !previous_codex_auth_should_be_saved(
        &auth,
        load_saved_app_key().as_deref(),
        config_before_enable,
    ) {
        return Ok(false);
    }
    save_previous_codex_auth(content)?;
    Ok(true)
}

pub(super) fn previous_codex_auth_should_be_saved(
    auth: &serde_json::Value,
    saved_key: Option<&str>,
    config_before_enable: &str,
) -> bool {
    auth.is_object() && !zenith_auth_is_owned(auth, saved_key, config_before_enable)
}

pub(super) fn restore_or_remove_zenith_auth(
    config_before_reset: &str,
    auth_before_reset: Option<&str>,
    saved_key: Option<&str>,
) -> Result<(), String> {
    let auth_path = default_codex_home().join(AUTH_FILE);
    if let Some(previous_auth) = load_previous_codex_auth() {
        replace_if_unchanged(&auth_path, auth_before_reset, &previous_auth)?;
        if let Err(error) = delete_previous_codex_auth() {
            return Err(with_cleanup(
                error,
                rollback_file(&auth_path, Some(&previous_auth), auth_before_reset),
            ));
        }
        return Ok(());
    }
    remove_zenith_auth_if_owned(
        config_before_reset,
        auth_before_reset,
        saved_key,
        &auth_path,
    )
}

fn remove_zenith_auth_if_owned(
    config_before_reset: &str,
    auth_before_reset: Option<&str>,
    saved_key: Option<&str>,
    auth_path: &Path,
) -> Result<(), String> {
    let Some(content) = auth_before_reset else {
        return Ok(());
    };
    let Ok(auth) = serde_json::from_str::<serde_json::Value>(content) else {
        return Ok(());
    };
    if zenith_auth_is_owned(&auth, saved_key, config_before_reset) {
        remove_if_unchanged(auth_path, Some(content))?;
    }
    Ok(())
}

pub(super) fn zenith_auth_is_owned(
    auth: &serde_json::Value,
    saved_key: Option<&str>,
    config_before_reset: &str,
) -> bool {
    let Some(current_key) = auth
        .get("OPENAI_API_KEY")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
    else {
        return false;
    };
    let auth_mode_matches = auth
        .get("auth_mode")
        .and_then(serde_json::Value::as_str)
        .map(|mode| mode == "apikey")
        .unwrap_or(false);
    if !auth_mode_matches {
        return false;
    }
    let key_matches = auth
        .get("OPENAI_API_KEY")
        .and_then(serde_json::Value::as_str)
        .map(|key| saved_key.is_some_and(|saved_key| key.trim() == saved_key.trim()))
        .unwrap_or(false);
    key_matches
        || (config_uses_zenith_provider(config_before_reset) && is_zenith_customer_key(current_key))
}
