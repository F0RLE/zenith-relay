use super::super::super::ImportedCredentialMaterial;
use super::super::*;
use super::{mark_preview_quota_failed, reject_preview_item, PreviewItemFailure};

pub(super) enum AccountPreviewStep {
    Skipped {
        credentials_changed: bool,
    },
    Prepared {
        prepared_json: serde_json::Value,
        credentials_changed: bool,
    },
}

pub(super) struct AccountPreviewInput<'a> {
    pub(super) state: &'a DesktopState,
    pub(super) credentials: &'a CredentialStore<NativeSecretBackend>,
    pub(super) settings: &'a crate::local_pool::models::GatewaySettings,
    pub(super) common_proxy: &'a Option<zenith_relay_core::ProxyConfig>,
    pub(super) probe_quota: bool,
    pub(super) now_ms: u64,
    pub(super) session_hash: &'a str,
    pub(super) item_hash: &'a str,
    pub(super) index: usize,
    pub(super) row: &'a mut zenith_relay_core::accounts::ImportPreviewRow,
    pub(super) import_item: zenith_relay_core::accounts::ParsedImportItem,
    pub(super) original_json: serde_json::Value,
    pub(super) prepared_identity_keys: &'a mut HashSet<String>,
}

pub(super) async fn prepare_account_preview_item(
    input: AccountPreviewInput<'_>,
) -> CommandResult<AccountPreviewStep> {
    let AccountPreviewInput {
        state,
        credentials,
        settings,
        common_proxy,
        probe_quota,
        now_ms,
        session_hash,
        item_hash,
        index,
        row,
        import_item,
        original_json,
        prepared_identity_keys,
    } = input;
    let mut credentials_changed = false;
    let plan_hint = row.plan.clone();
    let hinted_proxy = hinted_import_proxy(state, credentials, settings, &import_item)
        .map_err(import_item_command_error)?;
    let import_proxy = hinted_proxy.as_ref().or(common_proxy.as_ref());
    if let Err(error) = ensure_account_proxy(settings, import_proxy) {
        reject_preview_item(
            row,
            PreviewItemFailure {
                code: ImportIssueCode::RefreshExchangeFailed,
                diagnostic_code: error_codes::PROXY_UNAVAILABLE,
                message: &error.message,
                session_hash,
                item_hash,
                index,
            },
        );
        return Ok(AccountPreviewStep::Skipped {
            credentials_changed,
        });
    }
    credentials_changed |= import_item.secrets().access_token().is_none()
        && import_item.secrets().refresh_token().is_some();
    crate::diagnostics::breadcrumb(
        "account-import",
        "identity_lookup_started",
        &[
            ("session", session_hash.to_string()),
            ("item", item_hash.to_string()),
            ("index", index.to_string()),
        ],
    );
    let material = match build_import_credential_material(
        import_item,
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
            reject_preview_item(
                row,
                PreviewItemFailure {
                    code: ImportIssueCode::RefreshExchangeFailed,
                    diagnostic_code: &error.code,
                    message: &error.message,
                    session_hash,
                    item_hash,
                    index,
                },
            );
            return Ok(AccountPreviewStep::Skipped {
                credentials_changed,
            });
        }
    };
    crate::diagnostics::breadcrumb(
        "account-import",
        "identity_lookup_completed",
        &[
            ("session", session_hash.to_string()),
            ("item", item_hash.to_string()),
            ("index", index.to_string()),
        ],
    );
    let Some(provider_account_id) = material.provider_account_id.as_deref() else {
        reject_preview_item(
            row,
            PreviewItemFailure {
                code: ImportIssueCode::InvalidCredentials,
                diagnostic_code: error_codes::PROVIDER_ACCOUNT_ID_MISSING,
                message: "ChatGPT account identity is missing",
                session_hash,
                item_hash,
                index,
            },
        );
        return Ok(AccountPreviewStep::Skipped {
            credentials_changed,
        });
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
        material.oauth_client_kind,
    );
    if !prepared_identity_keys.insert(provider_identity) {
        reject_preview_item(
            row,
            PreviewItemFailure {
                code: ImportIssueCode::DuplicateItem,
                diagnostic_code: error_codes::DUPLICATE_ITEM,
                message: "duplicate authenticated account identity",
                session_hash,
                item_hash,
                index,
            },
        );
        return Ok(AccountPreviewStep::Skipped {
            credentials_changed,
        });
    }
    row.identity = masked_account_identity(provider_account_id);
    row.oauth_client_kind = Some(material.oauth_client_kind);
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
        material.oauth_client_kind,
    )
    .map_err(import_item_command_error)?;
    if existing_account.is_some() {
        row.existing = true;
        row.status = ImportPreviewStatus::Existing;
    }
    if probe_quota {
        probe_account_preview_quota(
            AccountPreviewQuota {
                credentials,
                settings,
                common_proxy,
                existing_account: existing_account.as_ref(),
                material: &material,
                provider_account_id,
                now_ms,
            },
            row,
        )
        .await?;
    }
    let prepared_json = parsed_item_json_with_material(original_json, &material);
    crate::diagnostics::breadcrumb(
        "account-import",
        "prepare_item_completed",
        &[
            ("session", session_hash.to_string()),
            ("item", item_hash.to_string()),
            ("index", index.to_string()),
        ],
    );
    Ok(AccountPreviewStep::Prepared {
        prepared_json,
        credentials_changed,
    })
}

struct AccountPreviewQuota<'a> {
    credentials: &'a CredentialStore<NativeSecretBackend>,
    settings: &'a crate::local_pool::models::GatewaySettings,
    common_proxy: &'a Option<zenith_relay_core::ProxyConfig>,
    existing_account: Option<&'a crate::local_pool::models::LocalAccountRecord>,
    material: &'a ImportedCredentialMaterial,
    provider_account_id: &'a str,
    now_ms: u64,
}

async fn probe_account_preview_quota(
    probe: AccountPreviewQuota<'_>,
    row: &mut zenith_relay_core::accounts::ImportPreviewRow,
) -> CommandResult<()> {
    let AccountPreviewQuota {
        credentials,
        settings,
        common_proxy,
        existing_account,
        material,
        provider_account_id,
        now_ms,
    } = probe;
    let proxy = match existing_account {
        Some(account) => credentials
            .load(&account.account.id)
            .map_err(credential_local_error)?
            .map(|stored| effective_proxy_config(settings, &stored))
            .transpose()?
            .flatten()
            .or_else(|| (*common_proxy).clone()),
        None => (*common_proxy).clone(),
    };
    let request_timeout = Duration::from_secs(settings.quota_request_timeout_seconds);
    let quota = CodexQuotaClient::new_with_proxy_and_timeout(proxy.as_ref(), request_timeout)
        .map_err(|_| LocalPoolError::new(ErrorCode::InvalidState, "quota client is unavailable"))?;
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
        Ok(Ok(quota_data)) => match quota_data.quota.normalize(&Default::default()) {
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
    Ok(())
}
