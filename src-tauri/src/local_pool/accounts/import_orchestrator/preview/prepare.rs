use super::*;

mod account;
use account::{prepare_account_preview_item, AccountPreviewInput, AccountPreviewStep};

pub(in crate::local_pool::accounts::import_orchestrator) async fn prepare_import_preview(
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
    let item_count = session.items.len();
    let mut prepared_values = Vec::with_capacity(item_count);
    let mut prepared_identity_keys = HashSet::with_capacity(item_count);
    let mut credentials_changed = false;
    let now_ms = current_time_ms();
    let settings = state.store()?.gateway().clone();
    let common_proxy = common_proxy_config(&settings)?;
    let session_hash = crate::diagnostics::hash_identifier(&session.session_id);
    crate::diagnostics::breadcrumb(
        "account-import",
        "prepare_preview_started",
        &[
            ("session", session_hash.clone()),
            ("candidates", item_count.to_string()),
            ("probe_quota", probe_quota.to_string()),
        ],
    );
    for (index, (item, row)) in session
        .items
        .into_iter()
        .zip(preview.rows.iter_mut().filter(|row| row.selectable))
        .enumerate()
    {
        let item_hash = crate::diagnostics::hash_identifier(&item.item_id);
        crate::diagnostics::breadcrumb(
            "account-import",
            "prepare_item_started",
            &[
                ("session", session_hash.clone()),
                ("item", item_hash.clone()),
                ("index", index.to_string()),
                ("total", item_count.to_string()),
                ("auth_mode", row.auth_mode.as_str().to_string()),
            ],
        );
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
            crate::diagnostics::breadcrumb(
                "account-import",
                "prepare_item_completed",
                &[
                    ("session", session_hash.clone()),
                    ("item", item_hash),
                    ("index", index.to_string()),
                ],
            );
            continue;
        }

        match prepare_account_preview_item(AccountPreviewInput {
            state,
            credentials,
            settings: &settings,
            common_proxy: &common_proxy,
            probe_quota,
            now_ms,
            session_hash: &session_hash,
            item_hash: &item_hash,
            index,
            row,
            item,
            original,
            prepared_identity_keys: &mut prepared_identity_keys,
        })
        .await?
        {
            AccountPreviewStep::Skipped {
                credentials_changed: changed,
            } => credentials_changed |= changed,
            AccountPreviewStep::Prepared {
                value,
                credentials_changed: changed,
            } => {
                credentials_changed |= changed;
                prepared_values.push(value);
            }
        }
    }
    // Preparation can reject an otherwise parseable item after contacting the
    // provider. In that case the prepared snapshot must retain only the
    // credentials that still have a selectable row; reusing the original
    // document would make the snapshot's item count disagree with the preview.
    let content = (credentials_changed || prepared_values.len() != item_count)
        .then(|| serde_json::to_string(&prepared_values))
        .transpose()
        .map_err(|_| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "failed to encode prepared import credentials",
            )
        })?;
    crate::diagnostics::record_operation(
        "account-import",
        "prepare_preview_completed",
        &[
            ("session", session_hash),
            (
                "selectable",
                preview
                    .rows
                    .iter()
                    .filter(|row| row.selectable)
                    .count()
                    .to_string(),
            ),
        ],
    );
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

struct PreviewItemFailure<'a> {
    code: ImportIssueCode,
    diagnostic_code: &'a str,
    message: &'a str,
    session_hash: &'a str,
    item_hash: &'a str,
    index: usize,
}

fn reject_preview_item(
    row: &mut zenith_relay_core::accounts::ImportPreviewRow,
    failure: PreviewItemFailure<'_>,
) {
    row.status = ImportPreviewStatus::Invalid;
    row.selectable = false;
    row.default_selected = false;
    row.error = Some(ImportIssue {
        code: failure.code,
        message: failure.message.to_string(),
    });
    crate::diagnostics::record_error(
        "account-import",
        Some(failure.diagnostic_code),
        failure.message,
        &[
            ("session", failure.session_hash.to_string()),
            ("item", failure.item_hash.to_string()),
            ("index", failure.index.to_string()),
        ],
    );
}
