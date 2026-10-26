use crate::{
    codex_config::load_api_key_for_launch,
    launcher::{is_codex_running, launch_codex_with_profile},
    local_pool::{
        accounts::{
            NativeSecretBackend,
            credentials::CredentialStore,
            quota_refresh::{
                PreparedAccountCredentials, prepare_account_credentials,
                sync_managed_account_profile,
            },
            records::{candidate_health, candidate_quota_with_stale_after},
        },
        error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
        models::LocalPoolSnapshot,
        profiles::{catalog, codex, snapshots},
        state::DesktopState,
        store::secret_store,
    },
    platform::default_codex_home,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::State;
use zenith_relay_core::QUOTA_STALE_AFTER_MS;

pub(crate) mod actions;
pub(crate) mod gateway;
mod history;
mod policy;
mod process;

use catalog::{
    fetch_local_codex_model_catalog, load_direct_source_api_key, validate_direct_source,
};

pub(crate) use actions::restore_managed_profiles_before_reset;
use actions::set_runtime_pool_interface_reserve;
use history::{CodexHistoryProvider, rollback_history_on_error, synchronize_history_for_command};
use policy::{
    GatewayOAuthBindingRequest, gateway_oauth_binding_request, prioritize_account_candidates,
    profile_quota_rank,
};
use process::{
    restart_codex_after_failed_change, restart_codex_after_restore, stop_codex_and_sync_account,
    stop_codex_and_sync_account_at, stop_codex_for_profile_change,
};

fn with_chatgpt_context<T>(
    state: &DesktopState,
    profile_dir: &Path,
    operation: impl FnOnce() -> Result<T, CommandError>,
) -> Result<T, CommandError> {
    let change = codex::sync_context_policy(profile_dir, &state.profile_backup_root())?;
    operation().map_err(|mut error| {
        if let Err(rollback) = change.rollback() {
            error.message = format!(
                "{}; automatic ChatGPT context rollback failed: {}",
                error.message, rollback.message
            );
        }
        error
    })
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileActivation {
    binding: codex::ProfileBinding,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateChatgptQuotaReserveInput {
    reserve_basis_points: u64,
}

#[cfg(test)]
mod tests;
