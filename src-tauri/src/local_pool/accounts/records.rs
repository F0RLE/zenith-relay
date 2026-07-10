use super::credentials::{credential_secret_ref, StoredCodexCredentials};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::LocalAccountRecord,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use zenith_relay_core::{
    accounts::{
        AccountAuthMode, AccountAuthState, AccountHealthState, AccountIdentity, AccountRecord,
    },
    quota::{QuotaSnapshot, Subscription, SubscriptionInput, SubscriptionStatus},
    CandidateHealth, CandidateQuota, WireApi,
};

pub const CODEX_SOURCE_ID: &str = "openai_codex";
pub const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const QUOTA_STALE_AFTER_MS: u64 = 15 * 60 * 1_000;

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
            "Codex credentials do not contain a ChatGPT account id",
        )
    })?;
    let identity_hash = hash(provider_account_id.trim().to_ascii_lowercase().as_bytes());
    let secret_fingerprint = hash(
        credentials
            .refresh_token()
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
        .unwrap_or_else(|| "Codex account".to_string());
    let auth_state = if credentials.refresh_token().is_some() {
        AccountAuthState::Active
    } else {
        AccountAuthState::DegradedAccessOnly
    };
    let subscription = Subscription::normalize(SubscriptionInput {
        plan_type: snapshot.plan_type,
        active_until_ms: None,
        forbidden: false,
        observed_at_ms: now_ms,
    });
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
            draining: false,
            created_at_ms: now_ms,
            last_used_at_ms: None,
            last_error_code: None,
        },
        wire_api: WireApi::Responses,
        models,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority,
        weight: 1,
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
    };
    record.normalize();
    if record.models.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "Codex account did not expose any supported models",
        ));
    }
    Ok(record)
}

pub fn candidate_health(account: &AccountRecord) -> CandidateHealth {
    match account.auth_state {
        AccountAuthState::RequiresReauth(_) => return CandidateHealth::ReauthRequired,
        AccountAuthState::Error => return CandidateHealth::Unhealthy,
        _ => {}
    }
    match account.last_error_code.as_deref() {
        Some("checkpoint") => return CandidateHealth::Checkpoint,
        Some("captcha") => return CandidateHealth::Captcha,
        _ => {}
    }
    match account.subscription.status {
        SubscriptionStatus::Forbidden => return CandidateHealth::Blocked,
        SubscriptionStatus::Expired => return CandidateHealth::Expired,
        _ => {}
    }
    match account.health {
        AccountHealthState::Unknown => CandidateHealth::Unknown,
        AccountHealthState::Healthy => CandidateHealth::Healthy,
        AccountHealthState::Degraded => CandidateHealth::Degraded,
        AccountHealthState::Unhealthy => CandidateHealth::Unhealthy,
        AccountHealthState::Blocked => CandidateHealth::Blocked,
    }
}

pub fn candidate_quota(quota: &QuotaSnapshot, now_ms: u64) -> CandidateQuota {
    if quota
        .updated_at_ms
        .is_some_and(|updated_at| now_ms.saturating_sub(updated_at) > QUOTA_STALE_AFTER_MS)
    {
        return CandidateQuota::Stale;
    }
    let remaining = quota
        .primary
        .iter()
        .chain(quota.secondary.iter())
        .filter_map(|window| window.available_basis_points)
        .map(u64::from)
        .min();
    match remaining {
        Some(0) => CandidateQuota::Exhausted,
        Some(remaining) => CandidateQuota::Available(remaining),
        None => CandidateQuota::Unknown,
    }
}

fn hash(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn credential_error(error: super::credentials::CredentialError) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::InvalidState, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::{
        accounts::ReauthReason,
        quota::{QuotaWindow, QuotaWindowKind},
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
        assert!(!serialized.contains("provider-account"));
        assert!(!serialized.contains("access-secret"));
        assert!(!serialized.contains("refresh-secret"));
        assert!(serialized.contains("p***@e***.test"));
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
    }

    #[test]
    fn quota_uses_most_constrained_window_and_becomes_stale() {
        let quota = QuotaSnapshot {
            primary: Some(QuotaWindow {
                kind: QuotaWindowKind::Primary,
                available_basis_points: Some(8_000),
                explicitly_full: None,
                reset_at_ms: None,
                window_minutes: None,
                observed_at_ms: 1,
                full_transition_fingerprint: None,
            }),
            secondary: Some(QuotaWindow {
                kind: QuotaWindowKind::Secondary,
                available_basis_points: Some(2_000),
                explicitly_full: None,
                reset_at_ms: None,
                window_minutes: None,
                observed_at_ms: 1,
                full_transition_fingerprint: None,
            }),
            reset_credits_available: None,
            updated_at_ms: Some(1),
            error: None,
        };
        assert_eq!(candidate_quota(&quota, 2), CandidateQuota::Available(2_000));
        assert_eq!(
            candidate_quota(&quota, QUOTA_STALE_AFTER_MS + 2),
            CandidateQuota::Stale
        );
    }
}
