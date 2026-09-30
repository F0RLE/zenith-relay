use super::super::*;
use super::document::{
    bytes_hash, key_hash, parse_config, root_model_catalog_json, root_model_provider,
};

pub(in crate::local_pool::profiles::codex) fn external_model_catalog(
    document: &DocumentMut,
    backup: &ProfileBackup,
) -> Option<String> {
    let current = root_model_catalog_json(document);
    if current.as_deref().map(portable_path_value)
        == backup
            .managed_model_catalog_path
            .as_deref()
            .map(portable_path_value)
    {
        backup.previous_model_catalog_json.clone()
    } else {
        current
    }
}

pub(in crate::local_pool::profiles::codex) fn managed_config_matches(
    document: &DocumentMut,
    backup: &ProfileBackup,
) -> bool {
    root_model_provider(document).as_deref() == Some(backup.managed_provider_id.as_str())
        && (backup.managed_model_catalog_path.is_none()
            || root_model_catalog_json(document)
                .as_deref()
                .map(portable_path_value)
                == backup
                    .managed_model_catalog_path
                    .as_deref()
                    .map(portable_path_value))
        && managed_provider_matches(document, backup)
}

pub(in crate::local_pool::profiles::codex) fn previous_config_matches(
    document: &DocumentMut,
    backup: &ProfileBackup,
) -> bool {
    root_model_provider(document) == backup.previous_model_provider
        && root_model_catalog_json(document) == backup.previous_model_catalog_json
}

pub(in crate::local_pool::profiles::codex) fn model_catalog_to_restore(
    document: &DocumentMut,
    backup: &ProfileBackup,
) -> Option<String> {
    if backup.managed_model_catalog_path.is_some() {
        backup.previous_model_catalog_json.clone()
    } else {
        root_model_catalog_json(document)
    }
}

pub(in crate::local_pool::profiles::codex) fn external_provider_took_over(
    document: &DocumentMut,
    backup: &ProfileBackup,
) -> bool {
    root_model_provider(document).is_some_and(|provider| provider != backup.managed_provider_id)
        && managed_provider_matches(document, backup)
}

pub(in crate::local_pool::profiles::codex) fn external_account_provider_took_over(
    codex_home: &Path,
) -> Result<bool> {
    let config_path = canonical_profile_dir(codex_home)?.join(CONFIG_FILE);
    let config = read_optional_bytes(&config_path)?;
    let document = parse_config(snapshot_text(&config, &config_path)?.unwrap_or_default())?;
    Ok(root_model_provider(&document)
        .is_some_and(|provider| provider != "openai" && provider != PROVIDER_ID))
}

fn managed_provider_matches(document: &DocumentMut, backup: &ProfileBackup) -> bool {
    document
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(&backup.managed_provider_id))
        .and_then(Item::as_table)
        .is_some_and(|provider| {
            provider
                .get("name")
                .and_then(Item::as_str)
                .is_some_and(|name| {
                    managed_provider_name_matches(name, &backup.managed_provider_id)
                })
                && provider
                    .get("base_url")
                    .and_then(Item::as_str)
                    .is_some_and(|base_url| {
                        base_url.trim_end_matches('/') == backup.managed_base_url
                    })
                && provider
                    .get("wire_api")
                    .and_then(Item::as_str)
                    .is_some_and(|wire_api| wire_api == "responses")
                && provider.get("requires_openai_auth").and_then(Item::as_bool) == Some(true)
                && (!backup.managed_bearer_in_config
                    || provider
                        .get("experimental_bearer_token")
                        .and_then(Item::as_str)
                        .is_some_and(|token| key_hash(token.trim()) == backup.managed_key_hash))
                && backup.managed_supports_websockets.is_none_or(|expected| {
                    provider.get("supports_websockets").and_then(Item::as_bool) == Some(expected)
                })
        })
}

fn managed_provider_name_matches(name: &str, provider_id: &str) -> bool {
    if provider_id == READY_API_PROVIDER_ID {
        name == READY_API_PROVIDER_NAME || name == LEGACY_READY_API_PROVIDER_NAME
    } else {
        name == "Zenith Relay Local"
    }
}

pub(in crate::local_pool::profiles::codex) fn normalize_managed_provider_name(
    document: &mut DocumentMut,
    backup: &ProfileBackup,
) -> bool {
    if backup.managed_provider_id != READY_API_PROVIDER_ID {
        return false;
    }
    let Some(provider) = document
        .get_mut("model_providers")
        .and_then(Item::as_table_like_mut)
        .and_then(|providers| providers.get_mut(&backup.managed_provider_id))
        .and_then(Item::as_table_like_mut)
    else {
        return false;
    };
    if provider.get("name").and_then(Item::as_str) != Some(LEGACY_READY_API_PROVIDER_NAME) {
        return false;
    }
    provider.insert("name", value(READY_API_PROVIDER_NAME));
    true
}

pub(in crate::local_pool::profiles::codex) fn auth_content(local_key: &str) -> String {
    format!(
        "{{\n  \"OPENAI_API_KEY\": \"{}\",\n  \"auth_mode\": \"apikey\"\n}}\n",
        escape_json_string(local_key)
    )
}

fn auth_matches_snapshot(
    snapshot: &Option<Vec<u8>>,
    _path: &Path,
    expected_hash: &str,
) -> Result<bool> {
    let Some(value) = auth_snapshot_json(snapshot) else {
        return Ok(false);
    };
    Ok(value
        .get("OPENAI_API_KEY")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|key| key_hash(key.trim()) == expected_hash)
        && value
            .get("auth_mode")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|mode| mode == "apikey"))
}

pub(in crate::local_pool::profiles::codex) fn managed_auth_matches_snapshot(
    snapshot: &Option<Vec<u8>>,
    path: &Path,
    backup: &ProfileBackup,
) -> Result<bool> {
    match (
        backup.bound_oauth_account_id.as_deref(),
        backup.managed_oauth_access_hash.as_deref(),
    ) {
        (Some(account_id), Some(access_hash))
            if !account_id.trim().is_empty() && access_hash.len() == 64 =>
        {
            account_auth_matches_snapshot(snapshot, path, access_hash)
        }
        (Some(account_id), None) if !account_id.trim().is_empty() => {
            auth_matches_snapshot(snapshot, path, &backup.managed_key_hash)
        }
        (None, None) => auth_matches_snapshot(snapshot, path, &backup.managed_key_hash),
        _ => Ok(false),
    }
}

pub(in crate::local_pool::profiles::codex) fn previous_auth_matches_snapshot(
    snapshot: &Option<Vec<u8>>,
    backup: &ProfileBackup,
) -> bool {
    match backup.previous_auth_hash.as_deref() {
        Some(expected_hash) => snapshot
            .as_deref()
            .is_some_and(|content| bytes_hash(content) == expected_hash),
        None if backup.previous_auth_secret_ref.is_none() => snapshot.is_none(),
        None => false,
    }
}
