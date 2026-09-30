use super::{
    build_import_credential_material, credential_local_error, existing_identity_index,
    find_existing_account, find_existing_source, hinted_import_proxy, import_item_command_error,
    import_session_error, masked_account_identity, normalize_import_input,
    parse_subscription_timestamp_ms, parsed_item_value, parsed_item_value_from_material,
    provider_identity_key, timestamp_from_ms, ImportSessionResponse, StartAccountImportInput,
};
use crate::local_pool::accounts::credentials::CredentialStore;
use crate::local_pool::accounts::import_session::{ImportSession, ImportSessionStore};
use crate::local_pool::accounts::proxy::{
    common_proxy_config, effective_proxy_config, ensure_account_proxy,
};
use crate::local_pool::accounts::quota_refresh::QUOTA_COMMAND_TIMEOUT_OVERHEAD;
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::commands::current_time_ms;
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError};
use crate::local_pool::state::DesktopState;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use zenith_relay_core::accounts::{
    ImportAuthMode, ImportIssue, ImportIssueCode, ImportPreview, ImportPreviewStatus,
    ImportQuotaStatus,
};
use zenith_relay_core::error_codes;
use zenith_relay_core::providers::chatgpt::CodexQuotaClient;

mod prepare;
pub(super) use prepare::prepare_import_preview;

type CommandResult<T> = std::result::Result<T, CommandError>;

pub(super) async fn preview_account_import_documents(
    documents: Vec<String>,
    state: &DesktopState,
) -> CommandResult<ImportSessionResponse> {
    let _mutation = state.setup_guard().await;
    let started = Instant::now();
    let document_count = documents.len();
    crate::diagnostics::breadcrumb(
        "account-import",
        "document_preview_started",
        &[("documents", document_count.to_string())],
    );
    let result = async {
        let (content, _) = normalize_import_input(StartAccountImportInput {
            content: None,
            documents,
            source_file: None,
        })?;
        let credentials = CredentialStore::from_backend(NativeSecretBackend);
        let existing = existing_identity_index(state, &credentials)?;
        let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
        let session = sessions
            .start(
                &content,
                None,
                &existing.keys().cloned().collect::<Vec<_>>(),
            )
            .map_err(import_session_error)?;
        let session_id = session.session_id.clone();
        let session_hash = crate::diagnostics::hash_identifier(&session_id);
        let prepared = async {
            let (content, preview) =
                prepare_import_preview(state, &credentials, session, false).await?;
            sessions
                .prepare(
                    &session_id,
                    content.as_deref(),
                    preview,
                    &existing.keys().cloned().collect::<Vec<_>>(),
                )
                .map_err(import_session_error)
        }
        .await;
        match prepared {
            Ok(session) => Ok((session.into(), session_hash)),
            Err(error) => {
                let _ = sessions.cancel(&session_id);
                Err(error)
            }
        }
    }
    .await;
    match result {
        Ok((session, session_hash)) => {
            crate::diagnostics::record_operation(
                "account-import",
                "document_preview_completed",
                &[
                    ("session", session_hash),
                    ("documents", document_count.to_string()),
                    ("duration_ms", started.elapsed().as_millis().to_string()),
                ],
            );
            Ok(session)
        }
        Err(error) => {
            crate::diagnostics::record_error(
                "account-import",
                Some(&format!("{:?}", error.code)),
                &error.message,
                &[
                    ("documents", document_count.to_string()),
                    ("duration_ms", started.elapsed().as_millis().to_string()),
                ],
            );
            Err(error)
        }
    }
}
#[cfg(test)]
mod tests;
