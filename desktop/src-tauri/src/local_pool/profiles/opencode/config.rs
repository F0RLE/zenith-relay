use crate::{
    files::atomic_write,
    local_pool::{
        error::{ErrorCode, LocalPoolError},
        state::DesktopState,
    },
};
use serde_json::{Map, Value};

mod order;
mod recovery;
#[cfg(test)]
pub(super) use order::serialize_config;
pub(super) use order::serialize_config_with_model_order;
pub(super) use recovery::restore_saved_config;
use std::{
    fs,
    path::{Path, PathBuf},
};

const MAX_SNAPSHOT_NAME_CHARS: usize = 80;

pub(super) fn backup_root(state: &DesktopState) -> PathBuf {
    state.opencode_backup_root()
}

pub(super) fn backup_path(state: &DesktopState) -> PathBuf {
    backup_root(state).join("original-opencode.json")
}

pub(super) fn missing_marker_path(state: &DesktopState) -> PathBuf {
    backup_root(state).join("original-opencode.missing")
}

pub(super) fn backup_name_path(state: &DesktopState) -> PathBuf {
    backup_root(state).join("original-opencode.name")
}

pub(super) fn normalize_snapshot_name(snapshot_name: &str) -> Result<String, LocalPoolError> {
    let normalized_name = snapshot_name.trim();
    if normalized_name.is_empty()
        || normalized_name.chars().count() > MAX_SNAPSHOT_NAME_CHARS
        || normalized_name.chars().any(char::is_control)
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "OpenCode snapshot name is invalid",
        ));
    }
    Ok(normalized_name.to_string())
}

pub(super) fn backup_name(state: &DesktopState) -> Option<String> {
    fs::read_to_string(backup_name_path(state))
        .ok()
        .and_then(|stored_name| normalize_snapshot_name(&stored_name).ok())
}

pub(super) fn backup_created_at_ms(state: &DesktopState) -> Option<u64> {
    let path = if backup_path(state).exists() {
        backup_path(state)
    } else if missing_marker_path(state).exists() {
        missing_marker_path(state)
    } else {
        return None;
    };
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
}

pub(super) fn read_config(path: &Path) -> Result<Map<String, Value>, LocalPoolError> {
    let content = match fs::read_to_string(path) {
        Ok(content) if !content.trim().is_empty() => content,
        Ok(_) => return Ok(Map::new()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(LocalPoolError::new(ErrorCode::Io, error.to_string())),
    };
    let parsed_config: Value = parse_jsonc(&content).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            format!("OpenCode config is not valid JSON: {error}"),
        )
    })?;
    parsed_config.as_object().cloned().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "OpenCode config root must be a JSON object",
        )
    })
}

pub(super) fn write_config_with_model_order(
    path: &Path,
    config: &Map<String, Value>,
    models: &[String],
) -> Result<(), LocalPoolError> {
    let parent = path.parent().ok_or_else(|| {
        LocalPoolError::new(ErrorCode::Io, "OpenCode config has no parent directory")
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
    let content = serialize_config_with_model_order(config, models)
        .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
    atomic_write(path, &format!("{content}\n"))
        .map_err(|error| LocalPoolError::new(ErrorCode::Io, error))
}

pub(super) fn remove_managed_configuration(config: &mut Map<String, Value>) -> bool {
    let (provider_removed, providers_empty) = config
        .get_mut("provider")
        .and_then(Value::as_object_mut)
        .map_or((false, false), |providers| {
            (
                super::protocols::GROUPS
                    .iter()
                    .fold(false, |removed, (_, provider_id, _)| {
                        providers.remove(*provider_id).is_some() || removed
                    }),
                providers.is_empty(),
            )
        });
    if providers_empty {
        config.remove("provider");
    }
    let model_removed = if config
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(super::protocols::managed_model)
    {
        config.remove("model");
        true
    } else {
        false
    };
    provider_removed || model_removed
}

/// OpenCode accepts JSONC. Strip comments and trailing commas without
/// touching characters inside JSON strings before handing the value to
/// serde_json. The original bytes remain recoverable through the backup.
pub(super) fn parse_jsonc(content: &str) -> Result<Value, serde_json::Error> {
    let mut cleaned = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            cleaned.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            cleaned.push(ch);
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            cleaned.push(' ');
            for line_char in chars.by_ref() {
                if line_char == '\n' {
                    cleaned.push('\n');
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            cleaned.push(' ');
            let mut previous_char = '\0';
            let mut closed = false;
            for block_char in chars.by_ref() {
                if previous_char == '*' && block_char == '/' {
                    closed = true;
                    break;
                }
                if block_char == '\n' {
                    cleaned.push('\n');
                }
                previous_char = block_char;
            }
            if !closed {
                return Err(<serde_json::Error as serde::de::Error>::custom(
                    "Unterminated JSONC block comment",
                ));
            }
            continue;
        }
        cleaned.push(ch);
    }
    let chars = cleaned.chars().collect::<Vec<_>>();
    let mut normalized_json = String::with_capacity(chars.len());
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in chars.iter().copied().enumerate() {
        if in_string {
            normalized_json.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            normalized_json.push(ch);
            continue;
        }
        if ch == ',' {
            let mut lookahead_index = index + 1;
            while lookahead_index < chars.len() && chars[lookahead_index].is_whitespace() {
                lookahead_index += 1;
            }
            let follows_value = normalized_json
                .chars()
                .rev()
                .find(|ch| !ch.is_whitespace())
                .is_some_and(|ch| !matches!(ch, '[' | '{' | ':' | ','));
            if follows_value
                && lookahead_index < chars.len()
                && (chars[lookahead_index] == '}' || chars[lookahead_index] == ']')
            {
                continue;
            }
        }
        normalized_json.push(ch);
    }
    serde_json::from_str(&normalized_json)
}

pub(super) fn backup_original_config(
    state: &DesktopState,
    path: &Path,
    snapshot_name: Option<&str>,
) -> Result<bool, LocalPoolError> {
    let backup = backup_path(state);
    let missing = missing_marker_path(state);
    if backup.exists() || missing.exists() {
        return Ok(false);
    }
    fs::create_dir_all(backup_root(state))
        .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
    if let Some(snapshot_name) = snapshot_name {
        atomic_write(&backup_name_path(state), snapshot_name)
            .map_err(|error| LocalPoolError::new(ErrorCode::Io, error))?;
    }
    if path.exists() {
        fs::copy(path, backup)
            .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
    } else {
        fs::write(missing, b"original config did not exist\n")
            .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
    }
    Ok(true)
}
