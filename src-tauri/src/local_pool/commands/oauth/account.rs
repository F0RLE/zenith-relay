use crate::local_pool::{
    accounts::{
        credentials::{credential_local_error as credential_error, CredentialStore},
        records, NativeSecretBackend,
    },
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
    models::LocalAccountRecord,
};

use zenith_relay_core::{
    accounts::{AccountAuthState, AccountHealthState},
    providers::chatgpt::ModelDiscoveryFailure,
    quota::SubscriptionStatus,
};

#[derive(Clone, Copy)]
pub(super) struct InitialModelIssue {
    code: &'static str,
    retryable: bool,
    auth_error: bool,
    blocked: bool,
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
    next: &mut LocalAccountRecord,
    current: &LocalAccountRecord,
) {
    next.account.id = current.account.id.clone();
    next.account.label = current.account.label.clone();
    next.account.tags = current.account.tags.clone();
    next.account.enabled = current.account.enabled;
    next.account.in_pool = current.account.in_pool;
    next.account.draining = current.account.draining;
    next.account.created_at_ms = current.account.created_at_ms;
    next.account.last_used_at_ms = current.account.last_used_at_ms;
    next.account.quota = current.account.quota.clone();
    next.purchase_cost_micro_usd = current.purchase_cost_micro_usd;
    next.remote_location = current.remote_location.clone();
    let fresh_models = std::mem::take(&mut next.models);
    // Some([]) is not a live catalog and must not hide models already shown.
    let fresh_discovered_models = next
        .discovered_models
        .take()
        .filter(|models| !models.is_empty());
    next.models = current.models.clone();
    next.discovered_models = fresh_discovered_models.or_else(|| {
        if fresh_models.is_empty() {
            current.discovered_models.clone()
        } else {
            Some(fresh_models)
        }
    });
    if next.account.subscription.plan_type.is_none() {
        next.account.subscription.plan_type = current.account.subscription.plan_type.clone();
    }
    if next.account.subscription.active_until_ms.is_none()
        && current.account.subscription.status != SubscriptionStatus::Expired
    {
        next.account.subscription.active_until_ms = current.account.subscription.active_until_ms;
    }
    next.allowed_models = current.allowed_models.clone();
    next.excluded_models = current.excluded_models.clone();
    next.priority = current.priority;
    next.weight = current.weight;
}

pub(super) fn initial_model_issue(error: &ModelDiscoveryFailure) -> InitialModelIssue {
    InitialModelIssue {
        code: error.code.management_code(),
        retryable: error.retryable,
        auth_error: error.code.is_authentication_failure(),
        blocked: error.code.blocks_account(),
    }
}

pub(super) fn apply_initial_model_issue(record: &mut LocalAccountRecord, issue: InitialModelIssue) {
    record.account.last_error_code = Some(issue.code.to_string());
    if issue.auth_error {
        record.account.auth_state = AccountAuthState::Error;
        record.account.health = AccountHealthState::Unhealthy;
    } else if issue.blocked {
        record.account.health = AccountHealthState::Blocked;
    } else if !matches!(
        record.account.health,
        AccountHealthState::Blocked | AccountHealthState::Unhealthy
    ) {
        record.account.health = if issue.retryable {
            AccountHealthState::Degraded
        } else {
            AccountHealthState::Unhealthy
        };
    }
}
