use super::credentials::{
    credential_invalid_state_error as credential_error, credential_secret_ref,
    StoredCodexCredentials,
};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::LocalAccountRecord,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use zenith_relay_core::{
    account_candidate_health,
    accounts::{
        AccountAuthMode, AccountAuthState, AccountHealthState, AccountIdentity, AccountRecord,
    },
    quota::{QuotaSnapshot, Subscription, SubscriptionInput},
    CandidateHealth, CandidateQuota, WireApi,
};

pub const CODEX_SOURCE_ID: &str = "openai_codex";
pub const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

pub fn new_account_record(
    credentials: &StoredCodexCredentials,
    auth_mode: AccountAuthMode,
    models: Vec<String>,
    priority: i32,
    now_ms: u64,
) -> Result<LocalAccountRecord> {
    let provider_account_id = credentials.provider_account_id().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT credentials do not contain an account id",
        )
    })?;
    let identity_hash = identity_hash(
        provider_account_id,
        credentials.provider_user_id(),
        credentials.email(),
    );
    let secret_fingerprint = hash(
        credentials
            .agent_identity()
            .map(|agent| agent.private_key())
            .or_else(|| credentials.refresh_token())
            .unwrap_or_else(|| credentials.access_token())
            .as_bytes(),
    );
    let identity = AccountIdentity::from_hashed_parts(
        CODEX_SOURCE_ID,
        "chatgpt.com/backend-api/codex",
        &identity_hash,
        &secret_fingerprint,
        "default",
        None,
    )
    .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    let snapshot = credentials.snapshot();
    let label = snapshot
        .identity
        .clone()
        .unwrap_or_else(|| "ChatGPT account".to_string());
    let auth_state = if credentials.is_agent_identity() || credentials.refresh_token().is_some() {
        AccountAuthState::Active
    } else {
        AccountAuthState::DegradedAccessOnly
    };
    let mut subscription = Subscription::normalize(SubscriptionInput {
        plan_type: snapshot.plan_type,
        active_until_ms: None,
        forbidden: false,
        observed_at_ms: now_ms,
    });
    subscription.updated_at_ms = None;
    let mut record = LocalAccountRecord {
        account: AccountRecord {
            id: credentials.local_account_id().to_string(),
            label,
            identity,
            auth_mode,
            auth_state,
            health: AccountHealthState::Healthy,
            source_id: CODEX_SOURCE_ID.to_string(),
            secret_refs: vec![
                credential_secret_ref(credentials.local_account_id()).map_err(credential_error)?
            ],
            subscription,
            quota: QuotaSnapshot::default(),
            token_generation: credentials.generation(),
            token_updated_at_ms: Some(credentials.issued_at_ms()),
            tags: BTreeSet::new(),
            enabled: true,
            in_pool: false,
            draining: false,
            created_at_ms: now_ms,
            last_used_at_ms: None,
            last_error_code: None,
        },
        purchase_cost_micro_usd: None,
        remote_location: None,
        wire_api: WireApi::Responses,
        models,
        discovered_models: None,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority,
        weight: 1,
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
    };
    record.normalize();
    Ok(record)
}

pub fn identity_hash(
    provider_account_id: &str,
    provider_user_id: Option<&str>,
    email: Option<&str>,
) -> String {
    let account = provider_account_id.trim().to_ascii_lowercase();
    let user = provider_user_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let email = email
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let value = match (email, user) {
        (Some(email), _) => format!("account:{account}\0email:{email}"),
        (None, Some(user)) => format!("account:{account}\0user:{user}"),
        (None, None) => format!("account:{account}"),
    };
    hash(value.as_bytes())
}

pub fn candidate_health(account: &AccountRecord) -> CandidateHealth {
    account_candidate_health(
        account.auth_state,
        account.health,
        account.subscription.status,
        account.last_error_code.as_deref(),
    )
}

pub fn candidate_quota_with_stale_after(
    quota: &QuotaSnapshot,
    now_ms: u64,
    stale_after_ms: u64,
) -> CandidateQuota {
    CandidateQuota::from_snapshot(quota, now_ms, stale_after_ms)
}

fn hash(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::{
        accounts::ReauthReason,
        quota::{QuotaWindow, QuotaWindowKind, SubscriptionStatus},
    };

    fn credentials() -> StoredCodexCredentials {
        StoredCodexCredentials::new(
            "account_local",
            "access-secret".into(),
            Some("refresh-secret".into()),
            Some("id-secret".into()),
            Some(60_000),
            1,
            2,
            Some("private@example.test".into()),
            Some("provider-account".into()),
            None,
            None,
            Some("plus".into()),
            false,
        )
        .unwrap()
    }

    #[test]
    fn account_record_contains_only_local_ids_and_hashes() {
        let record = new_account_record(
            &credentials(),
            AccountAuthMode::OAuth,
            vec!["gpt-test".into()],
            10,
            1,
        )
        .unwrap();
        let serialized = serde_json::to_string(&record).unwrap();
        assert_eq!(record.account.id, "account_local");
        assert_eq!(record.priority, 10);
        assert_eq!(record.account.subscription.updated_at_ms, None);
        assert!(!serialized.contains("provider-account"));
        assert!(!serialized.contains("access-secret"));
        assert!(!serialized.contains("refresh-secret"));
        assert!(serialized.contains("p***@e***.test"));
    }

    #[test]
    fn unavailable_account_can_be_saved_without_discovered_models() {
        let mut record =
            new_account_record(&credentials(), AccountAuthMode::OAuth, Vec::new(), 0, 1).unwrap();
        record.account.health = AccountHealthState::Unhealthy;
        record.account.last_error_code = Some("models_unauthorized".into());

        assert!(record.models.is_empty());
        assert_eq!(
            candidate_health(&record.account),
            CandidateHealth::Unhealthy
        );
    }

    #[test]
    fn team_members_have_distinct_account_identities() {
        let first = identity_hash("shared-team", Some("shared-user"), Some("one@example.test"));
        let second = identity_hash("shared-team", Some("shared-user"), Some("two@example.test"));
        assert_ne!(first, second);
        assert!(!first.contains("shared-team"));
    }

    #[test]
    fn auth_and_subscription_states_are_hard_filters() {
        let mut record = new_account_record(
            &credentials(),
            AccountAuthMode::OAuth,
            vec!["gpt-test".into()],
            0,
            1,
        )
        .unwrap();
        record.account.auth_state = AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant);
        assert_eq!(
            candidate_health(&record.account),
            CandidateHealth::ReauthRequired
        );
        record.account.auth_state = AccountAuthState::Active;
        record.account.subscription.status = SubscriptionStatus::Forbidden;
        assert_eq!(candidate_health(&record.account), CandidateHealth::Blocked);
        record.account.subscription.status = SubscriptionStatus::Active;
        record.account.auth_state = AccountAuthState::DegradedAccessOnly;
        record.account.last_error_code = Some("upstream_unauthorized".into());
        assert_eq!(candidate_health(&record.account), CandidateHealth::Healthy);
    }

    #[test]
    fn quota_uses_most_constrained_window_and_becomes_stale() {
        let quota = QuotaSnapshot {
            primary: Some(QuotaWindow {
                kind: QuotaWindowKind::Primary,
                provider_cycle_id: None,
                window_start_ms: None,
                available_basis_points: Some(8_000),
                explicitly_full: None,
                reset_at_ms: None,
                window_minutes: None,
                observed_at_ms: 1,
                full_transition_fingerprint: None,
                exhaustion_transition_fingerprint: None,
            }),
            secondary: Some(QuotaWindow {
                kind: QuotaWindowKind::Secondary,
                provider_cycle_id: None,
                window_start_ms: None,
                available_basis_points: Some(2_000),
                explicitly_full: None,
                reset_at_ms: None,
                window_minutes: None,
                observed_at_ms: 1,
                full_transition_fingerprint: None,
                exhaustion_transition_fingerprint: None,
            }),
            supplemental: Vec::new(),
            limit_reached: false,
            reset_credits_available: None,
            direct_balance_micro_usd: None,
            updated_at_ms: Some(1),
            error: None,
        };
        assert_eq!(
            candidate_quota_with_stale_after(&quota, 2, zenith_relay_core::QUOTA_STALE_AFTER_MS),
            CandidateQuota::Available(2_000)
        );
        assert_eq!(
            candidate_quota_with_stale_after(
                &quota,
                zenith_relay_core::QUOTA_STALE_AFTER_MS + 2,
                zenith_relay_core::QUOTA_STALE_AFTER_MS,
            ),
            CandidateQuota::Stale
        );
    }
}
