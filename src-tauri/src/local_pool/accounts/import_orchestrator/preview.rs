use super::{
    build_import_credential_material, credential_local_error, existing_identity_index,
    find_existing_account, find_existing_source, hinted_import_proxy, import_item_command_error,
    import_session_error, masked_account_identity, normalize_import_input,
    parse_subscription_timestamp_ms, parsed_item_value, parsed_item_value_from_material,
    timestamp_from_ms, ImportSessionResponse, StartAccountImportInput,
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
use std::time::Duration;
use zenith_relay_core::accounts::{
    ImportAuthMode, ImportIssue, ImportIssueCode, ImportPreview, ImportPreviewStatus,
    ImportQuotaStatus,
};
use zenith_relay_core::providers::chatgpt::CodexQuotaClient;

type CommandResult<T> = std::result::Result<T, CommandError>;

pub(super) async fn preview_account_import_documents(
    documents: Vec<String>,
    state: &DesktopState,
) -> CommandResult<ImportSessionResponse> {
    let _mutation = state.setup_guard().await;
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
        Ok(session) => Ok(session.into()),
        Err(error) => {
            let _ = sessions.cancel(&session_id);
            Err(error)
        }
    }
}

pub(super) async fn prepare_import_preview(
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
    session: ImportSession,
    probe_quota: bool,
) -> CommandResult<(Option<String>, ImportPreview)> {
    if session.items.len()
        != session
            .preview
            .rows
            .iter()
            .filter(|row| row.selectable)
            .count()
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "import preview does not match its credential items",
        )
        .into());
    }
    let mut preview = session.preview;
    let mut prepared_values = Vec::with_capacity(session.items.len());
    let mut credentials_changed = false;
    let now_ms = current_time_ms();
    let settings = state.store()?.gateway().clone();
    let common_proxy = common_proxy_config(&settings)?;
    for (item, row) in session
        .items
        .into_iter()
        .zip(preview.rows.iter_mut().filter(|row| row.selectable))
    {
        let original = parsed_item_value(&item, row.auth_mode);
        if row.auth_mode == ImportAuthMode::ApiKey {
            if let (Some(base_url), Some(api_key)) =
                (item.base_url.as_deref(), item.secrets().api_key())
            {
                if find_existing_source(state, base_url, api_key)
                    .map_err(import_item_command_error)?
                    .is_some()
                {
                    row.existing = true;
                    row.status = ImportPreviewStatus::Existing;
                }
            }
            prepared_values.push(original);
            continue;
        }

        let plan_hint = row.plan.clone();
        let hinted_proxy = hinted_import_proxy(state, credentials, &settings, &item)
            .map_err(import_item_command_error)?;
        let import_proxy = hinted_proxy.as_ref().or(common_proxy.as_ref());
        if let Err(error) = ensure_account_proxy(&settings, import_proxy) {
            row.status = ImportPreviewStatus::Invalid;
            row.selectable = false;
            row.default_selected = false;
            row.error = Some(ImportIssue {
                code: ImportIssueCode::RefreshExchangeFailed,
                message: error.message,
            });
            continue;
        }
        credentials_changed |=
            item.secrets().access_token().is_none() && item.secrets().refresh_token().is_some();
        let material = match build_import_credential_material(
            item,
            now_ms,
            plan_hint.as_deref(),
            row.subscription_expires_at
                .as_deref()
                .and_then(parse_subscription_timestamp_ms),
            import_proxy,
            settings.quota_request_timeout_seconds,
        )
        .await
        {
            Ok(material) => material,
            Err(error) => {
                row.status = ImportPreviewStatus::Invalid;
                row.selectable = false;
                row.default_selected = false;
                row.error = Some(ImportIssue {
                    code: ImportIssueCode::RefreshExchangeFailed,
                    message: error.message,
                });
                continue;
            }
        };
        let Some(provider_account_id) = material.provider_account_id.as_deref() else {
            row.status = ImportPreviewStatus::Invalid;
            row.selectable = false;
            row.default_selected = false;
            row.error = Some(ImportIssue {
                code: ImportIssueCode::InvalidCredentials,
                message: "ChatGPT account identity is missing".into(),
            });
            continue;
        };
        row.identity = masked_account_identity(provider_account_id);
        row.plan = material.plan_type.clone().or_else(|| row.plan.clone());
        row.expires_at = material.expires_at_ms.and_then(timestamp_from_ms);
        row.subscription_expires_at = material
            .subscription_active_until_ms
            .and_then(timestamp_from_ms)
            .or_else(|| row.subscription_expires_at.clone());
        let existing_account = find_existing_account(
            state,
            credentials,
            provider_account_id,
            material.provider_user_id.as_deref(),
            material.email.as_deref(),
        )
        .map_err(import_item_command_error)?;
        if existing_account.is_some() {
            row.existing = true;
            row.status = ImportPreviewStatus::Existing;
        }
        if probe_quota {
            let proxy = match existing_account {
                Some(ref account) => credentials
                    .load(&account.account.id)
                    .map_err(credential_local_error)?
                    .map(|stored| effective_proxy_config(&settings, &stored))
                    .transpose()?
                    .flatten()
                    .or_else(|| common_proxy.clone()),
                None => common_proxy.clone(),
            };
            let request_timeout = Duration::from_secs(settings.quota_request_timeout_seconds);
            let quota =
                CodexQuotaClient::new_with_proxy_and_timeout(proxy.as_ref(), request_timeout)
                    .map_err(|_| {
                        LocalPoolError::new(ErrorCode::InvalidState, "quota client is unavailable")
                    })?;
            match tokio::time::timeout(
                request_timeout.saturating_add(QUOTA_COMMAND_TIMEOUT_OVERHEAD),
                quota.refresh_data_with_subscription_authorization(
                    material
                        .authorization(now_ms)
                        .map_err(import_item_command_error)?,
                    material
                        .subscription_authorization()
                        .map_err(import_item_command_error)?,
                    provider_account_id,
                    now_ms,
                    &zenith_relay_core::quota::Subscription::normalize(
                        zenith_relay_core::quota::SubscriptionInput {
                            plan_type: material.plan_type.clone(),
                            active_until_ms: material.subscription_active_until_ms,
                            forbidden: false,
                            observed_at_ms: now_ms,
                        },
                    ),
                    true,
                ),
            )
            .await
            {
                Ok(Ok(data)) => match data.quota.normalize(&Default::default()) {
                    Ok((_, subscription)) => {
                        row.quota_status = ImportQuotaStatus::Success;
                        row.error = None;
                        if let Some(subscription) = subscription {
                            row.plan = subscription.plan_type.or_else(|| row.plan.clone());
                            row.subscription_expires_at = subscription
                                .active_until_ms
                                .and_then(timestamp_from_ms)
                                .or_else(|| row.subscription_expires_at.clone());
                        }
                    }
                    Err(_) => mark_preview_quota_failed(row, "quota response is invalid"),
                },
                Ok(Err(_)) => mark_preview_quota_failed(row, "quota probe failed"),
                Err(_) => mark_preview_quota_failed(row, "quota probe timed out"),
            }
        }
        prepared_values.push(parsed_item_value_from_material(original, &material));
    }
    let content = credentials_changed
        .then(|| serde_json::to_string(&prepared_values))
        .transpose()
        .map_err(|_| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "failed to encode prepared import credentials",
            )
        })?;
    Ok((content, preview))
}

fn mark_preview_quota_failed(
    row: &mut zenith_relay_core::accounts::ImportPreviewRow,
    message: &str,
) {
    row.quota_status = ImportQuotaStatus::Failed;
    row.status = ImportPreviewStatus::QuotaFailed;
    row.error = Some(ImportIssue {
        code: ImportIssueCode::QuotaProbeFailed,
        message: message.into(),
    });
}
