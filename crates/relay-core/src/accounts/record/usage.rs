use super::auth::{explicit_account_disable, AccountAuthState, AccountHealthState};
use crate::error_codes;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountAccessState {
    Refreshable,
    AccessOnly,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountUsageState {
    pub auth_state: AccountAuthState,
    pub health: AccountHealthState,
    pub last_error_code: Option<String>,
    pub last_used_at_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountUsageObservation<'a> {
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<&'a str>,
    pub affects_account: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountUsageUpdate {
    pub state: AccountUsageState,
    pub reset_runtime_failures: bool,
    pub refresh_quota: bool,
}

pub fn reduce_account_usage(
    mut state: AccountUsageState,
    observation: AccountUsageObservation<'_>,
    observed_at_ms: u64,
    access_state: Option<AccountAccessState>,
    successful_auth_state: Option<AccountAuthState>,
) -> AccountUsageUpdate {
    if observation.success {
        state.last_used_at_ms = Some(observed_at_ms);
        state.health = AccountHealthState::Healthy;
        state.last_error_code = None;
        if matches!(
            state.auth_state,
            AccountAuthState::Error | AccountAuthState::RequiresReauth(_)
        ) {
            if let Some(auth_state) = successful_auth_state {
                state.auth_state = auth_state;
            }
        }
        return AccountUsageUpdate {
            state,
            reset_runtime_failures: true,
            refresh_quota: false,
        };
    }

    if !observation.affects_account {
        return AccountUsageUpdate {
            state,
            reset_runtime_failures: false,
            refresh_quota: false,
        };
    }

    let explicit_state =
        state.auth_state.requires_fresh_login() || state.health == AccountHealthState::Blocked;
    let failure_category = observation
        .error_category
        .filter(|category| *category != error_codes::UPSTREAM_STATUS);
    // A quota rejection asks for a fresh window read. It is not an account
    // outage: the windows themselves decide rotation, and a stale copy of this
    // code must not keep a recoverable account looking broken.
    if failure_category == Some(error_codes::UPSTREAM_QUOTA_EXHAUSTED) {
        let owns_quota_error =
            state.last_error_code.as_deref() == Some(error_codes::UPSTREAM_QUOTA_EXHAUSTED);
        let cleared = if owns_quota_error && state.health == AccountHealthState::Degraded {
            state.health = AccountHealthState::Healthy;
            state.last_error_code = None;
            true
        } else if owns_quota_error && state.health == AccountHealthState::Healthy {
            state.last_error_code = None;
            true
        } else {
            false
        };
        return AccountUsageUpdate {
            state,
            reset_runtime_failures: cleared,
            refresh_quota: true,
        };
    }
    match observation.http_status {
        401 => match access_state {
            Some(AccountAccessState::Refreshable) => {
                if !explicit_state {
                    state.health = AccountHealthState::Degraded;
                }
                state.last_error_code = Some(
                    failure_category
                        .unwrap_or(error_codes::UPSTREAM_UNAUTHORIZED)
                        .to_string(),
                );
            }
            Some(AccountAccessState::AccessOnly) => {
                if !state.auth_state.requires_fresh_login() {
                    state.auth_state = AccountAuthState::Error;
                }
                state.health = AccountHealthState::Unhealthy;
                state.last_error_code = Some(
                    failure_category
                        .unwrap_or(error_codes::UPSTREAM_UNAUTHORIZED)
                        .to_string(),
                );
            }
            Some(AccountAccessState::Failed) | None if !explicit_state => {
                state.auth_state = AccountAuthState::Error;
                state.health = AccountHealthState::Unhealthy;
                state.last_error_code = Some("credential_access_expiry_failed".to_string());
            }
            Some(AccountAccessState::Failed) | None => {}
        },
        403 => {
            let category = failure_category.unwrap_or(error_codes::UPSTREAM_FORBIDDEN);
            if explicit_account_disable(category) {
                if !state.auth_state.requires_fresh_login() {
                    state.health = AccountHealthState::Blocked;
                }
                state.last_error_code = Some(category.to_string());
            } else if recoverable_forbidden(category) {
                if !explicit_state {
                    state.health = AccountHealthState::Degraded;
                }
                state.last_error_code = Some(category.to_string());
            } else if !preserves_stronger_account_state(&state) {
                state.health = AccountHealthState::Degraded;
                state.last_error_code = Some(category.to_string());
            }
        }
        429 => {
            if !explicit_state {
                state.health = AccountHealthState::Degraded;
            }
            state.last_error_code = Some(
                failure_category
                    .unwrap_or(error_codes::UPSTREAM_RATE_LIMITED)
                    .to_string(),
            );
        }
        _ => {
            if !explicit_state {
                state.health = AccountHealthState::Degraded;
            }
            state.last_error_code = Some(
                observation
                    .error_category
                    .unwrap_or(error_codes::UPSTREAM_FAILURE)
                    .to_string(),
            );
        }
    }

    AccountUsageUpdate {
        state,
        reset_runtime_failures: true,
        refresh_quota: observation.http_status == 429
            || (observation.http_status == 401
                && access_state == Some(AccountAccessState::Refreshable)),
    }
}

fn recoverable_forbidden(category: &str) -> bool {
    matches!(
        category,
        error_codes::UPSTREAM_QUOTA_EXHAUSTED
            | error_codes::UPSTREAM_USAGE_NOT_INCLUDED
            | error_codes::UPSTREAM_REGION_UNSUPPORTED
            | error_codes::UPSTREAM_EDGE_CHALLENGE
            | error_codes::UPSTREAM_ACCOUNT_VERIFICATION_REQUIRED
    )
}

fn preserves_stronger_account_state(state: &AccountUsageState) -> bool {
    state.auth_state.requires_fresh_login()
        || (state.health == AccountHealthState::Blocked
            && state.last_error_code.as_deref() != Some(error_codes::UPSTREAM_FORBIDDEN))
}
