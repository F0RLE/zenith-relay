use super::{CredentialStore, NativeSecretBackend};
use crate::local_pool::{accounts::import_session::SecretBackend, models::LocalAccountRecord};
use zenith_relay_core::{
    UsageEvent,
    accounts::{
        AccountAccessState, AccountAuthState, AccountUsageObservation, AccountUsageState,
        reduce_account_usage,
    },
    error_codes,
};

pub(super) struct AccountUsageGuard<'a> {
    _account: super::super::accounts::authority::ProcessAccountGuard,
    _revision: Option<zenith_relay_core::accounts::TokenDispatchRevisionGuard<'a>>,
}

pub(super) fn current_account_usage<'a, B: SecretBackend>(
    credentials: &CredentialStore<B>,
    locks: &super::super::accounts::authority::ProcessAccountLocks,
    event: &'a UsageEvent,
) -> Option<AccountUsageGuard<'a>> {
    let account_id = event.account_id.as_deref()?;
    let account = locks.try_acquire(account_id).ok()??;
    let revision = match (
        event.account_token_generation,
        event.account_token_revision.as_ref(),
    ) {
        (Some(_), Some(revision)) => Some(revision.guard()?),
        (None, None) if credentials.load(account_id).ok()??.is_agent_identity() => None,
        _ => return None,
    };
    Some(AccountUsageGuard {
        _account: account,
        _revision: revision,
    })
}

pub(super) fn expire_account_access<B: SecretBackend>(
    credentials: &CredentialStore<B>,
    account_id: &str,
    expected_generation: u64,
    now_ms: u64,
) -> Option<AccountAccessState> {
    let Ok(Some(mut stored)) = credentials.load(account_id) else {
        return Some(AccountAccessState::Failed);
    };
    if stored.oauth_client_kind() == zenith_relay_core::providers::chatgpt::OAuthClientKind::Siwc
        || stored.generation() != expected_generation
    {
        return None;
    };
    let refreshable = stored.refresh_token().is_some();
    stored.expire_access_at(now_ms);
    if credentials.save(&stored).is_err() {
        Some(AccountAccessState::Failed)
    } else if refreshable {
        Some(AccountAccessState::Refreshable)
    } else {
        Some(AccountAccessState::AccessOnly)
    }
}

pub(super) fn persisted_auth_state(
    credentials: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
) -> Option<AccountAuthState> {
    credentials.load(account_id).ok().flatten().map(|stored| {
        if stored.refresh_token().is_some() {
            AccountAuthState::Active
        } else {
            AccountAuthState::DegradedAccessOnly
        }
    })
}

pub(in crate::local_pool) fn apply_account_usage_state(
    account: &mut LocalAccountRecord,
    event: &UsageEvent,
    observed_at_ms: u64,
    access_state: Option<AccountAccessState>,
    successful_auth_state: Option<AccountAuthState>,
    ignore_account_observation: bool,
) -> bool {
    if ignore_account_observation {
        return false;
    }
    if let Some(snapshot) = event.quota_snapshot.as_ref() {
        account.account.quota.merge_observation(snapshot);
    } else if event.affects_account_state()
        && event.error_category.as_deref() == Some(error_codes::UPSTREAM_QUOTA_EXHAUSTED)
    {
        account
            .account
            .quota
            .note_reported_window_exhaustion(observed_at_ms);
    }
    let update = reduce_account_usage(
        AccountUsageState {
            auth_state: account.account.auth_state,
            health: account.account.health,
            last_error_code: account.account.last_error_code.clone(),
            last_used_at_ms: account.account.last_used_at_ms,
        },
        AccountUsageObservation {
            success: event.success,
            http_status: event.http_status,
            error_category: event.error_category.as_deref(),
            affects_account: event.affects_account_state(),
        },
        observed_at_ms,
        access_state,
        successful_auth_state,
    );
    account.account.auth_state = update.state.auth_state;
    account.account.health = update.state.health;
    account.account.last_error_code = update.state.last_error_code;
    account.account.last_used_at_ms = update.state.last_used_at_ms;
    if update.reset_runtime_failures {
        account.cooldowns.clear();
        account.consecutive_failures = 0;
    }
    update.refresh_quota
}
