#[path = "codex_config_auth.rs"]
mod codex_config_auth;
#[path = "codex_config_backup.rs"]
mod codex_config_backup;
#[path = "codex_config_text.rs"]
mod codex_config_text;

pub(crate) use codex_config_auth::load_api_key_for_launch;
use codex_config_auth::restore_or_remove_zenith_auth;
#[cfg(test)]
use codex_config_auth::{
    previous_codex_auth_should_be_saved, zenith_auth_is_owned, zenith_auth_key_if_configured,
};
#[cfg(test)]
use codex_config_backup::redact_config_secrets;
#[cfg(test)]
use codex_config_backup::{backup_config, prune_config_backups};
#[cfg(test)]
use codex_config_text::{
    backup_paths_from_directories, backup_paths_newest_first,
    remove_zenith_openai_base_url_override, upsert_zenith_provider,
};
use codex_config_text::{
    config_selects_zenith_provider, config_uses_zenith_provider, is_zenith_customer_key,
    latest_backup_model_provider, remove_zenith_provider, with_model_provider,
};
use std::{
    fs,
    path::Path,
    sync::{Mutex, MutexGuard},
};
#[cfg(test)]
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    files::atomic_write,
    key_storage::{delete_saved_app_key, load_saved_app_key, save_app_key},
    platform::default_codex_home,
};

const PROVIDER_ID: &str = "codex_local_access";
const LEGACY_PROVIDER_ID: &str = "zenith";
#[cfg(test)]
const PROVIDER_NAME: &str = "Zenith";
const BASE_URL: &str = "https://api.zenithmarket.dev/v1";
const CONFIG_FILE: &str = "config.toml";
const AUTH_FILE: &str = "auth.json";
const BACKUP_SUFFIX: &str = ".zenith.bak";
#[cfg(test)]
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
    crate::local_pool::profiles::codex::attach_ready_api(
        &default_codex_home(),
        profile_root(backup_dir)?,
        api_key,
    )
    .map_err(|error| error.message)
}

fn profile_root(backup_dir: &Path) -> Result<&Path, String> {
    if backup_dir
        .file_name()
        .is_some_and(|name| name == "client-config")
    {
        backup_dir
            .parent()
            .ok_or_else(|| "Profile recovery root is missing".to_string())
    } else {
        Ok(backup_dir)
    }
}

#[cfg(test)]
fn config_uses_local_pool_provider(content: &str) -> bool {
    content.lines().any(|line| {
        let line = line.trim();
        line.eq_ignore_ascii_case(&format!("model_provider = \"{LOCAL_POOL_PROVIDER_ID}\""))
            || line == format!("[model_providers.{LOCAL_POOL_PROVIDER_ID}]")
    })
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
    let root = profile_root(backup_dir)?;
    if crate::local_pool::profiles::codex::restore_ready_api(&default_codex_home(), root)
        .map_err(|error| error.message)?
    {
        if forget_key {
            delete_saved_app_key()?;
        }
        return Ok(());
    }
    // Read-only compatibility input for pre-unified API attachments. New
    // attachments never create rotating text backups or use this path.
    let _profile_guard = lock_codex_profile();
    let codex_home = default_codex_home();
    let config_path = codex_home.join(CONFIG_FILE);
    let auth_path = codex_home.join(AUTH_FILE);
    let original_config = read_optional_text(&config_path)?;
    let original_auth = read_optional_text(&auth_path)?;
    let original = original_config.as_deref().unwrap_or_default();
    if !config_selects_zenith_provider(original) {
        if forget_key {
            delete_saved_app_key()?;
        }
        return Ok(());
    }
    let previous_model_provider = latest_backup_model_provider(backup_dir);
    let mut next = remove_zenith_provider(original)?;

    let model_provider =
        previous_model_provider.unwrap_or_else(|| DEFAULT_MODEL_PROVIDER.to_string());
    next = with_model_provider(next, &model_provider)?;
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

        let next = upsert_zenith_provider(original).unwrap();
        let parsed: toml_edit::DocumentMut = next.parse().unwrap();

        assert!(next.contains(r#"model_provider = "codex_local_access""#));
        assert_eq!(
            parsed["model_providers"][PROVIDER_ID]["name"].as_str(),
            Some("Zenith")
        );
        assert_eq!(
            parsed["model_providers"][PROVIDER_ID]["base_url"].as_str(),
            Some("https://api.zenithmarket.dev/v1")
        );
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

        let next = remove_zenith_provider(original).unwrap();

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
        )
        .unwrap();

        assert!(next.starts_with(r#"model_provider = "openai""#));
        assert!(next.contains("[profiles.default]"));
    }

    #[test]
    fn remove_zenith_openai_base_url_override_keeps_other_openai_base_url() {
        let original = r#"
openai_base_url = "https://us.api.openai.com/v1"
model = "gpt-5.5"
"#;

        let next = remove_zenith_openai_base_url_override(original).unwrap();

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
