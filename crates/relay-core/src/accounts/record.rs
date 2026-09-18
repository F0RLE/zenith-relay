use crate::error_codes;
use crate::quota::{QuotaSnapshot, Subscription};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

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
            if matches!(
                category,
                error_codes::UPSTREAM_QUOTA_EXHAUSTED
                    | error_codes::UPSTREAM_USAGE_NOT_INCLUDED
                    | error_codes::UPSTREAM_REGION_UNSUPPORTED
                    | error_codes::UPSTREAM_EDGE_CHALLENGE
            ) {
                if !explicit_state {
                    state.health = AccountHealthState::Degraded;
                }
            } else {
                state.health = AccountHealthState::Blocked;
            }
            state.last_error_code = Some(category.to_string());
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
                && access_state == Some(AccountAccessState::Refreshable))
            || (observation.http_status == 403
                && failure_category == Some(error_codes::UPSTREAM_QUOTA_EXHAUSTED)),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountIdentity {
    pub stable_index: String,
    pub identity_hash: String,
    pub organization_hash: Option<String>,
}

impl AccountIdentity {
    pub fn from_hashed_parts(
        source_kind: &str,
        base_url_scope: &str,
        identity_hash: &str,
        secret_fingerprint_hash: &str,
        namespace: &str,
        organization_hash: Option<&str>,
    ) -> Result<Self, &'static str> {
        let source_kind = required(source_kind)?;
        let base_url_scope = required(base_url_scope)?;
        let identity_hash = required(identity_hash)?;
        let secret_fingerprint_hash = required(secret_fingerprint_hash)?;
        let namespace = required(namespace)?;
        let organization_hash = organization_hash
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let stable_index = hex::encode(Sha256::digest(
            format!(
                "{}\0{}\0{}\0{}\0{}",
                source_kind.to_ascii_lowercase(),
                base_url_scope.to_ascii_lowercase(),
                identity_hash.to_ascii_lowercase(),
                secret_fingerprint_hash.to_ascii_lowercase(),
                namespace.to_ascii_lowercase(),
            )
            .as_bytes(),
        ));
        Ok(Self {
            stable_index,
            identity_hash: identity_hash.to_ascii_lowercase(),
            organization_hash,
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRecord {
    pub id: String,
    pub label: String,
    pub identity: AccountIdentity,
    pub auth_mode: AccountAuthMode,
    pub auth_state: AccountAuthState,
    pub health: AccountHealthState,
    pub source_id: String,
    pub secret_refs: Vec<String>,
    pub subscription: Subscription,
    pub quota: QuotaSnapshot,
    pub token_generation: u64,
    pub token_updated_at_ms: Option<u64>,
    pub tags: BTreeSet<String>,
    pub enabled: bool,
    #[serde(default)]
    pub in_pool: bool,
    pub draining: bool,
    pub created_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
    pub last_error_code: Option<String>,
}

impl AccountRecord {
    pub fn is_automatic_quota_monitoring_eligible(&self) -> bool {
        automatic_quota_monitoring_eligible(self.enabled, self.auth_state)
    }

    pub fn is_wake_eligible(&self) -> bool {
        self.enabled
            && self.in_pool
            && !self.draining
            && self.auth_state == AccountAuthState::Active
            && self.health == AccountHealthState::Healthy
    }
}

fn required(value: &str) -> Result<&str, &'static str> {
    let value = value.trim();
    if value.is_empty() {
        Err("stable identity parts must not be empty")
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_identity_uses_only_hashed_inputs() {
        let first = AccountIdentity::from_hashed_parts(
            "openai",
            "api.example.test/v1",
            "email-hash",
            "secret-hash",
            "default",
            Some("org-hash"),
        )
        .unwrap();
        let second = AccountIdentity::from_hashed_parts(
            "OPENAI",
            "API.EXAMPLE.TEST/V1",
            "EMAIL-HASH",
            "SECRET-HASH",
            "DEFAULT",
            Some("ORG-HASH"),
        )
        .unwrap();

        assert_eq!(first, second);
        assert!(!format!("{first:?}").contains('@'));
        assert_eq!(first.stable_index.len(), 64);
    }

    #[test]
    fn exact_provider_failures_have_one_terminal_account_state() {
        assert_eq!(
            provider_account_failure("token_invalidated"),
            Some(ProviderAccountFailure::Authentication)
        );
        assert_eq!(
            provider_account_failure("deactivated_workspace"),
            Some(ProviderAccountFailure::Blocked)
        );
        assert_eq!(provider_account_failure("quota_unauthorized"), None);
    }

    #[test]
    fn automatic_quota_monitoring_is_independent_of_pool_and_health() {
        assert!(automatic_quota_monitoring_eligible(
            true,
            AccountAuthState::Active,
        ));
        assert!(automatic_quota_monitoring_eligible(
            true,
            AccountAuthState::DegradedAccessOnly,
        ));
        assert!(automatic_quota_monitoring_eligible(
            true,
            AccountAuthState::Error,
        ));
        assert!(!automatic_quota_monitoring_eligible(
            false,
            AccountAuthState::Active,
        ));
        assert!(!automatic_quota_monitoring_eligible(
            true,
            AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant),
        ));
        assert!(automatic_quota_monitoring_eligible(
            true,
            AccountAuthState::RequiresReauth(ReauthReason::ReusedRefreshToken),
        ));
    }

    #[test]
    fn refresh_token_reused_is_not_a_provider_authentication_failure() {
        assert_eq!(provider_account_failure("refresh_token_reused"), None);
        assert_eq!(
            provider_account_failure("refresh_token_expired"),
            Some(ProviderAccountFailure::Authentication)
        );
    }

    #[test]
    fn model_discovery_state_transitions_keep_user_reauthentication_intact() {
        let mut auth_state = AccountAuthState::Active;
        let mut health = AccountHealthState::Healthy;
        let mut last_error_code = None;

        apply_model_discovery_failure(
            &mut auth_state,
            &mut health,
            &mut last_error_code,
            "models_transport",
            true,
        );
        assert_eq!(auth_state, AccountAuthState::Active);
        assert_eq!(health, AccountHealthState::Degraded);
        assert_eq!(last_error_code.as_deref(), Some("models_transport"));
        assert!(recover_model_discovery_state(
            &mut auth_state,
            &mut health,
            &mut last_error_code,
        ));
        assert_eq!(health, AccountHealthState::Healthy);
        assert!(last_error_code.is_none());

        auth_state = AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant);
        health = AccountHealthState::Unhealthy;
        last_error_code = Some("invalid_grant".into());
        apply_model_discovery_failure(
            &mut auth_state,
            &mut health,
            &mut last_error_code,
            "models_unauthorized",
            false,
        );
        assert!(auth_state.requires_fresh_login());
        assert_eq!(health, AccountHealthState::Unhealthy);
        assert!(!recover_model_discovery_state(
            &mut auth_state,
            &mut health,
            &mut last_error_code,
        ));
        assert_eq!(health, AccountHealthState::Unhealthy);
        assert_eq!(last_error_code.as_deref(), Some("invalid_grant"));

        auth_state = AccountAuthState::Active;
        health = AccountHealthState::Healthy;
        last_error_code = None;
        apply_model_discovery_failure(
            &mut auth_state,
            &mut health,
            &mut last_error_code,
            "models_forbidden",
            false,
        );
        assert_eq!(health, AccountHealthState::Degraded);

        // A previously confirmed account block must not be cleared by a
        // separate model-discovery permission failure.
        health = AccountHealthState::Blocked;
        apply_model_discovery_failure(
            &mut auth_state,
            &mut health,
            &mut last_error_code,
            "models_forbidden",
            false,
        );
        assert_eq!(health, AccountHealthState::Blocked);
    }

    fn usage_state() -> AccountUsageState {
        AccountUsageState {
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            last_error_code: None,
            last_used_at_ms: None,
        }
    }

    #[test]
    fn model_discovery_preserves_independent_account_failures() {
        use crate::{account_candidate_health, quota::SubscriptionStatus};

        for (initial_auth, initial_health, initial_error) in [
            (
                AccountAuthState::Active,
                AccountHealthState::Blocked,
                "workspace_disabled",
            ),
            (
                AccountAuthState::Active,
                AccountHealthState::Unhealthy,
                "token_invalidated",
            ),
            (
                AccountAuthState::Error,
                AccountHealthState::Unhealthy,
                "upstream_unauthorized",
            ),
            (
                AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant),
                AccountHealthState::Unhealthy,
                "invalid_grant",
            ),
            (
                AccountAuthState::Active,
                AccountHealthState::Degraded,
                "checkpoint",
            ),
            (
                AccountAuthState::Active,
                AccountHealthState::Degraded,
                "captcha",
            ),
            (
                AccountAuthState::Active,
                AccountHealthState::Degraded,
                "upstream_account_verification_required",
            ),
        ] {
            for (code, retryable) in [
                ("models_transport", true),
                ("models_forbidden", false),
                ("models_unauthorized", false),
            ] {
                let mut auth_state = initial_auth;
                let mut health = initial_health;
                let mut last_error_code = Some(initial_error.to_string());
                apply_model_discovery_failure(
                    &mut auth_state,
                    &mut health,
                    &mut last_error_code,
                    code,
                    retryable,
                );
                assert_eq!(auth_state, initial_auth, "{initial_error}: {code}");
                assert_eq!(health, initial_health, "{initial_error}: {code}");
                assert_eq!(last_error_code.as_deref(), Some(initial_error));
                assert!(!recover_model_discovery_state(
                    &mut auth_state,
                    &mut health,
                    &mut last_error_code,
                ));
                assert!(!account_candidate_health(
                    auth_state,
                    health,
                    SubscriptionStatus::Active,
                    last_error_code.as_deref(),
                )
                .is_eligible());
            }
        }
    }

    #[test]
    fn model_discovery_transient_failure_preserves_terminal_catalog_failure() {
        let mut auth_state = AccountAuthState::Active;
        let mut health = AccountHealthState::Healthy;
        let mut last_error_code = None;
        apply_model_discovery_failure(
            &mut auth_state,
            &mut health,
            &mut last_error_code,
            "models_unauthorized",
            false,
        );
        for (code, retryable) in [("models_transport", true), ("models_forbidden", false)] {
            apply_model_discovery_failure(
                &mut auth_state,
                &mut health,
                &mut last_error_code,
                code,
                retryable,
            );
            assert_eq!(auth_state, AccountAuthState::Error);
            assert_eq!(health, AccountHealthState::Unhealthy);
            assert_eq!(last_error_code.as_deref(), Some("models_unauthorized"));
        }
        assert!(recover_model_discovery_state(
            &mut auth_state,
            &mut health,
            &mut last_error_code,
        ));
        assert_eq!(auth_state, AccountAuthState::Active);
        assert_eq!(health, AccountHealthState::Healthy);
        assert!(last_error_code.is_none());
    }

    #[test]
    fn model_discovery_success_does_not_clear_persisted_block() {
        let mut auth_state = AccountAuthState::Active;
        let mut health = AccountHealthState::Blocked;
        let mut last_error_code = Some("models_forbidden".to_string());
        assert!(!recover_model_discovery_state(
            &mut auth_state,
            &mut health,
            &mut last_error_code,
        ));
        assert_eq!(health, AccountHealthState::Blocked);
        assert_eq!(last_error_code.as_deref(), Some("models_forbidden"));
    }

    #[test]
    fn account_usage_reducer_ignores_request_errors_and_restores_success() {
        let neutral = reduce_account_usage(
            usage_state(),
            AccountUsageObservation {
                success: false,
                http_status: 400,
                error_category: Some("upstream_invalid_request"),
                affects_account: false,
            },
            10,
            None,
            None,
        );
        assert_eq!(neutral.state, usage_state());
        assert!(!neutral.reset_runtime_failures);

        let success = reduce_account_usage(
            AccountUsageState {
                auth_state: AccountAuthState::Error,
                health: AccountHealthState::Unhealthy,
                last_error_code: Some("upstream_unauthorized".into()),
                last_used_at_ms: None,
            },
            AccountUsageObservation {
                success: true,
                http_status: 200,
                error_category: None,
                affects_account: false,
            },
            20,
            None,
            Some(AccountAuthState::Active),
        );
        assert_eq!(
            success.state,
            AccountUsageState {
                auth_state: AccountAuthState::Active,
                health: AccountHealthState::Healthy,
                last_error_code: None,
                last_used_at_ms: Some(20),
            }
        );
        assert!(success.reset_runtime_failures);
    }

    #[test]
    fn account_usage_reducer_distinguishes_refreshable_and_access_only_401() {
        let observation = AccountUsageObservation {
            success: false,
            http_status: 401,
            error_category: Some("token_invalidated"),
            affects_account: true,
        };
        let refreshable = reduce_account_usage(
            usage_state(),
            observation,
            10,
            Some(AccountAccessState::Refreshable),
            None,
        );
        assert_eq!(refreshable.state.health, AccountHealthState::Degraded);
        assert_eq!(refreshable.state.auth_state, AccountAuthState::Active);
        assert!(refreshable.refresh_quota);

        let access_only = reduce_account_usage(
            usage_state(),
            observation,
            10,
            Some(AccountAccessState::AccessOnly),
            None,
        );
        assert_eq!(access_only.state.health, AccountHealthState::Unhealthy);
        assert_eq!(access_only.state.auth_state, AccountAuthState::Error);
        assert!(!access_only.refresh_quota);
    }

    #[test]
    fn account_usage_reducer_keeps_quota_and_entitlement_failures_recoverable() {
        for (status, category, refresh_quota) in [
            (403, "upstream_quota_exhausted", true),
            (403, "upstream_usage_not_included", false),
            (429, "upstream_rate_limited", true),
        ] {
            let update = reduce_account_usage(
                usage_state(),
                AccountUsageObservation {
                    success: false,
                    http_status: status,
                    error_category: Some(category),
                    affects_account: true,
                },
                10,
                None,
                None,
            );
            assert_eq!(update.state.health, AccountHealthState::Degraded);
            assert_eq!(update.state.last_error_code.as_deref(), Some(category));
            assert_eq!(update.refresh_quota, refresh_quota);
        }

        let blocked = reduce_account_usage(
            usage_state(),
            AccountUsageObservation {
                success: false,
                http_status: 403,
                error_category: Some("upstream_forbidden"),
                affects_account: true,
            },
            10,
            None,
            None,
        );
        assert_eq!(blocked.state.health, AccountHealthState::Blocked);
    }
}
