use super::super::{account_summary, store_error, ManagementError};
use super::preview::AccountImportPreview;
use crate::jobs;
use crate::state::{now_ms, AppState, ServerAccountRecord};
use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use zenith_relay_core::accounts::AccountHealthState;
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::{valid_generated_id, AccountSummary};
use zenith_relay_core::quota::{Subscription, SubscriptionInput};

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
    if now_ms().saturating_sub(pending.created_at_ms) > 30 * 60 * 1_000 {
        let _ = state.store.delete_pending_import(session_id);
        let _ = state.vault.delete(&pending.secret_ref);
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
    let existing_account = state
        .store
        .accounts()
        .map_err(store_error)?
        .into_iter()
        .find(|account_record| account_record.id == preview.account_id);
    let mut subscription =
        if preview.plan_type.is_some() || preview.subscription_active_until_ms.is_some() {
            Subscription::normalize(SubscriptionInput {
                plan_type: preview.plan_type.clone(),
                active_until_ms: preview.subscription_active_until_ms,
                forbidden: false,
                observed_at_ms: now_ms(),
            })
        } else {
            existing_account
                .as_ref()
                .map(|existing_record| existing_record.subscription.clone())
                .unwrap_or_default()
        };
    if subscription.active_until_ms.is_none() {
        subscription.updated_at_ms = None;
    }
    let mut account_record = ServerAccountRecord {
        id: preview.account_id.clone(),
        label: existing_account
            .as_ref()
            .map(|existing_record| existing_record.label.clone())
            .unwrap_or(preview.label),
        identity_hint: preview.identity_hint,
        enabled: existing_account
            .as_ref()
            .is_none_or(|existing_record| existing_record.enabled),
        in_pool: add_to_pool
            || existing_account
                .as_ref()
                .is_some_and(|existing_record| existing_record.in_pool),
        draining: existing_account
            .as_ref()
            .is_some_and(|existing_record| existing_record.draining),
        source_id: "openai_codex".to_string(),
        secret_ref: pending.secret_ref.clone(),
        provider_family: existing_account
            .as_ref()
            .and_then(|existing_record| existing_record.provider_family.clone())
            .or_else(|| Some("openai".to_string())),
        auth_state: preview.auth_state,
        health: AccountHealthState::Healthy,
        models: existing_account
            .as_ref()
            .map(|existing_record| existing_record.models.clone())
            .unwrap_or(preview.models),
        discovered_models: existing_account
            .as_ref()
            .and_then(|existing_record| existing_record.discovered_models.clone()),
        allowed_models: existing_account
            .as_ref()
            .map(|existing_record| existing_record.allowed_models.clone())
            .unwrap_or(preview.allowed_models),
        excluded_models: existing_account
            .as_ref()
            .map(|existing_record| existing_record.excluded_models.clone())
            .unwrap_or(preview.excluded_models),
        priority: existing_account
            .as_ref()
            .map_or(preview.priority, |existing_record| existing_record.priority),
        weight: existing_account
            .as_ref()
            .map_or(preview.weight, |existing_record| existing_record.weight),
        subscription,
        quota: existing_account
            .as_ref()
            .map(|existing_record| existing_record.quota.clone())
            .unwrap_or_default(),
        purchase_cost_micro_usd: existing_account
            .as_ref()
            .and_then(|existing_record| existing_record.purchase_cost_micro_usd),
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
        created_at_ms: existing_account
            .as_ref()
            .map(|existing_record| existing_record.created_at_ms)
            .filter(|created_at_ms| *created_at_ms > 0)
            .unwrap_or(pending.created_at_ms),
        last_used_at_ms: existing_account
            .as_ref()
            .and_then(|existing_record| existing_record.last_used_at_ms),
        last_error_code: None,
        proxy_id: existing_account
            .as_ref()
            .and_then(|existing_record| existing_record.proxy_id.clone()),
        bypass_common_proxy: existing_account
            .as_ref()
            .is_some_and(|existing_record| existing_record.bypass_common_proxy),
    };
    // A replacement login must close pending final dispatches before its
    // durable reference changes. The build lock prevents another publication;
    // this fence closes the still-live runtime through commit and replacement.
    let previous_runtime = state.runtime().map_err(super::super::runtime_error)?;
    let _dispatch_fence = previous_runtime
        .as_ref()
        .and_then(|runtime| runtime.fence_candidate_dispatch(&account_record.id));
    let created = state
        .store
        .save_account_and_consume_pending_import(&account_record, session_id)
        .map_err(store_error)?;
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
                mark_import_failure(state, &account_record, "metadata_refresh_failed");
            }
        }
    } else if !rebuilt {
        mark_import_failure(state, &account_record, "runtime_rebuild_failed");
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

fn mark_import_failure(state: &AppState, imported_account: &ServerAccountRecord, code: &str) {
    // A later import may already have installed another credential under the
    // same account id. Never save the captured pre-HTTP record over it.
    if let Ok((stored_account, fence)) = state.store.account_refresh_scope(&imported_account.id) {
        if stored_account.secret_ref == imported_account.secret_ref {
            let _ = state.store.apply_account_refresh(&fence, |account_record| {
                account_record.health = AccountHealthState::Degraded;
                account_record.last_error_code = Some(code.to_string());
                Ok(())
            });
        }
    }
}

pub(super) fn cleanup_expired_imports(state: &AppState) -> Result<(), ManagementError> {
    let cutoff = now_ms().saturating_sub(30 * 60 * 1_000);
    for secret_ref in state
        .store
        .delete_pending_imports_before(cutoff)
        .map_err(store_error)?
    {
        let _ = state.vault.delete(&secret_ref);
    }
    Ok(())
}
