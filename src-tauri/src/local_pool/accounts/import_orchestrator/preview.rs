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
                message: error.message.clone(),
            });
            crate::diagnostics::record_error(
                "account-import",
                Some(error_codes::PROXY_UNAVAILABLE),
                &error.message,
                &[
                    ("session", session_hash.clone()),
                    ("item", item_hash.clone()),
                    ("index", index.to_string()),
                ],
            );
            continue;
        }
        credentials_changed |=
            item.secrets().access_token().is_none() && item.secrets().refresh_token().is_some();
        crate::diagnostics::breadcrumb(
            "account-import",
            "identity_lookup_started",
            &[
                ("session", session_hash.clone()),
                ("item", item_hash.clone()),
                ("index", index.to_string()),
            ],
        );
        let material = match build_import_credential_material(
            item,
            now_ms,
            plan_hint.as_deref(),
            row.subscription_expires_at
                .as_deref()
                .and_then(parse_subscription_timestamp_ms),
            import_proxy,
            settings.quota_request_timeout_seconds,
            state.account_check_url(),
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
                    message: error.message.clone(),
                });
                crate::diagnostics::record_error(
                    "account-import",
                    Some(&error.code),
                    &error.message,
                    &[
                        ("session", session_hash.clone()),
                        ("item", item_hash.clone()),
                        ("index", index.to_string()),
                    ],
                );
                continue;
            }
        };
        crate::diagnostics::breadcrumb(
            "account-import",
            "identity_lookup_completed",
            &[
                ("session", session_hash.clone()),
                ("item", item_hash.clone()),
                ("index", index.to_string()),
            ],
        );
        let Some(provider_account_id) = material.provider_account_id.as_deref() else {
            row.status = ImportPreviewStatus::Invalid;
            row.selectable = false;
            row.default_selected = false;
            row.error = Some(ImportIssue {
                code: ImportIssueCode::InvalidCredentials,
                message: "ChatGPT account identity is missing".into(),
            });
            crate::diagnostics::record_error(
                "account-import",
                Some(error_codes::PROVIDER_ACCOUNT_ID_MISSING),
                "ChatGPT account identity is missing",
                &[
                    ("session", session_hash.clone()),
                    ("item", item_hash.clone()),
                    ("index", index.to_string()),
                ],
            );
            continue;
        };
        // Some account exports contain the same credentials more than once,
        // while their cached account_id/email fields differ. The initial
        // parser quite reasonably treats those rows as distinct, but the
        // authenticated lookup gives them the same canonical identity. If we
        // serialize both rows, the session re-parser collapses the duplicate
        // and rejects the prepared snapshot with a misleading count mismatch.
        // Keep the first canonical identity and mark later rows explicitly so
        // the preview and prepared credential set stay in lockstep.
        let provider_identity = provider_identity_key(
            provider_account_id,
            material.provider_user_id.as_deref(),
            material.email.as_deref(),
        );
        if !prepared_identity_keys.insert(provider_identity) {
            row.status = ImportPreviewStatus::Invalid;
            row.selectable = false;
            row.default_selected = false;
            row.error = Some(ImportIssue {
                code: ImportIssueCode::DuplicateItem,
                message: "duplicate authenticated account identity".into(),
            });
            crate::diagnostics::record_error(
                "account-import",
                Some(error_codes::DUPLICATE_ITEM),
                "duplicate authenticated account identity",
                &[
                    ("session", session_hash.clone()),
                    ("item", item_hash.clone()),
                    ("index", index.to_string()),
                ],
            );
            continue;
        }
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
        crate::diagnostics::breadcrumb(
            "account-import",
            "prepare_item_completed",
            &[
                ("session", session_hash.clone()),
                ("item", item_hash),
                ("index", index.to_string()),
            ],
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{header::AUTHORIZATION, HeaderMap};
    use axum::{routing::get, Json, Router};
    use std::fs;
    use tokio::net::TcpListener;
    use url::Url;
    use uuid::Uuid;

    #[tokio::test]
    async fn preparation_persists_only_credentials_for_selectable_rows() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-import-preview-{}",
            Uuid::new_v4().simple()
        ));
        let mut state = DesktopState::open(root.clone()).unwrap();
        let (endpoint, server) = spawn_account_check_server().await;
        state.set_account_check_url_for_test(endpoint);

        let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
        let session = sessions
            .start(
                r#"[
                    {"auth_mode":"oauth","account_id":"synthetic-provider-ok","access_token":"synthetic-access-ok","refresh_token":"synthetic-refresh-ok"},
                    {"auth_mode":"oauth","account_id":"synthetic-provider-rejected","access_token":"synthetic-access-rejected","refresh_token":"synthetic-refresh-rejected"}
                ]"#,
                None,
                &[],
            )
            .unwrap();
        let session_id = session.session_id.clone();
        let credentials = CredentialStore::from_backend(NativeSecretBackend);

        let (prepared_content, preview) =
            prepare_import_preview(&state, &credentials, session, false)
                .await
                .unwrap();
        let prepared_content = prepared_content.expect("filtered credentials must be persisted");
        let prepared_values =
            zenith_relay_core::accounts::parse_import(&prepared_content, None, &[]).unwrap();

        assert_eq!(prepared_values.items.len(), 1);
        assert_eq!(preview.rows.len(), 2);
        assert_eq!(preview.rows.iter().filter(|row| row.selectable).count(), 1);
        assert!(preview.rows[0].selectable);
        assert!(!preview.rows[1].selectable);

        let prepared = sessions
            .prepare(&session_id, Some(&prepared_content), preview.clone(), &[])
            .unwrap();
        assert_eq!(prepared.items.len(), 1);
        assert_eq!(prepared.preview, preview);

        sessions.cancel(&session_id).unwrap();
        server.abort();
        drop(state);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn preparation_filters_duplicates_found_by_authenticated_identity() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-import-duplicate-preview-{}",
            Uuid::new_v4().simple()
        ));
        let mut state = DesktopState::open(root.clone()).unwrap();
        let (endpoint, server) = spawn_duplicate_account_check_server().await;
        state.set_account_check_url_for_test(endpoint);

        let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
        let session = sessions
            .start(
                r#"[
                    {"auth_mode":"oauth","access_token":"synthetic-access-one"},
                    {"auth_mode":"oauth","access_token":"synthetic-access-two"}
                ]"#,
                None,
                &[],
            )
            .unwrap();
        let session_id = session.session_id.clone();
        let credentials = CredentialStore::from_backend(NativeSecretBackend);

        let (prepared_content, preview) =
            prepare_import_preview(&state, &credentials, session, false)
                .await
                .unwrap();
        let prepared_content = prepared_content.expect("duplicate credentials must be filtered");
        let prepared_values =
            zenith_relay_core::accounts::parse_import(&prepared_content, None, &[]).unwrap();

        assert_eq!(prepared_values.items.len(), 1);
        assert_eq!(preview.rows.len(), 2);
        assert_eq!(preview.rows.iter().filter(|row| row.selectable).count(), 1);
        assert_eq!(
            preview.rows[1].error.as_ref().map(|error| error.code),
            Some(ImportIssueCode::DuplicateItem)
        );

        let prepared = sessions
            .prepare(&session_id, Some(&prepared_content), preview, &[])
            .unwrap();
        assert_eq!(prepared.items.len(), 1);

        sessions.cancel(&session_id).unwrap();
        server.abort();
        drop(state);
        fs::remove_dir_all(root).unwrap();
    }

    async fn spawn_account_check_server() -> (Url, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/accounts/check",
            get(|headers: HeaderMap| async move {
                let valid = headers
                    .get(AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value == "Bearer synthetic-access-ok");
                let payload = if valid {
                    serde_json::json!({
                        "accounts": [{"account": {"id": "synthetic-provider-ok"}}]
                    })
                } else {
                    serde_json::json!({"accounts": []})
                };
                Json(payload)
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            Url::parse(&format!("http://{address}/accounts/check")).unwrap(),
            server,
        )
    }

    async fn spawn_duplicate_account_check_server() -> (Url, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/accounts/check",
            get(|headers: HeaderMap| async move {
                let valid = headers
                    .get(AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| {
                        matches!(
                            value,
                            "Bearer synthetic-access-one" | "Bearer synthetic-access-two"
                        )
                    });
                let payload = if valid {
                    serde_json::json!({
                        "accounts": [{"account": {"id": "synthetic-shared-account"}}]
                    })
                } else {
                    serde_json::json!({"accounts": []})
                };
                Json(payload)
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            Url::parse(&format!("http://{address}/accounts/check")).unwrap(),
            server,
        )
    }
}
