pub(crate) mod automations;
pub(crate) mod connections;
pub(crate) mod gateway;
pub(crate) mod oauth;
pub(crate) mod pool;
pub(crate) mod profiles;
pub(crate) mod proxies;
pub(crate) mod recovery;
pub(crate) mod remote_server;
pub(crate) mod state;
pub(crate) mod usage;

mod runtime;

use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
    store::secret_store,
};

pub(super) fn cleanup_created_secret(secret_ref: &str, cause: &LocalPoolError) -> LocalResult<()> {
    secret_store::delete(secret_ref).map_err(|cleanup| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "{}; secret cleanup failed: {}",
                cause.message, cleanup.message
            ),
        )
    })
}

pub(in crate::local_pool) use runtime::{
    apply_account_policy_if_running, apply_local_gateway_key_scope,
    apply_source_policies_if_running, apply_source_policy_if_running, core_error, current_time_ms,
    fail_closed, refresh_active_codex_catalog_in_background,
    refresh_local_gateway_key_scope_if_running, restart_after_secret_change, restart_or_rollback,
    runtime_account_policy, runtime_from_store, sync_accounts_or_rollback,
    sync_gateway_or_rollback, sync_records_or_rollback, sync_refreshed_account_or_rollback,
};
