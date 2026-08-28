use super::{
    existing_identity_index, import_account_item, import_session_error, import_source_item,
    normalize_models, normalize_selected_item_ids, parse_subscription_timestamp_ms,
    AccountImportProgressEvent, ConfirmAccountImportInput, ImportItemError, ImportItemStatus,
    ImportRowContext, ACCOUNT_IMPORT_PROGRESS_EVENT,
};
use crate::local_pool::accounts::credentials::CredentialStore;
use crate::local_pool::accounts::import_session::ImportSessionStore;
use crate::local_pool::accounts::quota_refresh::{ConfirmAccountImportResponse, ImportItemResult};
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError};
use crate::local_pool::state::DesktopState;
use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Emitter};
use zenith_relay_core::accounts::{ImportAuthMode, ImportQuotaStatus};

type CommandResult<T> = std::result::Result<T, CommandError>;

pub(in crate::local_pool::accounts) async fn confirm_local_account_import_inner(
    input: ConfirmAccountImportInput,
    state: &DesktopState,
    app: Option<&AppHandle>,
) -> CommandResult<ConfirmAccountImportResponse> {
    let selected_item_ids = normalize_selected_item_ids(input.selected_item_ids)?;
    let configured_models = normalize_models(input.models.clone())?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let existing = existing_identity_index(state, &credentials)?;
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions
        .resume(
            &input.session_id,
            &existing.keys().cloned().collect::<Vec<_>>(),
        )
        .map_err(import_session_error)?;
    let selected = selected_item_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let refresh_exchange_required = !session.prepared
        && session.items.iter().any(|item| {
            selected.contains(item.item_id.as_str())
                && item.secrets().access_token().is_none()
                && item.secrets().refresh_token().is_some()
        });
    let probe_quota = input.probe_quota
        && !session.preview.rows.iter().any(|row| {
            selected.contains(row.item_id.as_str())
                && row.auth_mode != ImportAuthMode::ApiKey
                && row.quota_status == ImportQuotaStatus::Skipped
        });
    if refresh_exchange_required {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "prepare refresh-only credentials before confirming selected accounts",
        )
        .into());
    }
    let row_context = session
        .preview
        .rows
        .iter()
        .map(|row| {
            (
                row.item_id.clone(),
                ImportRowContext {
                    label: row.label.clone(),
                    auth_mode: row.auth_mode,
                    selectable: row.selectable,
                    plan: row.plan.clone(),
                    subscription_active_until_ms: row
                        .subscription_expires_at
                        .as_deref()
                        .and_then(parse_subscription_timestamp_ms),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let mut items = session
        .items
        .into_iter()
        .map(|item| (item.item_id.clone(), item))
        .collect::<HashMap<_, _>>();
    let mut results = Vec::with_capacity(selected_item_ids.len());
    let total = selected_item_ids.len();
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    emit_account_import_progress(app, &input.session_id, 0, total, succeeded, failed, None);

    for (completed, item_id) in selected_item_ids.into_iter().enumerate() {
        let label = row_context
            .get(&item_id)
            .map(|context| context.label.clone())
            .unwrap_or_else(|| item_id.clone());
        emit_account_import_progress(
            app,
            &input.session_id,
            completed,
            total,
            succeeded,
            failed,
            Some(label),
        );
        let result = match row_context.get(&item_id) {
            None => ImportItemResult::failure(
                item_id,
                ImportItemError::new("item_not_found", "import item was not found"),
            ),
            Some(context) if !context.selectable => ImportItemResult::failure(
                item_id,
                ImportItemError::new("item_not_selectable", "import item cannot be selected"),
            ),
            Some(context) => match items.remove(&item_id) {
                None => ImportItemResult::failure(
                    item_id,
                    ImportItemError::new(
                        "item_not_selectable",
                        "import item has no usable credentials",
                    ),
                ),
                Some(item) if context.auth_mode == ImportAuthMode::ApiKey => {
                    match import_source_item(
                        state,
                        item,
                        input.add_to_pool,
                        input.discover_models,
                        &configured_models,
                    )
                    .await
                    {
                        Ok(source) => ImportItemResult::source_success(item_id, source),
                        Err(error) => ImportItemResult::failure(item_id, error),
                    }
                }
                Some(item) => match import_account_item(
                    state,
                    &credentials,
                    item,
                    context,
                    input.add_to_pool,
                    input.discover_models,
                    probe_quota,
                    &configured_models,
                )
                .await
                {
                    Ok((account, quota)) => {
                        ImportItemResult::account_success(item_id, account, quota)
                    }
                    Err(error) => ImportItemResult::failure(item_id, error),
                },
            },
        };
        match result.status {
            ImportItemStatus::Succeeded => succeeded += 1,
            ImportItemStatus::Failed => failed += 1,
        }
        results.push(result);
        emit_account_import_progress(
            app,
            &input.session_id,
            completed + 1,
            total,
            succeeded,
            failed,
            None,
        );
    }

    if failed == 0 {
        sessions
            .complete(&input.session_id)
            .map_err(import_session_error)?;
    }
    Ok(ConfirmAccountImportResponse {
        session_id: input.session_id,
        results,
    })
}

fn emit_account_import_progress(
    app: Option<&AppHandle>,
    session_id: &str,
    completed: usize,
    total: usize,
    succeeded: usize,
    failed: usize,
    current_label: Option<String>,
) {
    if let Some(app) = app {
        let _ = app.emit(
            ACCOUNT_IMPORT_PROGRESS_EVENT,
            AccountImportProgressEvent {
                session_id: session_id.to_string(),
                completed,
                total,
                succeeded,
                failed,
                current_label,
            },
        );
    }
}
