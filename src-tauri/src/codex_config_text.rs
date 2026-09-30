#[cfg(test)]
use super::PROVIDER_NAME;
use super::{BASE_URL, CONFIG_FILE, LEGACY_PROVIDER_ID, LOCAL_POOL_PROVIDER_ID, PROVIDER_ID};
use std::{
    fs,
    path::{Path, PathBuf},
};
use toml_edit::{value, DocumentMut, Item};

pub(super) fn config_uses_zenith_provider(content: &str) -> bool {
    config_selects_zenith_provider(content)
}

pub(super) fn config_selects_zenith_provider(content: &str) -> bool {
    parse_document(content)
        .ok()
        .and_then(|doc| {
            doc.get("model_provider")
                .and_then(Item::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|id| id == PROVIDER_ID || id == LEGACY_PROVIDER_ID)
}

fn parse_document(content: &str) -> Result<DocumentMut, String> {
    content
        .parse()
        .map_err(|_| "Codex config is not valid TOML; no changes were applied".to_string())
}

pub(super) fn is_zenith_customer_key(key: &str) -> bool {
    key.starts_with("znt_")
}

#[cfg(test)]
pub(super) fn upsert_zenith_provider(original: &str) -> Result<String, String> {
    let mut doc = parse_document(&remove_zenith_provider(original)?)?;
    doc["model_provider"] = value(PROVIDER_ID);
    doc["model_providers"][PROVIDER_ID]["name"] = value(PROVIDER_NAME);
    doc["model_providers"][PROVIDER_ID]["base_url"] = value(BASE_URL);
    doc["model_providers"][PROVIDER_ID]["wire_api"] = value("responses");
    doc["model_providers"][PROVIDER_ID]["requires_openai_auth"] = value(true);
    doc["model_providers"][PROVIDER_ID]["supports_websockets"] = value(true);
    Ok(doc.to_string())
}

pub(super) fn remove_zenith_provider(original: &str) -> Result<String, String> {
    let mut doc = parse_document(original)?;
    if let Some(providers) = doc
        .get_mut("model_providers")
        .and_then(Item::as_table_like_mut)
    {
        providers.remove(PROVIDER_ID);
        providers.remove(LEGACY_PROVIDER_ID);
    }
    doc.remove("model_provider");
    remove_zenith_openai_base_url_override(&doc.to_string())
}

pub(super) fn with_model_provider(content: String, model_provider: &str) -> Result<String, String> {
    let mut doc = parse_document(&content)?;
    doc["model_provider"] = value(model_provider);
    Ok(doc.to_string())
}

pub(super) fn remove_zenith_openai_base_url_override(content: &str) -> Result<String, String> {
    let mut doc = parse_document(content)?;
    if doc
        .get("openai_base_url")
        .and_then(Item::as_str)
        .is_some_and(|url| url.trim_end_matches('/') == BASE_URL)
    {
        doc.remove("openai_base_url");
    }
    Ok(doc.to_string())
}

pub(super) fn latest_backup_model_provider(backup_dir: &Path) -> Option<String> {
    backup_paths_newest_first(backup_dir)
        .into_iter()
        .find_map(|path| {
            let content = fs::read_to_string(path).ok()?;
            read_model_provider(&content)
        })
}

pub(super) fn backup_paths_newest_first(backup_dir: &Path) -> Vec<PathBuf> {
    backup_paths_from_directories([backup_dir.to_path_buf()])
}

pub(super) fn backup_paths_from_directories(
    directories: impl IntoIterator<Item = PathBuf>,
) -> Vec<PathBuf> {
    let mut backups = directories
        .into_iter()
        .flat_map(|directory| {
            fs::read_dir(directory)
                .ok()
                .into_iter()
                .flat_map(|entries| entries.filter_map(Result::ok))
        })
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_file() || file_type.is_symlink() {
                return None;
            }
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
    let doc = parse_document(content).ok()?;
    let provider = doc.get("model_provider")?.as_str()?;
    (![PROVIDER_ID, LEGACY_PROVIDER_ID, LOCAL_POOL_PROVIDER_ID].contains(&provider)
        && !provider.is_empty())
    .then(|| provider.to_owned())
}

fn is_zenith_backup_name(name: &str) -> bool {
    name.starts_with(&format!("{CONFIG_FILE}."))
        && name.ends_with(super::BACKUP_SUFFIX)
        && name.len() > CONFIG_FILE.len() + super::BACKUP_SUFFIX.len() + 1
}

fn backup_timestamp_from_name(name: &str) -> u64 {
    name.trim_start_matches(&format!("{CONFIG_FILE}."))
        .trim_end_matches(super::BACKUP_SUFFIX)
        .split('-')
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
}
