use super::{
    account_auth_mode, apply_quota_outcome, build_import_credential_material,
    credential_item_error, ensure_account_import_item, existing_identity_index,
    find_existing_account, merge_existing_account, model_item_error,
    parse_subscription_timestamp_ms, persist_imported_account, proxy_item_error, validate_label,
    ImportItemError, ImportRowContext, ItemResult,
};
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::proxy::{
    common_proxy_config, effective_proxy_config, ensure_account_proxy,
};
use crate::local_pool::accounts::quota_refresh::{
    AccountQuotaOutcome, QUOTA_COMMAND_TIMEOUT_OVERHEAD,
};
use crate::local_pool::accounts::quota_service::apply_quota_failure;
use crate::local_pool::accounts::{records, NativeSecretBackend};
use crate::local_pool::commands::current_time_ms;
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::{GatewaySettings, LocalAccountRecord};
use crate::local_pool::state::DesktopState;
use std::time::Duration;
use uuid::Uuid;
use zenith_relay_core::accounts::{parse_import, ParsedImportItem};
use zenith_relay_core::providers::chatgpt::{
    CodexModelsClient, CodexQuotaClient, QuotaRefreshOutcome,
};
use zenith_relay_core::quota::QuotaRefreshFailure;
use zenith_relay_core::ProxyConfig;

pub(crate) async fn stage_returned_remote_account(
    state: &DesktopState,
    local_account_id: &str,
    content: &str,
) -> LocalResult<LocalAccountRecord> {
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let existing = existing_identity_index(state, &credentials)?;
    let mut parsed = parse_import(content, None, &existing.keys().cloned().collect::<Vec<_>>())
        .map_err(LocalPoolError::invalid_state)?;
    if parsed.items.len() != 1 || parsed.preview.rows.len() != 1 {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote account export must contain exactly one account",
        ));
    }
    let row = parsed.preview.rows.remove(0);
    if !row.selectable {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote account export is not usable",
        ));
    }
    let existing_record = state
        .store()?
        .account(local_account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local account not found"))?;
    let row_context = ImportRowContext {
        label: row.label,
        auth_mode: row.auth_mode,
        selectable: row.selectable,
        plan: row.plan,
        subscription_active_until_ms: row
            .subscription_expires_at
            .as_deref()
            .and_then(parse_subscription_timestamp_ms),
    };
    let configured_models = existing_record.models.clone();
    let item = parsed.items.remove(0);
    let (account, _) = import_account_item(
        state,
        &credentials,
        item,
        &row_context,
        false,
        true,
        true,
        &configured_models,
    )
    .await
    .map_err(|error| {
        LocalPoolError::new(
            if error.code == "recovery_required" {
                ErrorCode::RecoveryRequired
            } else {
                ErrorCode::InvalidState
            },
            error.message,
        )
    })?;
    if account.account.id != local_account_id
        || account.remote_location != existing_record.remote_location
        || account.account.enabled
        || account.account.in_pool
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "returned credentials did not stage on the expected inactive local account",
        ));
    }
    Ok(account)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn import_account_item(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    item: ParsedImportItem,
    context: &ImportRowContext,
    add_to_pool: bool,
    discover_models: bool,
    probe_quota: bool,
    configured_models: &[String],
) -> ItemResult<(LocalAccountRecord, AccountQuotaOutcome)> {
    ensure_account_import_item(&item)?;
    let issued_at_ms = current_time_ms();
    let item_label = item.label.clone();
    let item_priority = item.priority;
    let settings = state
        .store()
        .map_err(|_| ImportItemError::new("account_store_failed", "account store is unavailable"))?
        .gateway()
        .clone();
    let common_proxy = common_proxy_config(&settings).map_err(proxy_item_error)?;
    let hinted_proxy = hinted_import_proxy(state, credential_store, &settings, &item)?;
    let import_proxy = hinted_proxy.as_ref().or(common_proxy.as_ref());
    ensure_account_proxy(&settings, import_proxy).map_err(proxy_item_error)?;
    let mut material = build_import_credential_material(
        item,
        issued_at_ms,
        context.plan.as_deref(),
        context.subscription_active_until_ms,
        import_proxy,
        settings.quota_request_timeout_seconds,
    )
    .await?;
    let provider_account_id = material.provider_account_id.as_deref().ok_or_else(|| {
        ImportItemError::new(
            "provider_account_id_missing",
            "ChatGPT account id is missing from imported credentials",
        )
    })?;
    let existing_account = find_existing_account(
        state,
        credential_store,
        provider_account_id,
        material.provider_user_id.as_deref(),
        material.email.as_deref(),
    )?;
    let local_account_id = existing_account
        .as_ref()
        .map(|account| account.account.id.clone())
        .unwrap_or_else(|| format!("account_{}", Uuid::new_v4().simple()));
    let old_credential = credential_store
        .load(&local_account_id)
        .map_err(credential_item_error)?;
    let preserved_refresh_token = material.refresh_token.is_none()
        && old_credential
            .as_ref()
            .and_then(StoredCodexCredentials::refresh_token)
            .is_some();
    if preserved_refresh_token {
        material.refresh_token = old_credential
            .as_ref()
            .and_then(StoredCodexCredentials::refresh_token)
            .map(str::to_string);
    }
    let generation = old_credential
        .as_ref()
        .map(StoredCodexCredentials::generation)
        .into_iter()
        .chain(
            existing_account
                .as_ref()
                .map(|account| account.account.token_generation),
        )
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let subscription_active_until_ms = material.subscription_active_until_ms;
    let mut credentials = material.into_stored(&local_account_id, issued_at_ms, generation)?;
    if let Some(proxy_url) = old_credential
        .as_ref()
        .and_then(StoredCodexCredentials::proxy_url)
    {
        credentials = credentials
            .with_proxy_url(Some(proxy_url.to_string()))
            .map_err(credential_item_error)?;
    }
    let proxy = effective_proxy_config(&settings, &credentials).map_err(proxy_item_error)?;
    let provider_account_id = credentials.provider_account_id().ok_or_else(|| {
        ImportItemError::new(
            "provider_account_id_missing",
            "ChatGPT account id is missing from imported credentials",
        )
    })?;
    let identity_is_registered = credentials
        .agent_identity()
        .is_none_or(|agent| agent.task_id().is_some());
    let discovered_models = if discover_models && identity_is_registered {
        let client = CodexModelsClient::new_with_proxy(proxy.as_ref()).map_err(model_item_error)?;
        let models = client
            .discover_authorized(
                credentials
                    .authorization(issued_at_ms)
                    .map_err(credential_item_error)?,
                provider_account_id,
                zenith_relay_core::providers::chatgpt::CODEX_MODELS_CLIENT_VERSION,
            )
            .await
            .map_err(model_item_error)?;
        Some(models)
    } else {
        None
    };
    let models = if let Some(existing) = &existing_account {
        existing.models.clone()
    } else if !configured_models.is_empty() {
        configured_models.to_vec()
    } else if let Some(discovered_models) = &discovered_models {
        discovered_models.clone()
    } else {
        Vec::new()
    };
    let auth_mode = if preserved_refresh_token {
        existing_account
            .as_ref()
            .map(|account| account.account.auth_mode)
            .unwrap_or(account_auth_mode(context.auth_mode)?)
    } else {
        account_auth_mode(context.auth_mode)?
    };
    let priority = existing_account
        .as_ref()
        .map(|value| value.priority)
        .or(item_priority);
    let mut account = records::new_account_record(
        &credentials,
        auth_mode,
        models,
        priority.unwrap_or_default(),
        issued_at_ms,
    )
    .map_err(|_| ImportItemError::new("invalid_account", "imported account record is invalid"))?;
    account.discovered_models = discovered_models.or_else(|| {
        existing_account
            .as_ref()
            .and_then(|value| value.discovered_models.clone())
    });
    merge_existing_account(&mut account, existing_account.as_ref());
    account.account.in_pool |= add_to_pool;
    if let Some(active_until_ms) = subscription_active_until_ms {
        account.account.subscription = zenith_relay_core::quota::Subscription::normalize(
            zenith_relay_core::quota::SubscriptionInput {
                plan_type: account.account.subscription.plan_type.clone(),
                active_until_ms: Some(active_until_ms),
                forbidden: false,
                observed_at_ms: issued_at_ms,
            },
        );
    }
    if existing_account.is_none() && !item_label.trim().is_empty() {
        account.account.label = item_label;
    }
    validate_label(&account.account.label)
        .map_err(|_| ImportItemError::new("invalid_label", "imported account label is invalid"))?;
    account.normalize();
    let quota = if probe_quota && identity_is_registered {
        probe_import_quota(
            &mut account,
            &credentials,
            proxy.as_ref(),
            settings.quota_request_timeout_seconds,
        )
        .await
    } else {
        AccountQuotaOutcome::Skipped
    };
    persist_imported_account(
        state,
        credential_store,
        &credentials,
        old_credential.as_ref(),
        account.clone(),
    )
    .await?;
    Ok((account, quota))
}

pub(super) fn hinted_import_proxy(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    settings: &GatewaySettings,
    item: &ParsedImportItem,
) -> ItemResult<Option<ProxyConfig>> {
    let Some(provider_account_id) = item.account_id.as_deref() else {
        return Ok(None);
    };
    let Some(existing) = find_existing_account(
        state,
        credential_store,
        provider_account_id,
        item.chatgpt_user_id.as_deref(),
        item.email(),
    )?
    else {
        return Ok(None);
    };
    let Some(credentials) = credential_store
        .load(&existing.account.id)
        .map_err(credential_item_error)?
    else {
        return Ok(None);
    };
    effective_proxy_config(settings, &credentials).map_err(proxy_item_error)
}

async fn probe_import_quota(
    account: &mut LocalAccountRecord,
    credentials: &StoredCodexCredentials,
    proxy: Option<&ProxyConfig>,
    request_timeout_seconds: u64,
) -> AccountQuotaOutcome {
    let now_ms = current_time_ms();
    let Some(provider_account_id) = credentials.provider_account_id() else {
        let failure = QuotaRefreshFailure::new("invalid_chatgpt_account_id", false);
        apply_quota_failure(account, &failure, now_ms);
        return AccountQuotaOutcome::Failed {
            code: failure.code,
            retryable: failure.retryable,
        };
    };
    let request_timeout = Duration::from_secs(request_timeout_seconds);
    let client = match CodexQuotaClient::new_with_proxy_and_timeout(proxy, request_timeout) {
        Ok(client) => client,
        Err(failure) => {
            apply_quota_failure(account, &failure, now_ms);
            return AccountQuotaOutcome::Failed {
                code: failure.code,
                retryable: failure.retryable,
            };
        }
    };
    let outcome = match tokio::time::timeout(
        request_timeout.saturating_add(QUOTA_COMMAND_TIMEOUT_OVERHEAD),
        client.refresh_quota_authorized(
            match credentials.authorization(now_ms) {
                Ok(authorization) => authorization,
                Err(_) => {
                    let failure = QuotaRefreshFailure::new("invalid_access_token", false);
                    apply_quota_failure(account, &failure, now_ms);
                    return AccountQuotaOutcome::Failed {
                        code: failure.code,
                        retryable: failure.retryable,
                    };
                }
            },
            provider_account_id,
            now_ms,
            &account.account.subscription,
            true,
        ),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => QuotaRefreshOutcome::Failed {
            failure: QuotaRefreshFailure::new("quota_timeout", true),
            subscription: account.account.subscription.clone(),
        },
    };
    apply_quota_outcome(account, outcome, now_ms)
}
