pub mod codex;
pub(crate) mod repair;
pub(crate) mod snapshots;

/// Drop Win32 extended-path prefixes so Codex config and backups stay readable.
pub(super) fn portable_path_value(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else {
        value.strip_prefix("\\\\?\\").unwrap_or(value).to_owned()
    }
}
