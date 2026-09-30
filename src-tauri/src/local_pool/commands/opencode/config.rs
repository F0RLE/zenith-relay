use crate::{
    files::atomic_write,
    local_pool::{
        error::{ErrorCode, LocalPoolError},
        state::DesktopState,
    },
};
use serde_json::{Map, Value};

mod order;
pub(super) use order::serialize_config;
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

pub(super) fn normalize_snapshot_name(value: &str) -> Result<String, LocalPoolError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_SNAPSHOT_NAME_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "OpenCode snapshot name is invalid",
        ));
    }
    Ok(value.to_string())
}

pub(super) fn backup_name(state: &DesktopState) -> Option<String> {
    fs::read_to_string(backup_name_path(state))
        .ok()
        .and_then(|value| normalize_snapshot_name(&value).ok())
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
    let value: Value = parse_jsonc(&content).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            format!("OpenCode config is not valid JSON: {error}"),
        )
    })?;
    value.as_object().cloned().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "OpenCode config root must be a JSON object",
        )
    })
}

pub(super) fn write_config(path: &Path, config: &Map<String, Value>) -> Result<(), LocalPoolError> {
    let parent = path.parent().ok_or_else(|| {
        LocalPoolError::new(ErrorCode::Io, "OpenCode config has no parent directory")
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
    let content = serialize_config(config)
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
                    .fold(false, |removed, (_, id, _)| {
                        providers.remove(*id).is_some() || removed
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

pub(super) fn current_config_is_managed(path: &Path) -> Result<bool, LocalPoolError> {
    if !path.exists() {
        return Ok(true);
    }
    let config = read_config(path)?;
    Ok(config
        .get("provider")
        .and_then(Value::as_object)
        .is_some_and(|providers| providers.keys().any(|id| super::protocols::managed_id(id))))
}

pub(super) fn restore_original_config_preserving_user_changes(
    original: &Path,
    current: &Path,
) -> Result<(), LocalPoolError> {
    let original_content = fs::read_to_string(original)
        .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
    let mut restored = parse_jsonc(&original_content)
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!("OpenCode snapshot is not valid JSON: {error}"),
            )
        })?
        .as_object()
        .cloned()
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "OpenCode snapshot root must be a JSON object",
            )
        })?;
    let current_config = read_config(current)?;
    for (key, value) in &current_config {
        if key != "provider" && key != "model" {
            restored.insert(key.clone(), value.clone());
        }
    }

    let mut providers = restored
        .get("provider")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(current_providers) = current_config.get("provider").and_then(Value::as_object) {
        for (id, provider) in current_providers {
            if !super::protocols::managed_id(id) {
                providers.insert(id.clone(), provider.clone());
            }
        }
    }
    if providers.is_empty() {
        restored.remove("provider");
    } else {
        restored.insert("provider".into(), Value::Object(providers));
    }

    let current_model_is_managed = current_config
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(super::protocols::managed_model);
    if !current_model_is_managed {
        match current_config.get("model") {
            Some(model) => {
                restored.insert("model".into(), model.clone());
            }
            None => {
                restored.remove("model");
            }
        }
    }
    write_config(current, &restored)
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
            for next in chars.by_ref() {
                if next == '\n' {
                    cleaned.push('\n');
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut previous = '\0';
            for next in chars.by_ref() {
                if previous == '*' && next == '/' {
                    break;
                }
                if next == '\n' {
                    cleaned.push('\n');
                }
                previous = next;
            }
            continue;
        }
        cleaned.push(ch);
    }
    let chars = cleaned.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(chars.len());
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in chars.iter().copied().enumerate() {
        if in_string {
            output.push(ch);
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
            output.push(ch);
            continue;
        }
        if ch == ',' {
            let mut next = index + 1;
            while next < chars.len() && chars[next].is_whitespace() {
                next += 1;
            }
            if next < chars.len() && (chars[next] == '}' || chars[next] == ']') {
                continue;
            }
        }
        output.push(ch);
    }
    serde_json::from_str(&output)
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
