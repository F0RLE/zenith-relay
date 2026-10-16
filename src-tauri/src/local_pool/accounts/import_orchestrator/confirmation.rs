use super::{
    existing_identity_index, import_account_item, import_session_error, import_source_item,
    normalize_models, normalize_selected_item_ids, parse_subscription_timestamp_ms,
    AccountImportOptions, AccountImportProgressEvent, ConfirmAccountImportInput, ImportItemError,
    ImportItemStatus, ImportRowContext, ACCOUNT_IMPORT_PROGRESS_EVENT,
};
use crate::local_pool::accounts::credentials::CredentialStore;
use crate::local_pool::accounts::import_session::ImportSessionStore;
use crate::local_pool::accounts::quota_refresh::{ConfirmAccountImportResponse, ImportItemResult};
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError};
use crate::local_pool::state::DesktopState;
use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Emitter};
use zenith_relay_core::accounts::{ImportAuthMode, ImportQuotaStatus, ParsedImportItem};
use zenith_relay_core::error_codes;

type CommandResult<T> = std::result::Result<T, CommandError>;

pub(in crate::local_pool::accounts) async fn confirm_local_account_import_inner(
    input: ConfirmAccountImportInput,
    state: &DesktopState,
    app: Option<&AppHandle>,
) -> CommandResult<ConfirmAccountImportResponse> {
    let selected_item_ids = normalize_selected_item_ids(input.selected_item_ids)?;
    let session_hash = crate::diagnostics::hash_identifier(&input.session_id);
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
    crate::diagnostics::breadcrumb(
        "account-import",
        "session_resolved",
        &[
            ("session", session_hash.clone()),
            ("selected_count", selected_item_ids.len().to_string()),
        ],
    );
    let selected = selected_item_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let refresh_exchange_required = !session.prepared
        && session.items.iter().any(|import_item| {
            selected.contains(import_item.item_id.as_str())
                && import_item.secrets().access_token().is_none()
                && import_item.secrets().refresh_token().is_some()
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
    let mut import_items_by_id = session
        .items
        .into_iter()
        .map(|import_item| (import_item.item_id.clone(), import_item))
        .collect::<HashMap<_, _>>();
    let mut results = Vec::with_capacity(selected_item_ids.len());
    let total = selected_item_ids.len();
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    emit_account_import_progress(app, &input.session_id, 0, total, succeeded, failed, None);

    let mut batch = ConfirmImport {
        state,
        credentials: &credentials,
        pending_items: &mut import_items_by_id,
        add_to_pool: input.add_to_pool,
        discover_models: input.discover_models,
        probe_quota,
        configured_models: &configured_models,
    };
    for (completed, item_id) in selected_item_ids.into_iter().enumerate() {
        let item_hash = crate::diagnostics::hash_identifier(&item_id);
        crate::diagnostics::breadcrumb(
            "account-import",
            "item_started",
            &[
                ("session", session_hash.clone()),
                ("item", item_hash.clone()),
                ("index", completed.to_string()),
                ("total", total.to_string()),
            ],
        );
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
        let row_context = row_context.get(&item_id);
        let import_result = import_confirmed_item(&mut batch, item_id, row_context).await;
        match import_result.status {
            ImportItemStatus::Succeeded => succeeded += 1,
            ImportItemStatus::Failed => {
                failed += 1;
                if let Some(error) = import_result.error.as_ref() {
                    crate::diagnostics::record_error(
                        "account-import",
                        Some(&error.code),
                        &error.message,
                        &[
                            ("session", session_hash.clone()),
                            ("item", item_hash.clone()),
                            ("index", completed.to_string()),
                        ],
                    );
                }
            }
        }
        results.push(import_result);
        crate::diagnostics::breadcrumb(
            "account-import",
            "item_finished",
            &[
                ("session", session_hash.clone()),
                ("item", item_hash),
                ("completed", (completed + 1).to_string()),
                ("failed", failed.to_string()),
            ],
        );
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
        crate::diagnostics::breadcrumb(
            "account-import",
            "session_completed",
            &[("session", session_hash.clone())],
        );
    }
    Ok(ConfirmAccountImportResponse {
        session_id: input.session_id,
        results,
    })
}

struct ConfirmImport<'a> {
    state: &'a DesktopState,
    credentials: &'a CredentialStore<NativeSecretBackend>,
    pending_items: &'a mut HashMap<String, ParsedImportItem>,
    add_to_pool: bool,
    discover_models: bool,
    probe_quota: bool,
    configured_models: &'a [String],
}

async fn import_confirmed_item(
    batch: &mut ConfirmImport<'_>,
    item_id: String,
    context: Option<&ImportRowContext>,
) -> ImportItemResult {
    let Some(context) = context else {
        return ImportItemResult::failure(
            item_id,
            ImportItemError::new(error_codes::ITEM_NOT_FOUND, "import item was not found"),
        );
    };
    if !context.selectable {
        return ImportItemResult::failure(
            item_id,
            ImportItemError::new(
                error_codes::ITEM_NOT_SELECTABLE,
                "import item cannot be selected",
            ),
        );
    }
    let Some(import_item) = batch.pending_items.remove(&item_id) else {
        return ImportItemResult::failure(
            item_id,
            ImportItemError::new(
                error_codes::ITEM_NOT_SELECTABLE,
                "import item has no usable credentials",
            ),
        );
    };
    if context.auth_mode == ImportAuthMode::ApiKey {
        return match import_source_item(
            batch.state,
            import_item,
            batch.add_to_pool,
            batch.discover_models,
            batch.configured_models,
        )
        .await
        {
            Ok(source) => ImportItemResult::source_success(item_id, source),
            Err(error) => ImportItemResult::failure(item_id, error),
        };
    }
    match import_account_item(
        batch.state,
        batch.credentials,
        import_item,
        context,
        AccountImportOptions {
            add_to_pool: batch.add_to_pool,
            discover_models: batch.discover_models,
            probe_quota: batch.probe_quota,
            configured_models: batch.configured_models,
        },
        batch.state.account_check_url(),
    )
    .await
    {
        Ok((account, quota)) => ImportItemResult::account_success(item_id, account, quota),
        Err(error) => ImportItemResult::failure(item_id, error),
    }
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
