use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    files::{atomic_write, escape_json_string, escape_toml_string, unquote_toml_string},
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
const BACKUP_DIR: &str = "zenith-backups";
const BACKUP_SUFFIX: &str = ".zenith.bak";
const DEFAULT_MODEL_PROVIDER: &str = "openai";

pub fn enable_provider(api_key: &str) -> Result<(), String> {
    if api_key.is_empty() {
        return Err("Введите API key.".to_string());
    }

    let codex_home = default_codex_home();
    fs::create_dir_all(&codex_home)
        .map_err(|err| format!("Не удалось создать {}: {err}", codex_home.display()))?;
    migrate_legacy_backups(&codex_home)?;

    let config_path = codex_home.join(CONFIG_FILE);
    let original = fs::read_to_string(&config_path).unwrap_or_default();
    let next = upsert_zenith_provider(&original);
    if next != original {
        backup_config(&config_path, &original)?;
        atomic_write(&config_path, &next)?;
    }
    save_previous_auth_if_needed(&original)?;
    write_codex_auth(api_key)
}

pub fn ensure_provider_on_launch() -> Result<(), String> {
    if let Some(api_key) = load_saved_app_key() {
        enable_provider(&api_key)?;
    } else if let Some(api_key) = load_codex_auth_key().or_else(load_zenith_key_from_codex_config) {
        save_app_key(&api_key)?;
        enable_provider(&api_key)?;
    }
    Ok(())
}

pub fn reset_provider() -> Result<(), String> {
    let codex_home = default_codex_home();
    migrate_legacy_backups(&codex_home)?;
    let config_path = codex_home.join(CONFIG_FILE);
    let original = fs::read_to_string(&config_path).unwrap_or_default();
    let previous_model_provider = latest_backup_model_provider(&codex_home);
    let mut next = remove_zenith_provider(&original);

    let model_provider =
        previous_model_provider.unwrap_or_else(|| DEFAULT_MODEL_PROVIDER.to_string());
    next = with_model_provider(next, &model_provider);

    if next != original {
        atomic_write(&config_path, next.trim_start())?;
    }

    restore_or_remove_zenith_auth(&original)?;
    delete_saved_app_key();
    Ok(())
}

pub fn provider_has_token() -> bool {
    let config_path = default_codex_home().join(CONFIG_FILE);
    let content = fs::read_to_string(config_path).unwrap_or_default();
    content.lines().any(|line| {
        line.trim()
            .eq_ignore_ascii_case(&format!("model_provider = \"{PROVIDER_ID}\""))
    }) && content.contains(&format!("[model_providers.{PROVIDER_ID}]"))
        && load_api_key_for_launch().is_some()
}

pub fn load_api_key_for_launch() -> Option<String> {
    load_saved_app_key()
        .or_else(load_codex_auth_key)
        .or_else(load_zenith_key_from_codex_config)
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

fn write_codex_auth(api_key: &str) -> Result<(), String> {
    let codex_home = default_codex_home();
    fs::create_dir_all(&codex_home)
        .map_err(|err| format!("Не удалось создать {}: {err}", codex_home.display()))?;
    let content = format!(
        "{{\n  \"OPENAI_API_KEY\": \"{}\",\n  \"auth_mode\": \"apikey\"\n}}\n",
        escape_json_string(api_key)
    );
    atomic_write(&codex_home.join(AUTH_FILE), &content)
}

fn save_previous_auth_if_needed(config_before_enable: &str) -> Result<(), String> {
    if load_previous_codex_auth().is_some() {
        return Ok(());
    }
    let auth_path = default_codex_home().join(AUTH_FILE);
    let Ok(content) = fs::read_to_string(&auth_path) else {
        return Ok(());
    };
    if content.trim().is_empty() {
        return Ok(());
    }
    let Ok(auth) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Ok(());
    };
    if !previous_codex_auth_should_be_saved(
        &auth,
        load_saved_app_key().as_deref(),
        config_before_enable,
    ) {
        return Ok(());
    }
    save_previous_codex_auth(&content)
}

fn previous_codex_auth_should_be_saved(
    auth: &serde_json::Value,
    saved_key: Option<&str>,
    config_before_enable: &str,
) -> bool {
    auth.is_object() && !zenith_auth_is_owned(auth, saved_key, config_before_enable)
}

fn restore_or_remove_zenith_auth(config_before_reset: &str) -> Result<(), String> {
    if let Some(previous_auth) = load_previous_codex_auth() {
        let codex_home = default_codex_home();
        fs::create_dir_all(&codex_home)
            .map_err(|err| format!("Не удалось создать {}: {err}", codex_home.display()))?;
        atomic_write(&codex_home.join(AUTH_FILE), &previous_auth)?;
        delete_previous_codex_auth();
        return Ok(());
    }
    remove_zenith_auth_if_owned(config_before_reset);
    Ok(())
}

fn remove_zenith_auth_if_owned(config_before_reset: &str) {
    let saved_key = load_saved_app_key();
    let auth_path = default_codex_home().join(AUTH_FILE);
    let Ok(content) = fs::read_to_string(&auth_path) else {
        return;
    };
    let Ok(auth) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };
    if zenith_auth_is_owned(&auth, saved_key.as_deref(), config_before_reset) {
        let _ = fs::remove_file(auth_path);
    }
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

fn config_uses_zenith_provider(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.eq_ignore_ascii_case(&format!("model_provider = \"{PROVIDER_ID}\""))
            || trimmed.eq_ignore_ascii_case(&format!("model_provider = \"{LEGACY_PROVIDER_ID}\""))
            || trimmed == format!("[model_providers.{PROVIDER_ID}]")
            || trimmed == format!("[model_providers.{LEGACY_PROVIDER_ID}]")
    })
}

fn is_zenith_customer_key(key: &str) -> bool {
    key.starts_with("znt_") || key.starts_with("zrk_")
}

fn upsert_zenith_provider(original: &str) -> String {
    let without_old = remove_zenith_provider(original);
    let without_model_provider = remove_key_line(&without_old, "model_provider");
    let mut result = format!("model_provider = \"{PROVIDER_ID}\"");
    let preserved = without_model_provider.trim();
    if !preserved.is_empty() {
        result.push_str("\n\n");
        result.push_str(preserved);
    }
    result.push_str("\n\n");
    result.push_str(&format!("[model_providers.{PROVIDER_ID}]\n"));
    result.push_str(&format!("name = \"{PROVIDER_NAME}\"\n"));
    result.push_str(&format!(
        "base_url = \"{}\"\n",
        escape_toml_string(BASE_URL)
    ));
    result.push_str("wire_api = \"responses\"\n");
    result.push_str("requires_openai_auth = true\n");
    result.push_str("supports_websockets = false\n");
    result
}

fn remove_zenith_provider(original: &str) -> String {
    let without_section = remove_table(original, &format!("[model_providers.{PROVIDER_ID}]"));
    let without_section = remove_table(
        &without_section,
        &format!("[model_providers.{LEGACY_PROVIDER_ID}]"),
    );
    let without_model_provider = remove_key_line(&without_section, "model_provider");
    remove_zenith_openai_base_url_override(&without_model_provider)
}

fn remove_key_line(content: &str, key: &str) -> String {
    let prefix = format!("{key} =");
    content
        .lines()
        .filter(|line| !line.trim().starts_with(&prefix))
        .collect::<Vec<_>>()
        .join("\n")
}

fn with_model_provider(content: String, model_provider: &str) -> String {
    let without_model_provider = remove_key_line(&content, "model_provider");
    let preserved = without_model_provider.trim().to_string();
    let mut next = format!(
        "model_provider = \"{}\"",
        escape_toml_string(model_provider)
    );
    if !preserved.is_empty() {
        next.push_str("\n\n");
        next.push_str(&preserved);
    }
    next
}

fn remove_zenith_openai_base_url_override(content: &str) -> String {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            let Some(value) = trimmed.strip_prefix("openai_base_url = ") else {
                return true;
            };
            !unquote_toml_string(value.trim())
                .is_some_and(|url| url.trim_end_matches('/') == BASE_URL)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn latest_backup_model_provider(codex_home: &Path) -> Option<String> {
    backup_paths_newest_first(codex_home)
        .into_iter()
        .find_map(|path| {
            let content = fs::read_to_string(path).ok()?;
            read_model_provider(&content)
        })
}

fn backup_paths_newest_first(codex_home: &Path) -> Vec<PathBuf> {
    let mut backups = backup_search_dirs(codex_home)
        .into_iter()
        .flat_map(|directory| {
            fs::read_dir(directory)
                .ok()
                .into_iter()
                .flat_map(|entries| entries.filter_map(Result::ok))
        })
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_string_lossy();
            is_zenith_backup_name(&name).then_some((backup_timestamp_from_name(&name), path))
        })
        .collect::<Vec<_>>();
    backups.sort_by(
        |(left_timestamp, left_path), (right_timestamp, right_path)| {
            right_timestamp
                .cmp(left_timestamp)
                .then_with(|| right_path.cmp(left_path))
        },
    );
    backups.into_iter().map(|(_, path)| path).collect()
}

fn read_model_provider(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let value = line.trim().strip_prefix("model_provider = ")?;
        let provider = unquote_toml_string(value.trim())?;
        (!provider.eq_ignore_ascii_case(PROVIDER_ID)
            && !provider.eq_ignore_ascii_case(LEGACY_PROVIDER_ID)
            && !provider.is_empty())
        .then_some(provider)
    })
}

fn remove_table(content: &str, header: &str) -> String {
    let mut skipping = false;
    let mut out = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == header {
            skipping = true;
            continue;
        }
        if skipping && trimmed.starts_with('[') && trimmed.ends_with(']') {
            skipping = false;
        }
        if !skipping {
            out.push(line);
        }
    }

    out.join("\n")
}

fn backup_config(config_path: &Path, content: &str) -> Result<(), String> {
    if content.trim().is_empty() {
        return Ok(());
    }
    let codex_home = config_path.parent().unwrap_or_else(|| Path::new("."));
    let backup_dir = backup_dir(codex_home);
    fs::create_dir_all(&backup_dir)
        .map_err(|err| format!("Не удалось создать {}: {err}", backup_dir.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("Ошибка времени: {err}"))?
        .as_secs();
    let backup_path = next_backup_path(&backup_dir, timestamp);
    fs::write(&backup_path, redact_config_secrets(content))
        .map_err(|err| format!("Не удалось создать backup {}: {err}", backup_path.display()))
}

fn backup_dir(codex_home: &Path) -> PathBuf {
    codex_home.join(BACKUP_DIR)
}

fn backup_search_dirs(codex_home: &Path) -> Vec<PathBuf> {
    vec![backup_dir(codex_home), codex_home.to_path_buf()]
}

fn is_zenith_backup_name(name: &str) -> bool {
    name.starts_with(&format!("{CONFIG_FILE}."))
        && name.ends_with(BACKUP_SUFFIX)
        && name.len() > CONFIG_FILE.len() + BACKUP_SUFFIX.len() + 1
}

fn backup_timestamp_from_name(name: &str) -> u64 {
    name.trim_start_matches(&format!("{CONFIG_FILE}."))
        .trim_end_matches(BACKUP_SUFFIX)
        .split('-')
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
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

fn migrate_legacy_backups(codex_home: &Path) -> Result<(), String> {
    let Ok(entries) = fs::read_dir(codex_home) else {
        return Ok(());
    };
    let backup_dir = backup_dir(codex_home);
    fs::create_dir_all(&backup_dir)
        .map_err(|err| format!("Не удалось создать {}: {err}", backup_dir.display()))?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_zenith_backup_name(name) {
            continue;
        }
        let target = backup_dir.join(name);
        if target == path {
            continue;
        }
        let target = if target.exists() {
            next_backup_path(&backup_dir, backup_timestamp_from_name(name))
        } else {
            target
        };
        fs::rename(&path, &target)
            .or_else(|_| fs::copy(&path, &target).map(|_| ()))
            .map_err(|err| format!("Не удалось перенести backup {}: {err}", path.display()))?;
        let _ = fs::remove_file(&path);
    }
    Ok(())
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
    fn latest_backup_model_provider_skips_newer_zenith_backup() {
        let codex_home = temp_codex_home("latest-backup");
        let backups = backup_dir(&codex_home);
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
            latest_backup_model_provider(&codex_home).as_deref(),
            Some("openai")
        );

        let _ = fs::remove_dir_all(codex_home);
    }

    #[test]
    fn backup_config_writes_into_dedicated_backup_directory() {
        let codex_home = temp_codex_home("backup-dir");
        fs::create_dir_all(&codex_home).expect("codex home");
        let config_path = codex_home.join(CONFIG_FILE);

        backup_config(&config_path, r#"model_provider = "openai""#).expect("backup");

        let backups = backup_paths_newest_first(&codex_home);
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].parent(), Some(backup_dir(&codex_home).as_path()));
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
    }

    #[test]
    fn migrate_legacy_backups_moves_root_backups_into_dedicated_directory() {
        let codex_home = temp_codex_home("migrate-backups");
        fs::create_dir_all(&codex_home).expect("codex home");
        let legacy_name = format!("{CONFIG_FILE}.123{BACKUP_SUFFIX}");
        fs::write(
            codex_home.join(&legacy_name),
            r#"model_provider = "openai""#,
        )
        .expect("legacy backup");

        migrate_legacy_backups(&codex_home).expect("migrate");

        assert!(!codex_home.join(&legacy_name).exists());
        assert!(backup_dir(&codex_home).join(&legacy_name).exists());

        let _ = fs::remove_dir_all(codex_home);
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
}
