use crate::{
    codex_config::load_api_key_for_launch,
    launcher::{is_codex_running, launch_codex_with_profile},
    local_pool::{
        accounts::{
            credentials::CredentialStore,
            quota_refresh::{
                prepare_account_credentials, sync_managed_account_profile,
                PreparedAccountCredentials,
            },
            records::{candidate_health, candidate_quota_with_stale_after},
            NativeSecretBackend,
        },
        error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
        models::LocalPoolSnapshot,
        profiles::{codex, snapshots},
        remote::client::RemoteProfileCredential,
        state::DesktopState,
        store::secret_store,
    },
    platform::default_codex_home,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::State;
use zenith_relay_core::{
    protocol::{Feature, ProfileKeyRotation},
    DefaultServiceTier, QUOTA_STALE_AFTER_MS,
};

pub(crate) mod actions;
mod catalog;
pub(crate) mod gateway;
mod history;
mod policy;
mod process;

pub(in crate::local_pool) use catalog::CodexCatalogRefreshStatus;
use catalog::{fetch_codex_model_catalog, load_direct_source_api_key, validate_direct_source};

pub(in crate::local_pool) async fn refresh_active_client_catalogs(
    state: &DesktopState,
) -> LocalResult<CodexCatalogRefreshStatus> {
    catalog::refresh_client_catalogs(
        super::opencode::refresh_active_opencode_catalog(state),
        catalog::refresh_active_codex_catalog(state),
    )
    .await
}
pub(in crate::local_pool::commands) use actions::verify_remote_profile_binding;
use actions::{
    append_profile_rollback_error, append_remote_cleanup_error, profile_rotation_commit_state,
    set_runtime_pool_interface_reserve,
};
pub(crate) use actions::{prepare_ready_api_profile, restore_managed_profiles_before_reset};
pub(crate) use history::{
    discard_codex_history_backup, history_provider_changed, synchronize_codex_history,
    CodexHistoryProvider,
};
use history::{rollback_history_on_error, synchronize_history_for_command};
use policy::{
    gateway_oauth_binding_request, prioritize_account_candidates, profile_quota_rank,
    GatewayOAuthBindingRequest,
};
use process::{
    restart_codex_after_failed_change, restart_codex_after_restore, stop_codex_and_sync_account,
    stop_codex_and_sync_account_at, stop_codex_for_profile_change,
};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProfileRotationCommitState {
    Committed,
    NotCommitted,
    Unknown,
}
#[cfg(test)]
mod tests;
