use crate::error_codes;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountAuthMode {
    OAuth,
    ApiKey,
    ImportedToken,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReauthReason {
    InvalidGrant,
    ReusedRefreshToken,
    ExpiredRefreshToken,
    InvalidatedRefreshToken,
    AccessTokenExpired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderAccountFailure {
    Authentication,
    Blocked,
}

pub fn provider_account_failure(code: &str) -> Option<ProviderAccountFailure> {
    match code {
        error_codes::INVALID_GRANT
        | error_codes::INVALID_REFRESH_TOKEN
        | error_codes::REFRESH_TOKEN_EXPIRED
        | error_codes::REFRESH_TOKEN_INVALIDATED
        | error_codes::TOKEN_INVALIDATED
        | "token_revoked" => Some(ProviderAccountFailure::Authentication),
        "account_deactivated"
        | "account_disabled"
        | "deactivated_workspace"
        | "organization_deactivated"
        | "organization_disabled"
        | "project_deactivated"
        | "workspace_disabled"
        | "workspace_expired"
        | "workspace_terminated" => Some(ProviderAccountFailure::Blocked),
        _ => None,
    }
}

pub(super) fn explicit_account_disable(code: &str) -> bool {
    code == error_codes::UPSTREAM_ACCOUNT_DISABLED
        || matches!(
            provider_account_failure(code),
            Some(ProviderAccountFailure::Blocked)
        )
}

/// A generic HTTP 403 was previously stored as a permanent block. That code
/// does not prove the account was disabled, so restore routing and drop it.
pub fn clear_false_upstream_block(
    health: &mut AccountHealthState,
    last_error_code: &mut Option<String>,
) -> bool {
    if *health == AccountHealthState::Blocked
        && last_error_code.as_deref() == Some(error_codes::UPSTREAM_FORBIDDEN)
    {
        *health = AccountHealthState::Healthy;
        *last_error_code = None;
        true
    } else {
        false
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "reason")]
pub enum AccountAuthState {
    #[default]
    Unknown,
    Active,
    DegradedAccessOnly,
    Refreshing,
    Error,
    RequiresReauth(ReauthReason),
}

impl AccountAuthState {
    /// Kept for backward-compatible account records. A reused refresh token
    /// indicates a concurrent rotation, not a credential that needs login.
    pub fn requires_fresh_login(self) -> bool {
        matches!(
            self,
            Self::RequiresReauth(reason) if !matches!(reason, ReauthReason::ReusedRefreshToken)
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountHealthState {
    #[default]
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
    Blocked,
}

pub fn automatic_quota_monitoring_eligible(enabled: bool, auth_state: AccountAuthState) -> bool {
    enabled && !auth_state.requires_fresh_login()
}

/// Applies the common terminal state for a failed account model discovery.
/// Callers retain ownership of their catalog storage and only share the account
/// status transition.
pub fn apply_model_discovery_failure(
    auth_state: &mut AccountAuthState,
    health: &mut AccountHealthState,
    last_error_code: &mut Option<String>,
    code: &str,
    retryable: bool,
) {
    if auth_state.requires_fresh_login() && *health != AccountHealthState::Blocked {
        *health = AccountHealthState::Unhealthy;
    }
    let discovery_owned_error = last_error_code
        .as_deref()
        .is_some_and(|code| code.starts_with("models_"));
    let terminal_state = auth_state.requires_fresh_login()
        || *auth_state == AccountAuthState::Error
        || matches!(
            *health,
            AccountHealthState::Blocked | AccountHealthState::Unhealthy
        )
        || matches!(
            last_error_code.as_deref(),
            Some("checkpoint" | "captcha" | error_codes::UPSTREAM_ACCOUNT_VERIFICATION_REQUIRED)
        );
    // Catalog availability cannot disprove an independent account failure.
    // A softer discovery failure cannot clear a terminal discovery failure either.
    if terminal_state
        && (!discovery_owned_error
            || *health == AccountHealthState::Blocked
            || retryable
            || code == error_codes::MODELS_FORBIDDEN)
    {
        return;
    }
    *last_error_code = Some(code.to_string());
    match code {
        error_codes::MODELS_UNAUTHORIZED
        | error_codes::MODELS_INVALID_ACCESS_TOKEN
        | error_codes::MODELS_INVALID_ACCOUNT_ID => {
            // A user-actionable reauthentication state must survive a later
            // model probe while the last good catalog remains available.
            if !auth_state.requires_fresh_login() {
                *auth_state = AccountAuthState::Error;
            }
            *health = AccountHealthState::Unhealthy;
        }
        error_codes::MODELS_FORBIDDEN => {
            // A model catalog endpoint can be forbidden while the account's
            // normal inference and quota endpoints remain usable. Keep this
            // scoped to discovery so a catalog permission issue does not
            // remove an otherwise working account from the pool. Preserve a
            // stronger, independently observed account state.
            if !matches!(
                *health,
                AccountHealthState::Blocked | AccountHealthState::Unhealthy
            ) {
                *health = AccountHealthState::Degraded;
            }
        }
        _ if retryable => *health = AccountHealthState::Degraded,
        _ => *health = AccountHealthState::Unhealthy,
    }
}

/// Clears a stale model-discovery error after a successful catalog refresh.
pub fn recover_model_discovery_state(
    auth_state: &mut AccountAuthState,
    health: &mut AccountHealthState,
    last_error_code: &mut Option<String>,
) -> bool {
    let recovered = last_error_code
        .as_deref()
        .is_some_and(|code| code.starts_with("models_"));
    if !recovered || *health == AccountHealthState::Blocked {
        return false;
    }

    *last_error_code = None;
    if !auth_state.requires_fresh_login() {
        if *auth_state == AccountAuthState::Error {
            *auth_state = AccountAuthState::Active;
        }
        *health = AccountHealthState::Healthy;
    }
    true
}
