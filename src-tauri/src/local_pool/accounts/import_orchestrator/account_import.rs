use super::{
    account_auth_mode, build_import_credential_material, credential_item_error,
    ensure_account_import_item, merge_existing_account, model_item_error, persist_imported_account,
    proxy_item_error, validate_label, ImportItemError, ImportRowContext, ItemResult,
};
use crate::local_pool::accounts::credentials::CredentialStore;
use crate::local_pool::accounts::proxy::{common_proxy_config, ensure_account_proxy};
use crate::local_pool::accounts::quota_refresh::AccountQuotaOutcome;
use crate::local_pool::accounts::{records, NativeSecretBackend};
use crate::local_pool::commands::current_time_ms;
use crate::local_pool::models::LocalAccountRecord;
use crate::local_pool::state::DesktopState;
use zenith_relay_core::accounts::ParsedImportItem;
use zenith_relay_core::error_codes;
use zenith_relay_core::providers::chatgpt::CodexModelsClient;

mod quota_probe;
mod reconcile;
mod stage;

pub(super) use quota_probe::hinted_import_proxy;
use quota_probe::probe_import_quota;
use reconcile::reconcile_imported_credentials;
pub(crate) use stage::stage_returned_remote_account;

pub(super) struct AccountImportOptions<'a> {
    pub(super) add_to_pool: bool,
    pub(super) discover_models: bool,
    pub(super) probe_quota: bool,
    pub(super) configured_models: &'a [String],
}

pub(super) async fn import_account_item(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    import_item: ParsedImportItem,
    context: &ImportRowContext,
    options: AccountImportOptions<'_>,
    account_check_endpoint: &url::Url,
) -> ItemResult<(LocalAccountRecord, AccountQuotaOutcome)> {
    let AccountImportOptions {
        add_to_pool,
        discover_models,
        probe_quota,
        configured_models,
    } = options;
    ensure_account_import_item(&import_item)?;
    let item_hash = crate::diagnostics::hash_identifier(&import_item.item_id);
    crate::diagnostics::breadcrumb(
        "account-import",
        "item_processing_started",
        &[("item", item_hash.clone())],
    );
    let issued_at_ms = current_time_ms();
    let item_label = import_item.label.clone();
    let imported_tags = import_item.tags.clone();
    let item_priority = import_item.priority;
    let settings = state
        .store()
        .map_err(|_| {
            ImportItemError::new(
                error_codes::ACCOUNT_STORE_FAILED,
                "account store is unavailable",
            )
        })?
        .gateway()
        .clone();
    let common_proxy = common_proxy_config(&settings).map_err(proxy_item_error)?;
    let hinted_proxy = hinted_import_proxy(state, credential_store, &settings, &import_item)?;
    let import_proxy = hinted_proxy.as_ref().or(common_proxy.as_ref());
    ensure_account_proxy(&settings, import_proxy).map_err(proxy_item_error)?;
    let material = build_import_credential_material(
        import_item,
        issued_at_ms,
        context.plan.as_deref(),
        context.subscription_active_until_ms,
        import_proxy,
        settings.quota_request_timeout_seconds,
        account_check_endpoint,
    )
    .await?;
    crate::diagnostics::breadcrumb(
        "account-import",
        "credentials_resolved",
        &[("item", item_hash.clone())],
    );
    let reconciled =
        reconcile_imported_credentials(state, credential_store, &settings, issued_at_ms, material)?;
    let reconcile::ReconciledImportCredentials {
        credentials,
        old_credential,
        existing_account,
        preserved_refresh_token,
        subscription_active_until_ms,
        proxy,
        provider_account_id,
        identity_is_registered,
    } = reconciled;
    let discovered_models = if discover_models && identity_is_registered {
        let client = CodexModelsClient::new_with_proxy(proxy.as_ref()).map_err(model_item_error)?;
        let client_version =
            zenith_relay_core::providers::chatgpt::configured_codex_client_version();
        let models = client
            .discover_authorized(
                credentials
                    .authorization(issued_at_ms)
                    .map_err(credential_item_error)?,
                &provider_account_id,
                &client_version,
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
        .map(|existing_account| existing_account.priority)
        .or(item_priority);
    let mut account = records::new_account_record(
        &credentials,
        auth_mode,
        models,
        priority.unwrap_or_default(),
        issued_at_ms,
    )
    .map_err(|_| {
        ImportItemError::new(
            error_codes::INVALID_ACCOUNT,
            "imported account record is invalid",
        )
    })?;
    account.discovered_models = discovered_models.or_else(|| {
        existing_account
            .as_ref()
            .and_then(|existing_account| existing_account.discovered_models.clone())
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
    // Existing account metadata is authoritative. Imported tags initialize a
    // new account only, so re-importing a credential cannot erase local tags
    // selected for automation or operator notes.
    if existing_account.is_none() {
        account.account.tags = imported_tags;
    }
    validate_label(&account.account.label).map_err(|_| {
        ImportItemError::new(
            error_codes::INVALID_LABEL,
            "imported account label is invalid",
        )
    })?;
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
    crate::diagnostics::breadcrumb(
        "account-import",
        "persist_ready",
        &[("item", item_hash.clone())],
    );
    persist_imported_account(
        state,
        credential_store,
        &credentials,
        old_credential.as_ref(),
        account.clone(),
    )
    .await?;
    crate::diagnostics::breadcrumb(
        "account-import",
        "item_processing_completed",
        &[("item", item_hash)],
    );
    Ok((account, quota))
}
