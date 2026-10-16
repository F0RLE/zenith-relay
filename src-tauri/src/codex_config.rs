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
    zenith_relay_core::poison::mutex(&CODEX_PROFILE_LOCK)
}

pub fn enable_provider(api_key: &str, backup_dir: &Path) -> Result<(), String> {
    enable_provider_with_intent(api_key, backup_dir, false)
}

pub fn enable_provider_explicit(api_key: &str, backup_dir: &Path) -> Result<(), String> {
    enable_provider_with_intent(api_key, backup_dir, true)
}

fn enable_provider_with_intent(
    api_key: &str,
    backup_dir: &Path,
    rebase_newer_login: bool,
) -> Result<(), String> {
    if api_key.is_empty() {
        return Err("Введите API key.".to_string());
    }
    let profile_root = profile_root(backup_dir)?;
    let attach = if rebase_newer_login {
        crate::local_pool::profiles::codex::attach_ready_api_explicit
    } else {
        crate::local_pool::profiles::codex::attach_ready_api
    };
    attach(&default_codex_home(), profile_root, api_key).map_err(|error| error.message)
}

fn profile_root(backup_dir: &Path) -> Result<&Path, String> {
    if backup_dir
        .file_name()
        .is_some_and(|directory_name| directory_name == "client-config")
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
    original_content: Option<&str>,
) -> Result<(), String> {
    match original_content {
        Some(content) => replace_if_unchanged(path, expected_current, content),
        None => remove_if_unchanged(path, expected_current),
    }
}

fn rollback_ready_config(
    changed: bool,
    path: &Path,
    written_config: &str,
    original_content: Option<&str>,
) -> Result<(), String> {
    if changed {
        rollback_file(path, Some(written_config), original_content)
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
    let profile_root_path = profile_root(backup_dir)?;
    if crate::local_pool::profiles::codex::restore_ready_api(
        &default_codex_home(),
        profile_root_path,
    )
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
    let original_config_text = original_config.as_deref().unwrap_or_default();
    if !config_selects_zenith_provider(original_config_text) {
        if forget_key {
            delete_saved_app_key()?;
        }
        return Ok(());
    }
    let previous_model_provider = latest_backup_model_provider(backup_dir);
    let mut updated_config = remove_zenith_provider(original_config_text)?;

    let model_provider =
        previous_model_provider.unwrap_or_else(|| DEFAULT_MODEL_PROVIDER.to_string());
    updated_config = with_model_provider(updated_config, &model_provider)?;
    let saved_key = load_saved_app_key();
    if forget_key {
        delete_saved_app_key()?;
    }
    let reset_result = (|| {
        if updated_config != original_config_text {
            replace_if_unchanged(
                &config_path,
                original_config.as_deref(),
                updated_config.trim_start(),
            )?;
        }
        if let Err(error) = restore_or_remove_zenith_auth(
            original_config_text,
            original_auth.as_deref(),
            saved_key.as_deref(),
        ) {
            return Err(with_cleanup(
                error,
                rollback_ready_config(
                    updated_config != original_config_text,
                    &config_path,
                    updated_config.trim_start(),
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
    let config_text = fs::read_to_string(config_path).unwrap_or_default();
    config_selects_zenith_provider(&config_text)
        && config_text.contains(&format!("[model_providers.{PROVIDER_ID}]"))
        && load_api_key_for_launch().is_some()
}

#[cfg(test)]
mod tests;
