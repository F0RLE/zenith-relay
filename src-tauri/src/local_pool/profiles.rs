pub mod codex;
pub(crate) mod repair;
pub(crate) mod snapshots;

use super::error::LocalPoolError;

pub(super) fn io_error_message(error: String) -> LocalPoolError {
    LocalPoolError::io(error)
}

/// Drop Win32 extended-path prefixes so Codex config and backups stay readable.
pub(super) fn portable_path_value(path_text: &str) -> String {
    if let Some(rest) = path_text.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else {
        path_text
            .strip_prefix("\\\\?\\")
            .unwrap_or(path_text)
            .to_owned()
    }
}
