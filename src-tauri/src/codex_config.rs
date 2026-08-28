#[path = "codex_config_text.rs"]
mod codex_config_text;

use codex_config_text::{
    backup_paths_from_directories, config_selects_zenith_provider, config_uses_zenith_provider,
    is_zenith_customer_key, latest_backup_model_provider, remove_zenith_provider,
    upsert_zenith_provider, with_model_provider,
};
#[cfg(test)]
use codex_config_text::{backup_paths_newest_first, remove_zenith_openai_base_url_override};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    files::{atomic_write, escape_json_string, unquote_toml_string},
    key_storage::{
        delete_previous_codex_auth, delete_saved_app_key, load_previous_codex_auth,
        load_saved_app_key, save_app_key, save_previous_codex_auth,
    },
    platform::default_codex_home,
};

const PROVIDER_ID: &str = "codex_local_access";
const LEGACY_PROVIDER_ID: &str = "zenith";
const PROVIDER_NAME: &str = "Zenith";
const BASE_URL: &str = "https://api.zenithmarket.dev/v1";
const CONFIG_FILE: &str = "config.toml";
const AUTH_FILE: &str = "auth.json";
const BACKUP_SUFFIX: &str = ".zenith.bak";
const MAX_CONFIG_BACKUPS: usize = 3;
const DEFAULT_MODEL_PROVIDER: &str = "openai";
const LOCAL_POOL_PROVIDER_ID: &str = "zenith_relay_local";
static CODEX_PROFILE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn lock_codex_profile() -> MutexGuard<'static, ()> {
    CODEX_PROFILE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn enable_provider(api_key: &str, backup_dir: &Path) -> Result<(), String> {
    if api_key.is_empty() {
        return Err("Введите API key.".to_string());
    }
    let _profile_guard = lock_codex_profile();

    let codex_home = default_codex_home();
    let config_path = codex_home.join(CONFIG_FILE);
    let auth_path = codex_home.join(AUTH_FILE);
    let original_config = read_optional_text(&config_path)?;
    let original_auth = read_optional_text(&auth_path)?;
    let original = original_config.as_deref().unwrap_or_default();
    ensure_ready_api_profile_is_inactive(original)?;
    fs::create_dir_all(&codex_home)
        .map_err(|err| format!("Не удалось создать {}: {err}", codex_home.display()))?;
    prune_config_backups(backup_dir)?;
    let next = upsert_zenith_provider(original);
    if next != original {
        backup_config(backup_dir, original)?;
        replace_if_unchanged(&config_path, original_config.as_deref(), &next)?;
    }
    let managed_auth = codex_auth_content(api_key);
    if let Err(error) = replace_if_unchanged(&auth_path, original_auth.as_deref(), &managed_auth) {
        return Err(with_cleanup(
            error,
            rollback_ready_config(
                next != original,
                &config_path,
                &next,
                original_config.as_deref(),
            ),
        ));
    }
    let saved_previous_auth = match save_previous_auth_if_needed(original, original_auth.as_deref())
    {
        Ok(saved) => saved,
        Err(error) => {
            let auth_rollback =
                rollback_file(&auth_path, Some(&managed_auth), original_auth.as_deref());
            let config_rollback = rollback_ready_config(
                next != original,
                &config_path,
                &next,
                original_config.as_deref(),
            );
            return Err(with_cleanup(
                error,
                merge_cleanup(auth_rollback, config_rollback),
            ));
        }
    };
    if let Err(error) = ensure_unchanged(&auth_path, Some(&managed_auth)) {
        let config_rollback = rollback_ready_config(
            next != original,
            &config_path,
            &next,
            original_config.as_deref(),
        );
        let secret_cleanup = if saved_previous_auth {
            delete_previous_codex_auth()
        } else {
            Ok(())
        };
        return Err(with_cleanup(
            error,
            merge_cleanup(config_rollback, secret_cleanup),
        ));
    }
    let expected_config = if next != original {
        Some(next.as_str())
    } else {
        original_config.as_deref()
    };
    if let Err(error) = ensure_unchanged(&config_path, expected_config) {
        let auth_rollback =
            rollback_file(&auth_path, Some(&managed_auth), original_auth.as_deref());
        let secret_cleanup = if saved_previous_auth {
            delete_previous_codex_auth()
        } else {
            Ok(())
        };
        return Err(with_cleanup(
            error,
            merge_cleanup(auth_rollback, secret_cleanup),
        ));
    }
    Ok(())
}

pub fn ensure_provider_on_launch(backup_dir: &Path) -> Result<(), String> {
    prune_config_backups(backup_dir)?;
    if current_config_uses_local_pool_provider()? {
        return Ok(());
    }
    let config = read_optional_text(&default_codex_home().join(CONFIG_FILE))?.unwrap_or_default();
    if !config_selects_zenith_provider(&config) {
        return Ok(());
    }
    if let Some(api_key) = load_saved_app_key() {
        enable_provider(&api_key, backup_dir)?;
    } else if let Some(api_key) =
        load_zenith_key_from_codex_config().or_else(load_zenith_auth_key_if_configured)
    {
        save_app_key(&api_key)?;
        enable_provider(&api_key, backup_dir)?;
    }
    Ok(())
}

fn current_config_uses_local_pool_provider() -> Result<bool, String> {
    let config_path = default_codex_home().join(CONFIG_FILE);
    Ok(read_optional_text(&config_path)?
        .as_deref()
        .is_some_and(config_uses_local_pool_provider))
}

fn config_uses_local_pool_provider(content: &str) -> bool {
    content.lines().any(|line| {
        let line = line.trim();
        line.eq_ignore_ascii_case(&format!("model_provider = \"{LOCAL_POOL_PROVIDER_ID}\""))
            || line == format!("[model_providers.{LOCAL_POOL_PROVIDER_ID}]")
    })
}

fn ensure_ready_api_profile_is_inactive(content: &str) -> Result<(), String> {
    if config_uses_local_pool_provider(content) {
        return Err(
            "ChatGPT подключён к Local Pool. Сначала восстановите профиль Local Pool.".to_string(),
        );
    }
    Ok(())
}

fn read_optional_text(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Не удалось прочитать {}: {error}", path.display())),
    }
}

fn replace_if_unchanged(path: &Path, expected: Option<&str>, content: &str) -> Result<(), String> {
    ensure_unchanged(path, expected)?;
    atomic_write(path, content)
}

fn ensure_unchanged(path: &Path, expected: Option<&str>) -> Result<(), String> {
    if read_optional_text(path)?.as_deref() != expected {
        return Err(profile_changed_error(path));
    }
    Ok(())
}

fn remove_if_unchanged(path: &Path, expected: Option<&str>) -> Result<(), String> {
    if read_optional_text(path)?.as_deref() != expected {
        return Err(profile_changed_error(path));
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && expected.is_none() => Ok(()),
        Err(error) => Err(format!("Не удалось удалить {}: {error}", path.display())),
    }
}

fn rollback_file(
    path: &Path,
    expected_current: Option<&str>,
    previous: Option<&str>,
) -> Result<(), String> {
    match previous {
        Some(content) => replace_if_unchanged(path, expected_current, content),
        None => remove_if_unchanged(path, expected_current),
    }
}

fn rollback_ready_config(
    changed: bool,
    path: &Path,
    current: &str,
    previous: Option<&str>,
) -> Result<(), String> {
    if changed {
        rollback_file(path, Some(current), previous)
    } else {
        Ok(())
    }
}

fn merge_cleanup(first: Result<(), String>, second: Result<(), String>) -> Result<(), String> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(format!("{first}; {second}")),
    }
}

fn with_cleanup(error: String, cleanup: Result<(), String>) -> String {
    match cleanup {
        Ok(()) => error,
        Err(cleanup_error) => format!("{error}; rollback failed: {cleanup_error}"),
    }
}

fn profile_changed_error(path: &Path) -> String {
    format!(
        "ChatGPT изменил {} во время обновления; изменения Zenith Relay не применены.",
        path.display()
    )
}

pub fn deactivate_provider(backup_dir: &Path) -> Result<(), String> {
    restore_provider(backup_dir, false)
}

pub fn reset_provider(backup_dir: &Path) -> Result<(), String> {
    restore_provider(backup_dir, true)
}

fn restore_provider(backup_dir: &Path, forget_key: bool) -> Result<(), String> {
    let _profile_guard = lock_codex_profile();
    let codex_home = default_codex_home();
    let config_path = codex_home.join(CONFIG_FILE);
    let auth_path = codex_home.join(AUTH_FILE);
    let original_config = read_optional_text(&config_path)?;
    let original_auth = read_optional_text(&auth_path)?;
    let original = original_config.as_deref().unwrap_or_default();
    ensure_ready_api_profile_is_inactive(original)?;
    if !config_selects_zenith_provider(original) {
        if forget_key {
            delete_saved_app_key()?;
        }
        return Ok(());
    }
    let previous_model_provider = latest_backup_model_provider(backup_dir);
    let mut next = remove_zenith_provider(original);

    let model_provider =
        previous_model_provider.unwrap_or_else(|| DEFAULT_MODEL_PROVIDER.to_string());
    next = with_model_provider(next, &model_provider);
    let saved_key = load_saved_app_key();
    if forget_key {
        delete_saved_app_key()?;
    }
    let reset_result = (|| {
        if next != original {
            replace_if_unchanged(&config_path, original_config.as_deref(), next.trim_start())?;
        }
        if let Err(error) =
            restore_or_remove_zenith_auth(original, original_auth.as_deref(), saved_key.as_deref())
        {
            return Err(with_cleanup(
                error,
                rollback_ready_config(
                    next != original,
                    &config_path,
                    next.trim_start(),
                    original_config.as_deref(),
                ),
            ));
        }
        Ok(())
    })();
    if let Err(error) = reset_result {
        return Err(with_cleanup(
            error,
            if forget_key {
                saved_key.as_deref().map_or(Ok(()), save_app_key)
            } else {
                Ok(())
            },
        ));
    }
    Ok(())
}

pub fn provider_has_token() -> bool {
    let config_path = default_codex_home().join(CONFIG_FILE);
    let content = fs::read_to_string(config_path).unwrap_or_default();
    config_selects_zenith_provider(&content)
        && content.contains(&format!("[model_providers.{PROVIDER_ID}]"))
        && load_api_key_for_launch().is_some()
}

pub fn load_api_key_for_launch() -> Option<String> {
    load_saved_app_key()
        .or_else(load_zenith_key_from_codex_config)
        .or_else(load_zenith_auth_key_if_configured)
}

fn load_zenith_auth_key_if_configured() -> Option<String> {
    let config_path = default_codex_home().join(CONFIG_FILE);
    let config = fs::read_to_string(config_path).ok()?;
    zenith_auth_key_if_configured(&config, load_codex_auth_key())
}

fn zenith_auth_key_if_configured(config: &str, key: Option<String>) -> Option<String> {
    config_uses_zenith_provider(config)
        .then_some(key)
        .flatten()
        .filter(|key| is_zenith_customer_key(key))
}

fn load_zenith_key_from_codex_config() -> Option<String> {
    let config_path = default_codex_home().join(CONFIG_FILE);
    let content = fs::read_to_string(config_path).ok()?;
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
    let content = fs::read_to_string(auth_path).ok()?;
    let auth: serde_json::Value = serde_json::from_str(&content).ok()?;
    let key = auth
        .get("OPENAI_API_KEY")
        .and_then(serde_json::Value::as_str)?
        .trim()
        .to_string();
    (!key.is_empty()).then_some(key)
}

fn codex_auth_content(api_key: &str) -> String {
    format!(
        "{{\n  \"OPENAI_API_KEY\": \"{}\",\n  \"auth_mode\": \"apikey\"\n}}\n",
        escape_json_string(api_key)
    )
}

fn save_previous_auth_if_needed(
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

fn previous_codex_auth_should_be_saved(
    auth: &serde_json::Value,
    saved_key: Option<&str>,
    config_before_enable: &str,
) -> bool {
    auth.is_object() && !zenith_auth_is_owned(auth, saved_key, config_before_enable)
}

fn restore_or_remove_zenith_auth(
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

fn zenith_auth_is_owned(
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

fn backup_config(backup_dir: &Path, content: &str) -> Result<(), String> {
    if content.trim().is_empty() {
        return Ok(());
    }
    fs::create_dir_all(backup_dir)
        .map_err(|err| format!("Не удалось создать {}: {err}", backup_dir.display()))?;
    let redacted = redact_config_secrets(content);
    let existing = backup_paths_from_directories([backup_dir.to_path_buf()]);
    if existing
        .first()
        .and_then(|path| fs::read_to_string(path).ok())
        .as_deref()
        == Some(redacted.as_str())
    {
        return prune_config_backups(backup_dir);
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("Ошибка времени: {err}"))?
        .as_secs();
    let backup_path = next_backup_path(backup_dir, timestamp);
    fs::write(&backup_path, redacted)
        .map_err(|err| format!("Не удалось создать backup {}: {err}", backup_path.display()))?;
    prune_config_backups(backup_dir)
}

fn prune_config_backups(backup_dir: &Path) -> Result<(), String> {
    for path in backup_paths_from_directories([backup_dir.to_path_buf()])
        .into_iter()
        .skip(MAX_CONFIG_BACKUPS)
    {
        fs::remove_file(&path)
            .map_err(|err| format!("Не удалось удалить старый backup {}: {err}", path.display()))?;
    }
    Ok(())
}

fn next_backup_path(backup_dir: &Path, timestamp: u64) -> PathBuf {
    let first = backup_dir.join(format!("{CONFIG_FILE}.{timestamp}{BACKUP_SUFFIX}"));
    if !first.exists() {
        return first;
    }
    (1..)
        .map(|index| backup_dir.join(format!("{CONFIG_FILE}.{timestamp}-{index}{BACKUP_SUFFIX}")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

fn redact_config_secrets(content: &str) -> String {
    content
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("experimental_bearer_token =") {
                "experimental_bearer_token = \"<redacted>\"".to_string()
            } else {
                redact_inline_tokens(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_inline_tokens(line: &str) -> String {
    let mut redacted = line.to_string();
    for marker in ["znt_", "zrk_", "sk-"] {
        while let Some(start) = redacted.find(marker) {
            let end = redacted[start..]
                .find(|ch: char| {
                    ch.is_whitespace()
                        || matches!(ch, '"' | '\'' | ',' | ';' | ')' | ']' | '}' | '<' | '>')
                })
                .map(|offset| start + offset)
                .unwrap_or_else(|| redacted.len());
            redacted.replace_range(start..end, "<redacted>");
        }
    }
    redacted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, thread, time::Duration};

    #[test]
    fn upsert_zenith_provider_replaces_model_provider_and_preserves_other_tables() {
        let original = r#"
model_provider = "openai"

[profiles.default]
model = "gpt-5"

[model_providers.zenith]
name = "Old"
base_url = "https://old.example/v1"
"#;

        let next = upsert_zenith_provider(original);

        assert!(next.contains(r#"model_provider = "codex_local_access""#));
        assert!(next.contains("[model_providers.codex_local_access]"));
        assert!(next.contains(r#"base_url = "https://api.zenithmarket.dev/v1""#));
        assert!(next.contains("supports_websockets = true"));
        assert!(next.contains("[profiles.default]"));
        assert!(!next.contains("[model_providers.zenith]"));
        assert!(!next.contains(r#"model_provider = "openai""#));
    }

    #[test]
    fn remove_zenith_provider_keeps_unrelated_provider_config() {
        let original = r#"
model_provider = "codex_local_access"
openai_base_url = "https://api.zenithmarket.dev/v1"

[model_providers.codex_local_access]
name = "Zenith"
base_url = "https://api.zenithmarket.dev/v1"

[model_providers.openai]
name = "OpenAI"
base_url = "https://gateway.example/v1"
"#;

        let next = remove_zenith_provider(original);

        assert!(next.contains("[model_providers.openai]"));
        assert!(next.contains(r#"base_url = "https://gateway.example/v1""#));
        assert!(!next.contains("[model_providers.codex_local_access]"));
        assert!(!next.contains(r#"model_provider = "codex_local_access""#));
        assert!(!next.contains(r#"openai_base_url = "https://api.zenithmarket.dev/v1""#));
    }

    #[test]
    fn with_model_provider_defaults_reset_to_openai_when_no_backup_exists() {
        let next = with_model_provider(
            r#"
[profiles.default]
model = "gpt-5.5"
"#
            .to_string(),
            DEFAULT_MODEL_PROVIDER,
        );

        assert!(next.starts_with(r#"model_provider = "openai""#));
        assert!(next.contains("[profiles.default]"));
    }

    #[test]
    fn remove_zenith_openai_base_url_override_keeps_other_openai_base_url() {
        let original = r#"
openai_base_url = "https://us.api.openai.com/v1"
model = "gpt-5.5"
"#;

        let next = remove_zenith_openai_base_url_override(original);

        assert!(next.contains(r#"openai_base_url = "https://us.api.openai.com/v1""#));
        assert!(next.contains(r#"model = "gpt-5.5""#));
    }

    #[test]
    fn zenith_auth_owned_when_saved_key_matches() {
        let auth = serde_json::json!({
            "OPENAI_API_KEY": "custom-key",
            "auth_mode": "apikey"
        });

        assert!(zenith_auth_is_owned(&auth, Some("custom-key"), ""));
    }

    #[test]
    fn zenith_auth_owned_when_config_is_zenith_and_key_storage_is_missing() {
        let auth = serde_json::json!({
            "OPENAI_API_KEY": "znt_customer_key",
            "auth_mode": "apikey"
        });
        let config = r#"
model_provider = "codex_local_access"

[model_providers.codex_local_access]
name = "Zenith"
"#;

        assert!(zenith_auth_is_owned(&auth, None, config));
    }

    #[test]
    fn zenith_auth_does_not_remove_unrelated_api_key_without_saved_match() {
        let auth = serde_json::json!({
            "OPENAI_API_KEY": "sk-user-openai-key",
            "auth_mode": "apikey"
        });
        let config = r#"
model_provider = "openai"

[model_providers.openai]
name = "OpenAI"
"#;

        assert!(!zenith_auth_is_owned(&auth, None, config));
    }

    #[test]
    fn zenith_auth_requires_apikey_mode() {
        let auth = serde_json::json!({
            "OPENAI_API_KEY": "znt_customer_key",
            "auth_mode": "chatgpt"
        });
        let config = r#"model_provider = "codex_local_access""#;

        assert!(!zenith_auth_is_owned(&auth, None, config));
    }

    #[test]
    fn previous_codex_auth_saves_chatgpt_session_shape() {
        let auth = serde_json::json!({
            "tokens": {
                "access_token": "chatgpt-session-token"
            },
            "auth_mode": "chatgpt"
        });

        assert!(previous_codex_auth_should_be_saved(&auth, None, ""));
    }

    #[test]
    fn previous_codex_auth_saves_openai_api_key_shape() {
        let auth = serde_json::json!({
            "OPENAI_API_KEY": "sk-user-openai-key",
            "auth_mode": "apikey"
        });

        assert!(previous_codex_auth_should_be_saved(&auth, None, ""));
    }

    #[test]
    fn previous_codex_auth_does_not_save_current_zenith_key() {
        let auth = serde_json::json!({
            "OPENAI_API_KEY": "znt_customer_key",
            "auth_mode": "apikey"
        });
        let config = r#"model_provider = "codex_local_access""#;

        assert!(!previous_codex_auth_should_be_saved(&auth, None, config));
    }

    #[test]
    fn local_pool_key_is_not_a_zenith_customer_key() {
        assert!(!is_zenith_customer_key("zlr_local_generated_key"));
        assert!(!is_zenith_customer_key("zrk_retired_reseller_key"));
        assert!(is_zenith_customer_key("znt_customer_key"));
        assert!(zenith_auth_key_if_configured(
            "model_provider = \"zenith_relay_local\"",
            Some("zlr_local_generated_key".into())
        )
        .is_none());
        assert_eq!(
            zenith_auth_key_if_configured(
                "model_provider = \"codex_local_access\"",
                Some("znt_customer_key".into())
            )
            .as_deref(),
            Some("znt_customer_key")
        );
        assert!(config_uses_local_pool_provider(
            "model_provider = \"zenith_relay_local\"\n\n[model_providers.zenith_relay_local]"
        ));
    }

    #[test]
    fn saved_key_does_not_select_ready_api_by_itself() {
        assert!(config_selects_zenith_provider(
            "model_provider = \"codex_local_access\""
        ));
        assert!(config_selects_zenith_provider(
            "model_provider = \"zenith\""
        ));
        assert!(!config_selects_zenith_provider(
            "model_provider = \"openai\"\n\n[model_providers.codex_local_access]"
        ));
    }

    #[test]
    fn ready_api_guard_rejects_active_local_pool_profile() {
        let error = ensure_ready_api_profile_is_inactive(
            "model_provider = \"zenith_relay_local\"\n\n[model_providers.zenith_relay_local]",
        )
        .unwrap_err();

        assert!(error.contains("Local Pool"));
    }

    #[test]
    fn profile_reads_distinguish_missing_and_invalid_utf8() {
        let root = temp_codex_home("profile-read-errors");
        let path = root.join(CONFIG_FILE);
        assert_eq!(read_optional_text(&path).unwrap(), None);
        fs::create_dir_all(&root).unwrap();
        fs::write(&path, [0xff, 0xfe]).unwrap();

        assert!(read_optional_text(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), [0xff, 0xfe]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compare_before_write_preserves_external_change() {
        let root = temp_codex_home("profile-compare");
        let path = root.join(CONFIG_FILE);
        fs::create_dir_all(&root).unwrap();
        fs::write(&path, "model_provider = \"openai\"\n").unwrap();
        let original = read_optional_text(&path).unwrap();
        let changed = "model_provider = \"custom\"\n";
        fs::write(&path, changed).unwrap();

        assert!(replace_if_unchanged(&path, original.as_deref(), "replacement").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), changed);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profile_lock_serializes_mutations() {
        let guard = lock_codex_profile();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _guard = lock_codex_profile();
            sender.send(()).unwrap();
        });

        assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
        drop(guard);
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn latest_backup_model_provider_skips_newer_zenith_backup() {
        let codex_home = temp_codex_home("latest-backup");
        let backups = managed_backup_dir(&codex_home);
        fs::create_dir_all(&backups).expect("backup dir");
        fs::write(
            backups.join(format!("{CONFIG_FILE}.100{BACKUP_SUFFIX}")),
            r#"model_provider = "openai""#,
        )
        .expect("old backup");
        fs::write(
            backups.join(format!("{CONFIG_FILE}.200{BACKUP_SUFFIX}")),
            r#"model_provider = "codex_local_access""#,
        )
        .expect("new backup");

        assert_eq!(
            latest_backup_model_provider(&backups).as_deref(),
            Some("openai")
        );

        let _ = fs::remove_dir_all(codex_home);
        let _ = fs::remove_dir_all(backups);
    }

    #[test]
    fn backup_config_writes_into_dedicated_backup_directory() {
        let codex_home = temp_codex_home("backup-dir");
        let backup_dir = managed_backup_dir(&codex_home);
        fs::create_dir_all(&codex_home).expect("codex home");

        backup_config(&backup_dir, r#"model_provider = "openai""#).expect("backup");

        let backups = backup_paths_newest_first(&backup_dir);
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].parent(), Some(backup_dir.as_path()));
        assert!(
            fs::read_dir(&codex_home)
                .expect("codex entries")
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{CONFIG_FILE}.")))
                .count()
                == 0
        );

        let _ = fs::remove_dir_all(codex_home);
        let _ = fs::remove_dir_all(backup_dir);
    }

    #[test]
    fn config_backups_deduplicate_and_keep_last_three() {
        let codex_home = temp_codex_home("backup-retention");
        let backup_dir = managed_backup_dir(&codex_home);
        fs::create_dir_all(&backup_dir).expect("managed backup dir");
        for timestamp in 100..112 {
            fs::write(
                backup_dir.join(format!("{CONFIG_FILE}.{timestamp}{BACKUP_SUFFIX}")),
                format!("model_provider = \"provider-{timestamp}\""),
            )
            .expect("backup");
        }

        prune_config_backups(&backup_dir).expect("prune");
        let backups = backup_paths_from_directories([backup_dir.clone()]);
        assert_eq!(backups.len(), MAX_CONFIG_BACKUPS);
        assert!(backup_dir
            .join(format!("{CONFIG_FILE}.111{BACKUP_SUFFIX}"))
            .exists());
        assert!(!backup_dir
            .join(format!("{CONFIG_FILE}.101{BACKUP_SUFFIX}"))
            .exists());

        backup_config(&backup_dir, r#"model_provider = "provider-111""#)
            .expect("deduplicated backup");
        assert_eq!(
            backup_paths_from_directories([backup_dir.clone()]).len(),
            MAX_CONFIG_BACKUPS
        );

        let _ = fs::remove_dir_all(codex_home);
        let _ = fs::remove_dir_all(backup_dir);
    }

    #[test]
    fn redact_config_secrets_hides_inline_tokens() {
        let original = r#"
[model_providers.codex_local_access]
experimental_bearer_token = "znt_secret"
notes = "manual token zrk_customer_secret and sk-secret"
"#;

        let redacted = redact_config_secrets(original);

        assert!(redacted.contains(r#"experimental_bearer_token = "<redacted>""#));
        assert!(!redacted.contains("znt_secret"));
        assert!(!redacted.contains("zrk_customer_secret"));
        assert!(!redacted.contains("sk-secret"));
    }

    fn temp_codex_home(name: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("zenith-relay-{name}-{timestamp}"))
    }

    fn managed_backup_dir(codex_home: &Path) -> PathBuf {
        codex_home.with_extension("app-backups")
    }
}
