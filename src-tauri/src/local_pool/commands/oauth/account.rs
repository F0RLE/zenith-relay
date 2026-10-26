use crate::local_pool::{
    accounts::{
        NativeSecretBackend,
        credentials::{CredentialStore, credential_local_error as credential_error},
        records,
    },
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
    models::LocalAccountRecord,
};

use zenith_relay_core::{
    accounts::apply_model_discovery_failure, providers::chatgpt::ModelDiscoveryFailure,
    quota::SubscriptionStatus,
};

#[derive(Clone, Copy)]
pub(super) struct InitialModelIssue {
    code: &'static str,
    retryable: bool,
}

pub(super) fn find_existing_account<'a>(
    accounts: &'a [LocalAccountRecord],
    credentials: &CredentialStore<NativeSecretBackend>,
    identity_hash: &str,
) -> LocalResult<Option<&'a LocalAccountRecord>> {
    records::find_codex_account(
        accounts,
        identity_hash,
        |account| {
            records::codex_credentials_match(credentials, account, identity_hash)
                .map_err(credential_error)
        },
        || {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "multiple local accounts have the same ChatGPT identity",
            )
        },
    )
}

pub(super) fn preserve_existing_settings(
    account_to_update: &mut LocalAccountRecord,
    existing_account: &LocalAccountRecord,
) {
    account_to_update.account.id = existing_account.account.id.clone();
    account_to_update.account.label = existing_account.account.label.clone();
    account_to_update.account.tags = existing_account.account.tags.clone();
    account_to_update.account.enabled = existing_account.account.enabled;
    account_to_update.account.in_pool = existing_account.account.in_pool;
    account_to_update.image_pool_enabled =
        existing_account.image_pool_enabled && existing_account.account.in_pool;
    account_to_update.account.draining = existing_account.account.draining;
    account_to_update.account.created_at_ms = existing_account.account.created_at_ms;
    account_to_update.account.last_used_at_ms = existing_account.account.last_used_at_ms;
    account_to_update.account.quota = existing_account.account.quota.clone();
    account_to_update.purchase_cost_micro_usd = existing_account.purchase_cost_micro_usd;
    account_to_update.recovery = existing_account.recovery.clone();
    let fresh_models = std::mem::take(&mut account_to_update.models);
    // Some([]) is not a live catalog and must not hide models already shown.
    let fresh_discovered_models = account_to_update
        .discovered_models
        .take()
        .filter(|models| !models.is_empty());
    account_to_update.models = existing_account.models.clone();
    account_to_update.discovered_models = fresh_discovered_models.or_else(|| {
        if fresh_models.is_empty() {
            existing_account.discovered_models.clone()
        } else {
            Some(fresh_models)
        }
    });
    if account_to_update.account.subscription.plan_type.is_none() {
        account_to_update.account.subscription.plan_type =
            existing_account.account.subscription.plan_type.clone();
    }
    if account_to_update
        .account
        .subscription
        .active_until_ms
        .is_none()
        && existing_account.account.subscription.status != SubscriptionStatus::Expired
    {
        account_to_update.account.subscription.active_until_ms =
            existing_account.account.subscription.active_until_ms;
    }
    account_to_update.allowed_models = existing_account.allowed_models.clone();
    account_to_update.excluded_models = existing_account.excluded_models.clone();
    account_to_update.priority = existing_account.priority;
    account_to_update.weight = existing_account.weight;
}

pub(super) fn initial_model_issue(error: &ModelDiscoveryFailure) -> InitialModelIssue {
    InitialModelIssue {
        code: error.code.management_code(),
        retryable: error.retryable,
    }
}

pub(super) fn apply_initial_model_issue(
    account_record: &mut LocalAccountRecord,
    issue: InitialModelIssue,
) {
    let account = &mut account_record.account;
    apply_model_discovery_failure(
        &mut account.auth_state,
        &mut account.health,
        &mut account.last_error_code,
        issue.code,
        issue.retryable,
    );
}
