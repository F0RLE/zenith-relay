pub(in crate::local_pool) mod catalog;
pub(crate) mod claude_desktop;
pub mod codex;
pub(crate) mod opencode;
pub(crate) mod repair;
pub(crate) mod snapshots;

use super::error::{LocalPoolError, Result as LocalResult};
use std::future::Future;

pub(in crate::local_pool) use catalog::CodexCatalogRefreshStatus;

pub(in crate::local_pool) struct ClientCatalogRefreshResults {
    pub claude_desktop: LocalResult<()>,
    pub opencode: LocalResult<()>,
    pub codex: LocalResult<CodexCatalogRefreshStatus>,
}

pub(in crate::local_pool) async fn refresh_active_client_catalogs(
    state: &super::state::DesktopState,
) -> ClientCatalogRefreshResults {
    refresh_client_catalogs(
        claude_desktop::refresh_catalog(state),
        opencode::refresh_active_opencode_catalog(state),
        catalog::refresh_active_codex_catalog(state),
    )
    .await
}

async fn refresh_client_catalogs(
    claude_desktop: impl Future<Output = LocalResult<()>>,
    opencode: impl Future<Output = LocalResult<()>>,
    codex: impl Future<Output = LocalResult<CodexCatalogRefreshStatus>>,
) -> ClientCatalogRefreshResults {
    let claude_desktop = claude_desktop.await;
    let opencode = opencode.await;
    let codex = codex.await;
    ClientCatalogRefreshResults {
        claude_desktop,
        opencode,
        codex,
    }
}

#[cfg(test)]
#[path = "profiles/tests/catalog_refresh.rs"]
mod catalog_refresh_tests;

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
