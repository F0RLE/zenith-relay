use super::super::{
    DesktopState, ErrorCode, LocalPoolError, Path, Value, backup_name_path, backup_path,
    missing_marker_path, parse_jsonc, remove_managed_configuration,
    serialize_config_with_model_order,
};
use crate::{files::atomic_write_bytes, local_pool::profiles::opencode::protocols};
use serde_json::Map;
use std::{fs, path::PathBuf};

type Config = Map<String, Value>;

struct CapturedFile {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
}

impl CapturedFile {
    fn read(path: PathBuf) -> Result<Self, LocalPoolError> {
        let bytes = read_optional(&path)?;
        Ok(Self { path, bytes })
    }

    fn check(&self) -> Result<(), LocalPoolError> {
        check_bytes(&self.path, &self.bytes)
    }
}

pub(super) struct PreparedRestore {
    config: CapturedFile,
    restored: Option<Vec<u8>>,
    recovery: [CapturedFile; 3],
    record_index: usize,
}

impl PreparedRestore {
    pub(super) fn apply(self) -> Result<(), LocalPoolError> {
        for file in &self.recovery {
            file.check()?;
        }
        self.config.check()?;
        if self.config.bytes != self.restored {
            match &self.restored {
                Some(bytes) => atomic_write_bytes(&self.config.path, bytes)
                    .map_err(|_| io_error("OpenCode configuration could not be replaced"))?,
                None => fs::remove_file(&self.config.path)
                    .map_err(|_| io_error("OpenCode configuration could not be removed"))?,
            }
        }
        check_bytes(&self.config.path, &self.restored)?;
        for file in &self.recovery {
            file.check()?;
        }
        fs::remove_file(&self.recovery[self.record_index].path)
            .map_err(|_| io_error("OpenCode recovery record could not be consumed"))?;
        if self.recovery[2].bytes.is_some() {
            let _ = fs::remove_file(&self.recovery[2].path);
        }
        Ok(())
    }
}

pub(super) fn prepare(
    state: &DesktopState,
    path: &Path,
) -> Result<Option<PreparedRestore>, LocalPoolError> {
    let backup = CapturedFile::read(backup_path(state))?;
    let missing = CapturedFile::read(missing_marker_path(state))?;
    let (record_index, original) = match (&backup.bytes, &missing.bytes) {
        (Some(bytes), None) => (
            0,
            Some(parse_config(bytes, ErrorCode::RecoveryRequired, false)?),
        ),
        (None, Some(bytes)) if bytes == b"original config did not exist\n" => (1, None),
        (None, None) => return Ok(None),
        _ => {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "OpenCode recovery records are inconsistent",
            ));
        }
    };
    let name = CapturedFile::read(backup_name_path(state))?;
    let config = CapturedFile::read(path.to_path_buf())?;
    let current = config
        .bytes
        .as_deref()
        .map(|bytes| parse_config(bytes, ErrorCode::InvalidState, true))
        .transpose()?
        .unwrap_or_default();
    let original_is_missing = original.is_none();
    let (restored_config, remove_file) = match original {
        Some(original) if config.bytes.is_none() => (original, false),
        Some(original) => (merge_connection(original, &current), false),
        None => {
            let mut restored = current.clone();
            let removed = remove_managed_configuration(&mut restored);
            let empty = restored.is_empty();
            (restored, removed && empty)
        }
    };
    let restored = if remove_file {
        None
    } else if restored_config == current && (config.bytes.is_some() || original_is_missing) {
        config.bytes.clone()
    } else {
        let content = serialize_config_with_model_order(&restored_config, &[])
            .map_err(|_| io_error("OpenCode configuration could not be serialized"))?;
        Some(format!("{content}\n").into_bytes())
    };
    Ok(Some(PreparedRestore {
        config,
        restored,
        recovery: [backup, missing, name],
        record_index,
    }))
}

fn parse_config(
    bytes: &[u8],
    code: ErrorCode,
    allow_empty: bool,
) -> Result<Config, LocalPoolError> {
    let content = std::str::from_utf8(bytes)
        .map_err(|_| LocalPoolError::new(code, "OpenCode configuration is not valid UTF-8"))?;
    if allow_empty && content.trim().is_empty() {
        return Ok(Map::new());
    }
    let value = parse_jsonc(content)
        .map_err(|_| LocalPoolError::new(code, "OpenCode configuration is not valid JSONC"))?;
    let config = value.as_object().cloned().ok_or_else(|| {
        LocalPoolError::new(code, "OpenCode configuration root must be an object")
    })?;
    if config
        .get("provider")
        .is_some_and(|provider| !provider.is_object())
    {
        return Err(LocalPoolError::new(
            code,
            "OpenCode provider settings must be an object for recovery",
        ));
    }
    Ok(config)
}

fn merge_connection(original: Config, current: &Config) -> Config {
    let mut restored = current.clone();
    let mut providers = original
        .get("provider")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(current_providers) = current.get("provider").and_then(Value::as_object) {
        for (id, provider) in current_providers {
            if !protocols::managed_id(id) {
                providers.insert(id.clone(), provider.clone());
            }
        }
    }
    if providers.is_empty() {
        restored.remove("provider");
    } else {
        restored.insert("provider".into(), Value::Object(providers));
    }
    if current
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(protocols::managed_model)
    {
        match original.get("model") {
            Some(model) => {
                restored.insert("model".into(), model.clone());
            }
            None => {
                restored.remove("model");
            }
        }
    }
    restored
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, LocalPoolError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(io_error(
            "OpenCode configuration or recovery data could not be read",
        )),
    }
}

fn check_bytes(path: &Path, expected: &Option<Vec<u8>>) -> Result<(), LocalPoolError> {
    if read_optional(path)?.as_deref() != expected.as_deref() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "OpenCode configuration or recovery data changed during restoration; retry",
        ));
    }
    Ok(())
}

fn io_error(message: &str) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, message)
}
