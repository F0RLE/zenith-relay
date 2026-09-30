use super::super::{
    apply_quota_outcome, credential_item_error, find_existing_account, proxy_item_error, ItemResult,
};
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::proxy::effective_proxy_config;
use crate::local_pool::accounts::quota_refresh::{
    AccountQuotaOutcome, QUOTA_COMMAND_TIMEOUT_OVERHEAD,
};
use crate::local_pool::accounts::quota_service::apply_quota_failure;
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::commands::current_time_ms;
use crate::local_pool::models::{GatewaySettings, LocalAccountRecord};
use crate::local_pool::state::DesktopState;
use std::time::Duration;
use zenith_relay_core::accounts::ParsedImportItem;
use zenith_relay_core::error_codes;
use zenith_relay_core::providers::chatgpt::{CodexQuotaClient, QuotaRefreshOutcome};
use zenith_relay_core::quota::QuotaRefreshFailure;
use zenith_relay_core::ProxyConfig;

pub(in crate::local_pool::accounts::import_orchestrator) fn hinted_import_proxy(
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

pub(super) async fn probe_import_quota(
    account: &mut LocalAccountRecord,
    credentials: &StoredCodexCredentials,
    proxy: Option<&ProxyConfig>,
    request_timeout_seconds: u64,
) -> AccountQuotaOutcome {
    let now_ms = current_time_ms();
    let Some(provider_account_id) = credentials.provider_account_id() else {
        let failure = QuotaRefreshFailure::new(error_codes::INVALID_CHATGPT_ACCOUNT_ID, false);
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
                    let failure =
                        QuotaRefreshFailure::new(error_codes::INVALID_ACCESS_TOKEN, false);
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
            failure: QuotaRefreshFailure::new(error_codes::QUOTA_TIMEOUT, true),
            subscription: account.account.subscription.clone(),
        },
    };
    apply_quota_outcome(account, outcome, now_ms)
}
