use super::{backup_paths_from_directories, BACKUP_SUFFIX, CONFIG_FILE, MAX_CONFIG_BACKUPS};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) fn backup_config(backup_dir: &Path, content: &str) -> Result<(), String> {
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

pub(super) fn prune_config_backups(backup_dir: &Path) -> Result<(), String> {
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

pub(super) fn redact_config_secrets(content: &str) -> String {
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
