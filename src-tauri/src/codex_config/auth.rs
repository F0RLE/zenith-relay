use super::{
    AUTH_FILE, CONFIG_FILE, LEGACY_PROVIDER_ID, PROVIDER_ID, config_uses_zenith_provider,
    default_codex_home, is_zenith_customer_key,
};
use crate::{files::unquote_toml_string, key_storage::load_saved_app_key};

/// Read old saved keys for explicit source launches; profile writes use the pool integration.
pub(crate) fn load_api_key_for_launch() -> Option<String> {
    load_saved_app_key()
        .or_else(load_zenith_key_from_codex_config)
        .or_else(load_zenith_auth_key_if_configured)
}

fn load_zenith_auth_key_if_configured() -> Option<String> {
    let config_path = default_codex_home().join(CONFIG_FILE);
    let config_text = std::fs::read_to_string(config_path).ok()?;
    zenith_auth_key_if_configured(&config_text, load_codex_auth_key())
}

pub(super) fn zenith_auth_key_if_configured(
    config_text: &str,
    candidate_api_key: Option<String>,
) -> Option<String> {
    config_uses_zenith_provider(config_text)
        .then_some(candidate_api_key)
        .flatten()
        .filter(|key| is_zenith_customer_key(key))
}

fn load_zenith_key_from_codex_config() -> Option<String> {
    let config_path = default_codex_home().join(CONFIG_FILE);
    let config_text = std::fs::read_to_string(config_path).ok()?;
    let mut in_zenith = false;
    for line in config_text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_zenith = trimmed == format!("[model_providers.{PROVIDER_ID}]")
                || trimmed == format!("[model_providers.{LEGACY_PROVIDER_ID}]");
            continue;
        }
        if in_zenith && let Some(token_text) = trimmed.strip_prefix("experimental_bearer_token = ")
        {
            let api_key = unquote_toml_string(token_text.trim())?;
            if !api_key.is_empty() {
                return Some(api_key);
            }
        }
    }
    None
}

fn load_codex_auth_key() -> Option<String> {
    let auth_text = std::fs::read_to_string(default_codex_home().join(AUTH_FILE)).ok()?;
    let auth: serde_json::Value = serde_json::from_str(&auth_text).ok()?;
    let api_key = auth.get("OPENAI_API_KEY")?.as_str()?.trim().to_string();
    (!api_key.is_empty()).then_some(api_key)
}
