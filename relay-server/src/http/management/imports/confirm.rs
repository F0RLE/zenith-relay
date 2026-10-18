use super::super::{account_summary, store_error, validation_error, vault_error, ManagementError};
use super::preview::AccountImportPreview;
use crate::jobs;
use crate::state::{now_ms, AccountCredential, AppState, ServerAccountRecord};
use crate::store::{AccountRefreshFence, PendingImport};
use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;
use zenith_relay_core::accounts::AccountHealthState;
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::{valid_generated_id, AccountSummary};

mod record;
use record::{confirmed_account_record, ConfirmedAccountRecordInput};

const IMPORT_SESSION_LIFETIME_MS: u64 = 30 * 60 * 1_000;

pub(super) struct ConfirmedAccountImport {
    pub(super) account: ServerAccountRecord,
    pub(super) created: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmImportInput {
    session_id: String,
    #[serde(default)]
    add_to_pool: bool,
    #[serde(default)]
    probe_metadata: bool,
}

pub async fn confirm_account_import(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ConfirmImportInput>,
) -> Result<Json<AccountSummary>, ManagementError> {
    confirm_one_account_import(
        &state,
        &input.session_id,
        None,
        input.add_to_pool,
        input.probe_metadata,
    )
    .await
    .and_then(|confirmed| account_summary(&state, &confirmed.account).map(Json))
}

pub(super) async fn confirm_one_account_import(
    state: &Arc<AppState>,
    session_id: &str,
    batch_session_id: Option<&str>,
    add_to_pool: bool,
    probe_metadata: bool,
) -> Result<ConfirmedAccountImport, ManagementError> {
    if !valid_generated_id(session_id, "import_") {
        return Err(ManagementError::validation(
            error_codes::IMPORT_SESSION_INVALID,
            "account import session is invalid",
        ));
    }
    let pending = state
        .store
        .pending_import(session_id)
        .map_err(store_error)?
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::IMPORT_NOT_FOUND, "import session not found")
        })?;
    if now_ms().saturating_sub(pending.created_at_ms) > IMPORT_SESSION_LIFETIME_MS {
        discard_expired_import(state, &pending)?;
        return Err(ManagementError::validation(
            error_codes::IMPORT_EXPIRED,
            "import session expired",
        ));
    }
    let preview: AccountImportPreview =
        serde_json::from_str(&pending.preview_json).map_err(|_| {
            ManagementError::internal(
                error_codes::PREVIEW_INVALID,
                "stored import preview is invalid",
            )
        })?;
    if preview.batch_session_id.as_deref() != batch_session_id {
        return Err(ManagementError::not_found(
            error_codes::IMPORT_NOT_FOUND,
            "import session not found",
        ));
    }
    // Serialize the credential switch with token persistence/preparation.
    // Never hold this lock across metadata HTTP or a runtime rebuild.
    let configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let credential = state.account_credential_lock.lock().await;
    let current_pending = state
        .store
        .pending_import(session_id)
        .map_err(store_error)?
        .filter(|current| current.secret_ref == pending.secret_ref)
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::IMPORT_NOT_FOUND, "import session not found")
        })?;
    if now_ms().saturating_sub(current_pending.created_at_ms) > IMPORT_SESSION_LIFETIME_MS {
        discard_expired_import(state, &current_pending)?;
        return Err(ManagementError::validation(
            error_codes::IMPORT_EXPIRED,
            "import session expired",
        ));
    }
    let pending_secret = state
        .vault
        .load(&current_pending.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| validation_error("prepared account credential is unavailable"))?;
    let mut pending_credential: AccountCredential = serde_json::from_str(&pending_secret)
        .map_err(|_| validation_error("prepared account credential is invalid"))?;
    pending_credential
        .validate_oauth_client()
        .map_err(validation_error)?;
    pending_credential
        .validate_plan(preview.plan_type.as_deref())
        .map_err(|message| {
            ManagementError::validation(error_codes::BASIS_POINTS_FREE_ACCOUNT_UNSUPPORTED, message)
        })?;
    for trashed_account in state.store.trashed_accounts().map_err(store_error)? {
        let Some(retained_secret) = state
            .vault
            .load(&trashed_account.secret_ref)
            .map_err(vault_error)?
        else {
            continue;
        };
        let retained_credential: AccountCredential = serde_json::from_str(&retained_secret)
            .map_err(|_| validation_error("trashed account credential is invalid"))?;
        if retained_credential.matches_connection(
            pending_credential.oauth_client_kind,
            &pending_credential.chatgpt_account_id,
            pending_credential.principal_user_id().as_deref(),
        ) {
            return Err(validation_error(
                "account is in trash; restore it before importing",
            ));
        }
    }
    let mut existing_account = None;
    for account_record in state.store.accounts().map_err(store_error)? {
        let Some(existing_secret) = state
            .vault
            .load(&account_record.secret_ref)
            .map_err(vault_error)?
        else {
            if account_record.id == preview.account_id {
                return Err(validation_error(
                    "existing account credential is unavailable",
                ));
            }
            continue;
        };
        let existing_credential: AccountCredential = serde_json::from_str(&existing_secret)
            .map_err(|_| validation_error("existing account credential is invalid"))?;
        let same_connection = pending_credential.matches_connection(
            existing_credential.oauth_client_kind,
            &existing_credential.chatgpt_account_id,
            existing_credential.principal_user_id().as_deref(),
        );
        if account_record.id != preview.account_id {
            if same_connection {
                return Err(validation_error(
                    "account OAuth connection was imported after preview; prepare a new preview",
                ));
            }
            continue;
        }
        if !same_connection {
            return Err(validation_error(
                "account OAuth connection changed after import preview",
            ));
        }
        // Read omitted session material at commit time so a background token
        // refresh or header update cannot be replaced by a stale preview copy.
        if pending_credential.refresh_token.is_none() {
            pending_credential.refresh_token = existing_credential.refresh_token;
        }
        if pending_credential.basis_points_headers.is_none() {
            pending_credential.basis_points_headers = existing_credential.basis_points_headers;
        }
        pending_credential.proxy_url = existing_credential.proxy_url;
        pending_credential.generation = existing_credential.generation.saturating_add(1);
        existing_account = Some(account_record);
    }
    if preview.duplicate_account_id.is_some() && existing_account.is_none() {
        return Err(validation_error(
            "account was removed after import preview; prepare a new preview",
        ));
    }
    let prepared_secret = serde_json::to_string(&pending_credential)
        .map_err(|_| validation_error("prepared account credential could not be saved"))?;
    let mut account_record = confirmed_account_record(ConfirmedAccountRecordInput {
        preview,
        credential: &pending_credential,
        existing: existing_account.as_ref(),
        secret_ref: pending.secret_ref.clone(),
        created_at_ms: pending.created_at_ms,
        observed_at_ms: now_ms(),
        add_to_pool,
    })?;
    // A replacement login must close pending final dispatches before its
    // durable reference changes. The build lock prevents another publication;
    // this fence closes the still-live runtime through commit and replacement.
    let previous_runtime = state.runtime().map_err(super::super::runtime_error)?;
    let _dispatch_fence = previous_runtime
        .as_ref()
        .and_then(|runtime| runtime.fence_candidate_dispatch(&account_record.id));
    state
        .vault
        .save(&current_pending.secret_ref, &prepared_secret)
        .map_err(vault_error)?;
    let created = match state
        .store
        .save_account_and_consume_pending_import(&account_record, session_id)
    {
        Ok(created) => created,
        Err(error) => {
            state
                .vault
                .save(&current_pending.secret_ref, &pending_secret)
                .map_err(vault_error)?;
            return Err(store_error(error));
        }
    };
    // Follow-up diagnostics are best-effort, but their ownership is not:
    // capture the committed login/configuration before releasing its locks.
    let import_refresh_fence = state
        .store
        .account_refresh_scope(&account_record.id)
        .ok()
        .map(|(_, fence)| fence);
    state.token_authority.remove(&account_record.id);
    if let Some(runtime) = previous_runtime.as_ref() {
        runtime.remove_candidate(&account_record.id);
    }
    drop(credential);
    drop(configuration);
    // Publish the new incarnation before metadata HTTP. A build that captured
    // the old login finished before our commit; newer builds wait for this one.
    let rebuilt = build.rebuild(state).await.is_ok();
    drop(build);
    if probe_metadata {
        match jobs::refresh_account_now(state, account_record.clone()).await {
            Ok(updated_account) => account_record = updated_account,
            Err(_) => {
                mark_import_failure(
                    state,
                    import_refresh_fence.as_ref(),
                    "metadata_refresh_failed",
                );
            }
        }
    } else if !rebuilt {
        mark_import_failure(
            state,
            import_refresh_fence.as_ref(),
            "runtime_rebuild_failed",
        );
    }
    if let Some(previous_account) = existing_account {
        if previous_account.secret_ref != account_record.secret_ref {
            let _ = state.vault.delete(&previous_account.secret_ref);
        }
    }
    Ok(ConfirmedAccountImport {
        account: account_record,
        created,
    })
}

fn discard_expired_import(
    state: &AppState,
    pending: &PendingImport,
) -> Result<(), ManagementError> {
    // A concurrent confirmation may have consumed the pending row and made
    // this same secret the account's active credential.
    if state
        .store
        .delete_pending_import(pending)
        .map_err(store_error)?
    {
        state
            .vault
            .delete(&pending.secret_ref)
            .map_err(vault_error)?;
    }
    Ok(())
}

fn mark_import_failure(state: &AppState, fence: Option<&AccountRefreshFence>, code: &str) {
    let Some(fence) = fence else {
        return;
    };
    let _ = state.store.apply_account_refresh(fence, |account| {
        // A generic follow-up error must not hide a newer quota/auth/model
        // diagnosis, even when it belongs to the same login incarnation.
        if account.health == AccountHealthState::Healthy && account.last_error_code.is_none() {
            account.health = AccountHealthState::Degraded;
            account.last_error_code = Some(code.to_string());
        }
        Ok(())
    });
}

pub(super) fn cleanup_expired_imports(state: &AppState) -> Result<(), ManagementError> {
    let cutoff = now_ms().saturating_sub(IMPORT_SESSION_LIFETIME_MS);
    for secret_ref in state
        .store
        .delete_pending_imports_before(cutoff)
        .map_err(store_error)?
    {
        let _ = state.vault.delete(&secret_ref);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
