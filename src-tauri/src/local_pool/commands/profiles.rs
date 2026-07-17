use crate::{
    launcher::{launch_codex_with_profile, stop_codex_and_wait},
    local_pool::{
        accounts::records::{candidate_quota_with_stale_after, quota_stale_after_ms_for_interval},
        commands::accounts::{
            prepare_account_credentials, sync_managed_account_profile, PreparedAccountCredentials,
        },
        error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
        models::LocalGatewayKeyRecord,
        profiles::{codex, repair, snapshots},
        state::DesktopState,
        store::secret_store,
    },
    platform::default_codex_home,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::State;
use zenith_relay_core::CandidateQuota;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileActivation {
    binding: codex::ProfileBinding,
    previous_credential_kind: Option<codex::ProfileCredentialKind>,
    repair_recommended: bool,
    stopped_running_client: bool,
}

#[tauri::command]
pub async fn attach_codex_to_local_gateway(
    key_id: String,
    bound_oauth_account_id: Option<String>,
    disable_oauth_binding: Option<bool>,
    state: State<'_, DesktopState>,
) -> Result<ProfileActivation, CommandError> {
    let _mutation = state.setup_guard().await;
    let (key, port) = {
        let store = state.store()?;
        let key = store
            .key(&key_id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local key not found"))?;
        (key, store.gateway().port)
    };
    if !key.enabled || !super::pool::has_usable_source(&state, &key)? {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "local key is not available for any enabled candidate",
        )
        .into());
    }
    let secret = super::pool::ensure_local_gateway_key_secret(&key)?;
    let profile_dir = default_codex_home();
    let previous = codex::credential_kind(&profile_dir, &state.profile_backup_root())?;
    let stopped = stop_codex_and_sync_account(&state).await?;
    let result: Result<ProfileActivation, CommandError> = async {
        let binding_request = gateway_oauth_binding_request(
            disable_oauth_binding.unwrap_or(false),
            bound_oauth_account_id.as_deref(),
        )?;
        let bound_oauth =
            resolve_gateway_oauth_binding(&state, &key, binding_request, &profile_dir).await?;
        let base_url = format!("http://127.0.0.1:{port}/v1");
        let binding = match bound_oauth.as_ref() {
            Some((account_id, prepared)) => codex::attach_with_oauth(
                &profile_dir,
                &state.profile_backup_root(),
                &key_id,
                &base_url,
                &secret,
                codex::BoundOAuthProfile {
                    account_id,
                    tokens: prepared.tokens(),
                    provider_account_id: prepared.provider_account_id(),
                },
            )?,
            None => codex::attach(
                &profile_dir,
                &state.profile_backup_root(),
                &key_id,
                &base_url,
                &secret,
            )?,
        };
        set_runtime_pool_interface_reserve(&state, binding.bound_oauth_account_id.as_deref()).await;
        Ok(profile_activation(binding, previous, stopped))
    }
    .await;
    restart_codex_after_failed_change(stopped, result, launch_codex_with_profile)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GatewayOAuthBindingRequest<'a> {
    Disabled,
    Automatic,
    Account(&'a str),
}

fn gateway_oauth_binding_request(
    disabled: bool,
    requested_account_id: Option<&str>,
) -> LocalResult<GatewayOAuthBindingRequest<'_>> {
    let requested_account_id = requested_account_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if disabled && requested_account_id.is_some() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "OAuth binding cannot be disabled and assigned to an account together",
        ));
    }
    Ok(if disabled {
        GatewayOAuthBindingRequest::Disabled
    } else if let Some(account_id) = requested_account_id {
        GatewayOAuthBindingRequest::Account(account_id)
    } else {
        GatewayOAuthBindingRequest::Automatic
    })
}

async fn resolve_gateway_oauth_binding(
    state: &DesktopState,
    key: &LocalGatewayKeyRecord,
    request: GatewayOAuthBindingRequest<'_>,
    profile_dir: &std::path::Path,
) -> LocalResult<Option<(String, PreparedAccountCredentials)>> {
    if request == GatewayOAuthBindingRequest::Disabled {
        return Ok(None);
    }
    let requested_account_id = match request {
        GatewayOAuthBindingRequest::Account(account_id) => Some(account_id),
        GatewayOAuthBindingRequest::Disabled | GatewayOAuthBindingRequest::Automatic => None,
    };
    let preferred_account_id = match requested_account_id {
        Some(account_id) => Some(account_id.to_string()),
        None => codex::active_managed_account_id(profile_dir, &state.profile_backup_root())?,
    };
    let automatic = requested_account_id.is_none();
    let mut candidates = {
        let store = state.store()?;
        let gateway = store.gateway();
        let mut candidates = Vec::new();
        for account in store.accounts() {
            let scoped = key
                .account_ids
                .as_ref()
                .is_none_or(|ids| ids.iter().any(|id| id == &account.account.id));
            if !scoped
                || !account.account.enabled
                || !account.account.in_pool
                || account.account.draining
                || account.account.auth_state
                    != zenith_relay_core::accounts::AccountAuthState::Active
                || !matches!(
                    account.account.auth_mode,
                    zenith_relay_core::accounts::AccountAuthMode::OAuth
                        | zenith_relay_core::accounts::AccountAuthMode::ImportedToken
                )
                || !super::account_routing_allowed(gateway, &account.account.subscription)
                || account.account.secret_refs.is_empty()
            {
                continue;
            }
            let mut secrets_available = true;
            for secret_ref in &account.account.secret_refs {
                if secret_store::load(secret_ref)?.is_none() {
                    secrets_available = false;
                    break;
                }
            }
            if secrets_available {
                let remaining = match candidate_quota_with_stale_after(
                    &account.account.quota,
                    super::current_time_ms(),
                    quota_stale_after_ms_for_interval(gateway.quota_refresh_interval_seconds),
                ) {
                    CandidateQuota::Available(remaining) => remaining,
                    CandidateQuota::Unknown | CandidateQuota::Exhausted | CandidateQuota::Stale => {
                        0
                    }
                };
                candidates.push((account.account.id.clone(), remaining));
            }
        }
        candidates
    };
    prioritize_account_candidates(&mut candidates, preferred_account_id.as_deref(), automatic);
    if requested_account_id.is_some()
        && candidates
            .first()
            .is_none_or(|(candidate, _)| Some(candidate.as_str()) != requested_account_id)
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "selected OAuth account is not available to this local pool key",
        ));
    }

    let mut last_error = None;
    for (account_id, _) in candidates {
        match prepare_account_credentials(state, &account_id).await {
            Ok(prepared)
                if prepared.tokens().refresh_token().is_some()
                    && prepared.tokens().id_token().is_some() =>
            {
                return Ok(Some((account_id, prepared)));
            }
            Ok(_) => {
                last_error = Some(LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "OAuth binding requires refresh, identity, and account tokens",
                ));
            }
            Err(error) => last_error = Some(error),
        }
        if requested_account_id.is_some() {
            break;
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

fn prioritize_account_candidates(
    candidates: &mut Vec<(String, u64)>,
    preferred: Option<&str>,
    automatic: bool,
) {
    if automatic {
        candidates.retain(|(_, remaining)| {
            *remaining > super::CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS
        });
    }
    candidates.sort_by(|left, right| {
        let left_preferred = Some(left.0.as_str()) == preferred;
        let right_preferred = Some(right.0.as_str()) == preferred;
        right_preferred
            .cmp(&left_preferred)
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| left.0.cmp(&right.0))
    });
    candidates.dedup_by(|left, right| left.0 == right.0);
}

#[tauri::command]
pub async fn restore_codex_profile(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let stopped = stop_codex_and_sync_account(&state).await?;
    let result =
        codex::restore(&default_codex_home(), &state.profile_backup_root()).map_err(Into::into);
    let result = restart_codex_after_failed_change(stopped, result, launch_codex_with_profile);
    if result.is_ok() {
        set_runtime_pool_interface_reserve(&state, None).await;
    }
    result
}

pub(crate) async fn prepare_ready_api_profile(state: &DesktopState) -> Result<bool, CommandError> {
    let _mutation = state.setup_guard().await;
    let profile_dir = default_codex_home();
    if codex::credential_kind(&profile_dir, &state.profile_backup_root())?
        != Some(codex::ProfileCredentialKind::LocalGateway)
    {
        return Ok(false);
    }
    let stopped = stop_codex_and_sync_account(state).await?;
    let result = codex::restore(&profile_dir, &state.profile_backup_root()).map_err(Into::into);
    let result = restart_codex_after_failed_change(stopped, result, launch_codex_with_profile);
    if result.is_ok() {
        set_runtime_pool_interface_reserve(state, None).await;
    }
    result.map(|()| stopped)
}

#[tauri::command]
pub async fn stop_managed_codex_profile(
    state: State<'_, DesktopState>,
) -> Result<bool, CommandError> {
    let _mutation = state.setup_guard().await;
    stop_codex_and_sync_account(&state).await
}

#[tauri::command]
pub async fn launch_managed_codex_profile(
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    launch_codex_with_profile().map_err(|error| {
        LocalPoolError::new(ErrorCode::Io, format!("failed to launch ChatGPT: {error}")).into()
    })
}

#[tauri::command]
pub async fn attach_codex_to_account(
    account_id: String,
    profile_dir: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<ProfileActivation, CommandError> {
    let _mutation = state.setup_guard().await;
    activate_account_profile(&account_id, profile_dir, &state).await
}

#[tauri::command]
pub async fn launch_codex_account(
    account_id: String,
    state: State<'_, DesktopState>,
) -> Result<ProfileActivation, CommandError> {
    let _mutation = state.setup_guard().await;
    activate_account_profile(&account_id, None, &state).await
}

#[tauri::command]
pub fn list_codex_account_bindings(
    state: State<'_, DesktopState>,
) -> Result<Vec<codex::ProfileBinding>, CommandError> {
    codex::profile_bindings(&default_codex_home(), &state.profile_backup_root()).map_err(Into::into)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepairPreviewInput {
    profile_dirs: Vec<String>,
    target_provider: repair::TargetProvider,
}

#[tauri::command]
pub fn preview_codex_history_repair(
    input: RepairPreviewInput,
    state: State<'_, DesktopState>,
) -> Result<repair::RepairPreview, CommandError> {
    let profiles = if input.profile_dirs.is_empty() {
        vec![resolve_profile_dir(None)?]
    } else {
        input
            .profile_dirs
            .into_iter()
            .map(|path| resolve_profile_dir(Some(path)))
            .collect::<Result<Vec<_>, _>>()?
    };
    repair::preview(
        &state.transient_root(),
        &profiles,
        input.target_provider,
        crate::launcher::is_codex_running(),
    )
    .map_err(repair_error)
}

#[tauri::command]
pub async fn apply_codex_history_repair(
    session_id: String,
    state: State<'_, DesktopState>,
) -> Result<repair::RepairResult, CommandError> {
    let _mutation = state.setup_guard().await;
    ensure_codex_stopped()?;
    repair::apply(
        &state.transient_root(),
        &state.history_repair_backup_root(),
        &session_id,
    )
    .map_err(repair_error)
}

#[tauri::command]
pub async fn rollback_codex_history_repair(
    backup_id: String,
    state: State<'_, DesktopState>,
) -> Result<repair::RollbackResult, CommandError> {
    let _mutation = state.setup_guard().await;
    ensure_codex_stopped()?;
    repair::rollback(&state.history_repair_backup_root(), &backup_id).map_err(repair_error)
}

#[tauri::command]
pub async fn restore_codex_account_profile(
    profile_dir: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<Option<codex::ProfileBinding>, CommandError> {
    let _mutation = state.setup_guard().await;
    let profile_dir = resolve_profile_dir(profile_dir)?;
    let stopped = stop_codex_and_sync_account_at(&state, &profile_dir).await?;
    let result = codex::restore_account_profile(&profile_dir, &state.profile_backup_root())
        .map_err(Into::into);
    restart_codex_after_failed_change(stopped, result, launch_codex_with_profile)
}

#[tauri::command]
pub fn list_codex_profile_snapshots(
    state: State<'_, DesktopState>,
) -> Result<Vec<snapshots::ProfileSnapshotSummary>, CommandError> {
    snapshots::list(&state.profile_backup_root()).map_err(Into::into)
}

#[tauri::command]
pub async fn create_codex_profile_snapshot(
    name: String,
    state: State<'_, DesktopState>,
) -> Result<snapshots::ProfileSnapshotSummary, CommandError> {
    let _mutation = state.setup_guard().await;
    snapshots::create(&default_codex_home(), &state.profile_backup_root(), &name)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn restore_codex_profile_snapshot(
    snapshot_id: String,
    safety_name: String,
    state: State<'_, DesktopState>,
) -> Result<snapshots::ProfileSnapshotSummary, CommandError> {
    let _mutation = state.setup_guard().await;
    let stopped = stop_codex_and_sync_account(&state).await?;
    let result = snapshots::restore(
        &default_codex_home(),
        &state.profile_backup_root(),
        &snapshot_id,
        &safety_name,
    )
    .map_err(Into::into);
    restart_codex_after_failed_change(stopped, result, launch_codex_with_profile)
}

#[tauri::command]
pub async fn delete_codex_profile_snapshot(
    snapshot_id: String,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    snapshots::delete(&state.profile_backup_root(), &snapshot_id).map_err(Into::into)
}

async fn activate_account_profile(
    account_id: &str,
    profile_dir: Option<String>,
    state: &DesktopState,
) -> Result<ProfileActivation, CommandError> {
    let profile_dir = resolve_profile_dir(profile_dir)?;
    let previous = codex::credential_kind(&profile_dir, &state.profile_backup_root())?;
    let stopped = stop_codex_and_sync_account_at(state, &profile_dir).await?;
    let result: Result<ProfileActivation, CommandError> = async {
        let prepared = prepare_account_credentials(state, account_id).await?;
        let binding = codex::attach_account(
            &profile_dir,
            &state.profile_backup_root(),
            account_id,
            prepared.tokens(),
            prepared.provider_account_id(),
        )?;
        Ok(profile_activation(binding, previous, stopped))
    }
    .await;
    let result = restart_codex_after_failed_change(stopped, result, launch_codex_with_profile);
    if result.is_ok() {
        set_runtime_pool_interface_reserve(state, None).await;
    }
    result
}

async fn set_runtime_pool_interface_reserve(state: &DesktopState, account_id: Option<&str>) {
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.set_protected_candidate(
            account_id,
            super::CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS,
        );
    }
}

fn profile_activation(
    binding: codex::ProfileBinding,
    previous: Option<codex::ProfileCredentialKind>,
    stopped_running_client: bool,
) -> ProfileActivation {
    ProfileActivation {
        repair_recommended: previous.is_some_and(|kind| kind != binding.credential_kind),
        binding,
        previous_credential_kind: previous,
        stopped_running_client,
    }
}

fn stop_codex_for_profile_change() -> Result<bool, CommandError> {
    stop_codex_and_wait().map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to stop ChatGPT before changing its profile: {error}"),
        )
        .into()
    })
}

async fn stop_codex_and_sync_account(state: &DesktopState) -> Result<bool, CommandError> {
    stop_codex_and_sync_account_at(state, &default_codex_home()).await
}

async fn stop_codex_and_sync_account_at(
    state: &DesktopState,
    profile_dir: &std::path::Path,
) -> Result<bool, CommandError> {
    let stopped = stop_codex_for_profile_change()?;
    let result: Result<(), CommandError> = async {
        if let Some(account_id) =
            codex::active_managed_account_id(profile_dir, &state.profile_backup_root())?
        {
            sync_managed_account_profile(state, &account_id).await?;
        }
        Ok(())
    }
    .await;
    restart_codex_after_failed_change(stopped, result, launch_codex_with_profile)?;
    Ok(stopped)
}

fn restart_codex_after_failed_change<T>(
    stopped: bool,
    result: Result<T, CommandError>,
    launch: impl FnOnce() -> Result<(), String>,
) -> Result<T, CommandError> {
    match result {
        Err(mut error) if stopped => {
            if let Err(launch_error) = launch() {
                error.message = format!(
                    "{}; failed to restart ChatGPT: {launch_error}",
                    error.message
                );
            }
            Err(error)
        }
        result => result,
    }
}

fn resolve_profile_dir(profile_dir: Option<String>) -> Result<PathBuf, CommandError> {
    let Some(profile_dir) = profile_dir else {
        return Ok(default_codex_home());
    };
    let profile_dir = profile_dir.trim();
    if profile_dir.is_empty() || profile_dir.chars().any(char::is_control) {
        return Err(LocalPoolError::new(ErrorCode::InvalidState, "profile path is invalid").into());
    }
    let path = PathBuf::from(profile_dir);
    if !path.is_absolute() {
        return Err(
            LocalPoolError::new(ErrorCode::InvalidState, "profile path must be absolute").into(),
        );
    }
    let canonical = std::fs::canonicalize(&path).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to access profile path: {error}"),
        )
    })?;
    if !canonical.is_dir() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "profile path is not a directory",
        )
        .into());
    }
    Ok(canonical)
}

fn repair_error(error: String) -> CommandError {
    LocalPoolError::new(ErrorCode::RecoveryRequired, error).into()
}

fn ensure_codex_stopped() -> Result<(), CommandError> {
    if crate::launcher::is_codex_running() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "close all ChatGPT instances before changing history",
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn failed_profile_change_restarts_a_previously_running_codex() {
        let launched = Cell::new(false);
        let error = restart_codex_after_failed_change::<()>(
            true,
            Err(LocalPoolError::new(ErrorCode::Conflict, "profile conflict").into()),
            || {
                launched.set(true);
                Ok(())
            },
        )
        .unwrap_err();

        assert!(launched.get());
        assert!(matches!(error.code, ErrorCode::Conflict));
    }

    #[test]
    fn oauth_binding_order_is_stable_and_preserves_the_active_account() {
        let mut candidates = vec![
            ("account-z".into(), 9_000),
            ("account-a".into(), 8_000),
            ("account-m".into(), 1_000),
        ];

        prioritize_account_candidates(&mut candidates, Some("account-m"), true);

        assert_eq!(
            candidates,
            [
                ("account-m".to_string(), 1_000),
                ("account-z".to_string(), 9_000),
                ("account-a".to_string(), 8_000),
            ]
        );
    }

    #[test]
    fn automatic_oauth_binding_skips_the_reserved_account() {
        let mut candidates = vec![
            ("preferred".into(), 100),
            ("highest".into(), 9_000),
            ("available".into(), 5_000),
            ("unknown".into(), 0),
        ];

        prioritize_account_candidates(&mut candidates, Some("preferred"), true);

        assert_eq!(
            candidates,
            [
                ("highest".to_string(), 9_000),
                ("available".to_string(), 5_000),
            ]
        );
    }

    #[test]
    fn manual_oauth_binding_keeps_the_explicit_account() {
        let mut candidates = vec![("selected".into(), 100), ("highest".into(), 9_000)];

        prioritize_account_candidates(&mut candidates, Some("selected"), false);

        assert_eq!(candidates[0], ("selected".to_string(), 100));
    }

    #[test]
    fn oauth_binding_request_distinguishes_none_automatic_and_manual() {
        assert_eq!(
            gateway_oauth_binding_request(true, None).unwrap(),
            GatewayOAuthBindingRequest::Disabled
        );
        assert_eq!(
            gateway_oauth_binding_request(false, None).unwrap(),
            GatewayOAuthBindingRequest::Automatic
        );
        assert_eq!(
            gateway_oauth_binding_request(false, Some(" account-a ")).unwrap(),
            GatewayOAuthBindingRequest::Account("account-a")
        );
        assert!(gateway_oauth_binding_request(true, Some("account-a")).is_err());
    }
}
