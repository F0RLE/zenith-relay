use rusqlite::Error as RusqliteError;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

pub(super) fn canonical_child(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let canonical = portable_canonicalize(path)?;
    let root = PathBuf::from(path_string(root));
    if !canonical.starts_with(&root) {
        return Err("repair path escaped its profile".to_string());
    }
    Ok(canonical)
}

pub(super) fn validate_target_provider(value: &str) -> Result<(), String> {
    if matches!(
        value,
        "openai" | "zenith_relay_local" | "codex_local_access"
    ) {
        Ok(())
    } else {
        Err("repair target provider is invalid".to_string())
    }
}

pub(super) fn sync_file(path: &Path) -> Result<(), String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(io_error)
}

pub(super) fn validate_id(value: &str, prefix: &str) -> Result<(), String> {
    if value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        Ok(())
    } else {
        Err("repair identifier is invalid".to_string())
    }
}

pub(super) fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(suffix);
    PathBuf::from(value)
}

pub(super) fn path_string(path: &Path) -> String {
    portable_path_value(&path.to_string_lossy())
}

pub(super) fn portable_canonicalize(path: &Path) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    Ok(PathBuf::from(path_string(&canonical)))
}

pub(super) use super::super::portable_path_value;

pub(super) fn db_error(error: RusqliteError) -> String {
    format!("ChatGPT history database operation failed: {error}")
}

pub(super) fn io_error(error: std::io::Error) -> String {
    format!("ChatGPT history repair I/O failed: {error}")
}
