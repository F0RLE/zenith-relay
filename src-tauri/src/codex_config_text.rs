use super::{BASE_URL, CONFIG_FILE, LEGACY_PROVIDER_ID, PROVIDER_ID, PROVIDER_NAME};
use crate::files::{escape_toml_string, unquote_toml_string};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) fn config_uses_zenith_provider(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.eq_ignore_ascii_case(&format!("model_provider = \"{PROVIDER_ID}\""))
            || trimmed.eq_ignore_ascii_case(&format!("model_provider = \"{LEGACY_PROVIDER_ID}\""))
            || trimmed == format!("[model_providers.{PROVIDER_ID}]")
            || trimmed == format!("[model_providers.{LEGACY_PROVIDER_ID}]")
    })
}

pub(super) fn config_selects_zenith_provider(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.eq_ignore_ascii_case(&format!("model_provider = \"{PROVIDER_ID}\""))
            || trimmed.eq_ignore_ascii_case(&format!("model_provider = \"{LEGACY_PROVIDER_ID}\""))
    })
}

pub(super) fn is_zenith_customer_key(key: &str) -> bool {
    key.starts_with("znt_")
}

pub(super) fn upsert_zenith_provider(original: &str) -> String {
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
    result.push_str("supports_websockets = true\n");
    result
}

pub(super) fn remove_zenith_provider(original: &str) -> String {
    let without_section = remove_table(original, &format!("[model_providers.{PROVIDER_ID}]"));
    let without_section = remove_table(
        &without_section,
        &format!("[model_providers.{LEGACY_PROVIDER_ID}]"),
    );
    let without_model_provider = remove_key_line(&without_section, "model_provider");
    remove_zenith_openai_base_url_override(&without_model_provider)
}

pub(super) fn remove_key_line(content: &str, key: &str) -> String {
    let prefix = format!("{key} =");
    content
        .lines()
        .filter(|line| !line.trim().starts_with(&prefix))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn with_model_provider(content: String, model_provider: &str) -> String {
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

pub(super) fn remove_zenith_openai_base_url_override(content: &str) -> String {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            let Some(value) = trimmed.strip_prefix("openai_base_url = ") else {
                return true;
            };
            unquote_toml_string(value.trim())
                .is_none_or(|url| url.trim_end_matches('/') != BASE_URL)
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    content.lines().find_map(|line| {
        let value = line.trim().strip_prefix("model_provider = ")?;
        let provider = unquote_toml_string(value.trim())?;
        (!provider.eq_ignore_ascii_case(PROVIDER_ID)
            && !provider.eq_ignore_ascii_case(LEGACY_PROVIDER_ID)
            && !provider.is_empty())
        .then_some(provider)
    })
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
