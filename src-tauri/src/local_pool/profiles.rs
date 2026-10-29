pub(in crate::local_pool) mod catalog;
pub(crate) mod claude_desktop;
pub mod codex;
pub(crate) mod opencode;
pub(crate) mod repair;
pub(crate) mod snapshots;

use super::error::LocalPoolError;

pub(in crate::local_pool) use catalog::CodexCatalogRefreshStatus;

pub(in crate::local_pool) async fn refresh_active_client_catalogs(
    state: &super::state::DesktopState,
) -> super::error::Result<CodexCatalogRefreshStatus> {
    let claude = claude_desktop::refresh_catalog(state).await;
    let other_clients = catalog::refresh_client_catalogs(
        opencode::refresh_active_opencode_catalog(state),
        catalog::refresh_active_codex_catalog(state),
    )
    .await;
    claude?;
    other_clients
}

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
