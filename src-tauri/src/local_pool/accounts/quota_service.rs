use super::quota::CodexQuotaRefreshData;
use crate::local_pool::models::LocalAccountRecord;
use zenith_relay_core::{
    accounts::{AccountAuthState, AccountHealthState},
    quota::{QuotaErrorState, QuotaRefreshFailure, QuotaTransition, QuotaWindowKind},
};

#[derive(Clone, Debug, PartialEq)]
pub struct AppliedQuota {
    pub transitions: Vec<QuotaTransition>,
}

pub fn apply_quota_success(
    account: &mut LocalAccountRecord,
    mut data: CodexQuotaRefreshData,
) -> Result<AppliedQuota, &'static str> {
    let observed_at_ms = data.quota.observed_at_ms;
    let previous = account.account.quota.clone();
    data.quota
        .preserve_subscription_metadata(&account.account.subscription);
    let (quota, subscription) = data
        .quota
        .normalize(&previous)
        .map_err(|_| "quota response could not be normalized")?;
    let transitions = [QuotaWindowKind::Primary, QuotaWindowKind::Secondary]
        .into_iter()
        .filter_map(|kind| {
            quota
                .window(kind)
                .and_then(|window| window.full_transition_from(previous.window(kind)))
        })
        .collect();
    account.account.quota = quota;
    if let Some(subscription) = subscription {
        account.account.subscription = subscription;
    }
    account.account.health = if data.allowed == Some(false) && data.limit_reached != Some(true) {
        AccountHealthState::Blocked
    } else {
        AccountHealthState::Healthy
    };
    account.account.last_error_code = None;
    if account.account.health == AccountHealthState::Healthy {
        if let Some(retry_at_ms) =
            exhausted_quota_retry_at(account, data.limit_reached, observed_at_ms)
        {
            account
                .cooldowns
                .entry("*".into())
                .and_modify(|current| *current = (*current).max(retry_at_ms))
                .or_insert(retry_at_ms);
        } else {
            account.cooldowns.remove("*");
        }
        account.consecutive_failures = 0;
    }
    Ok(AppliedQuota { transitions })
}

fn exhausted_quota_retry_at(
    account: &LocalAccountRecord,
    limit_reached: Option<bool>,
    now_ms: u64,
) -> Option<u64> {
    let reset_at_ms = account
        .account
        .quota
        .primary
        .iter()
        .chain(account.account.quota.secondary.iter())
        .filter(|window| window.available_basis_points == Some(0))
        .filter_map(|window| window.reset_at_ms)
        .filter(|reset_at_ms| *reset_at_ms > now_ms)
        .max();
    reset_at_ms.or_else(|| {
        (limit_reached == Some(true))
            .then(|| {
                account
                    .account
                    .quota
                    .primary
                    .iter()
                    .chain(account.account.quota.secondary.iter())
                    .filter_map(|window| window.reset_at_ms)
                    .filter(|reset_at_ms| *reset_at_ms > now_ms)
                    .max()
            })
            .flatten()
    })
}

pub fn apply_quota_failure(
    account: &mut LocalAccountRecord,
    failure: &QuotaRefreshFailure,
    now_ms: u64,
) {
    account.account.quota.error = Some(QuotaErrorState::new(&failure.code, now_ms));
    account.account.last_error_code = Some(failure.code.clone());
    match failure.code.as_str() {
        "quota_forbidden" => account.account.health = AccountHealthState::Blocked,
        "quota_unauthorized" => {
            account.account.auth_state = AccountAuthState::Error;
            account.account.health = AccountHealthState::Unhealthy;
        }
        _ if failure.retryable => account.account.health = AccountHealthState::Degraded,
        _ => account.account.health = AccountHealthState::Unhealthy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::accounts::{credentials::StoredCodexCredentials, records};
    use zenith_relay_core::{
        accounts::AccountAuthMode,
        quota::{
            QuotaRefreshData, QuotaWindowInput, ResetTime, SubscriptionInput, SubscriptionStatus,
        },
    };

    fn account() -> LocalAccountRecord {
        let credentials = StoredCodexCredentials::new(
            "account_local",
            "access".into(),
            Some("refresh".into()),
            Some("id".into()),
            Some(60_000),
            1,
            0,
            None,
            Some("provider-account".into()),
            None,
            None,
            Some("plus".into()),
            false,
        )
        .unwrap();
        records::new_account_record(
            &credentials,
            AccountAuthMode::OAuth,
            vec!["gpt-test".into()],
            0,
            1,
        )
        .unwrap()
    }

    fn refresh(percent: f64, observed_at_ms: u64) -> CodexQuotaRefreshData {
        CodexQuotaRefreshData {
            quota: QuotaRefreshData {
                primary: Some(QuotaWindowInput {
                    kind: QuotaWindowKind::Primary,
                    available_percent: Some(percent),
                    explicitly_full: None,
                    reset: Some(ResetTime::RelativeSeconds(60)),
                    window_minutes: Some(300),
                    provider_cycle_id: None,
                    observed_at_ms,
                }),
                secondary: None,
                supplemental: Vec::new(),
                subscription: Some(SubscriptionInput {
                    plan_type: Some("plus".into()),
                    active_until_ms: None,
                    forbidden: false,
                    observed_at_ms,
                }),
                reset_credits_available: Some(1),
                observed_at_ms,
            },
            allowed: Some(true),
            limit_reached: Some(false),
            rate_limit_reached_type: None,
        }
    }

    #[test]
    fn full_transition_is_emitted_once_and_subscription_is_kept() {
        let mut account = account();
        account.account.subscription.active_until_ms = Some(2_000);
        apply_quota_success(&mut account, refresh(20.0, 10)).unwrap();
        let applied = apply_quota_success(&mut account, refresh(100.0, 20)).unwrap();
        assert_eq!(applied.transitions.len(), 1);
        assert_eq!(
            account.account.subscription.status,
            SubscriptionStatus::Active
        );
        assert_eq!(account.account.subscription.active_until_ms, Some(2_000));
        assert!(apply_quota_success(&mut account, refresh(100.0, 30))
            .unwrap()
            .transitions
            .is_empty());
    }

    #[test]
    fn failure_preserves_quota_and_subscription_but_records_safe_error() {
        let mut account = account();
        apply_quota_success(&mut account, refresh(40.0, 10)).unwrap();
        let previous_quota = account.account.quota.clone();
        let previous_subscription = account.account.subscription.clone();
        apply_quota_failure(
            &mut account,
            &QuotaRefreshFailure::new("quota_transport", true),
            20,
        );
        assert_eq!(account.account.quota.primary, previous_quota.primary);
        assert_eq!(account.account.subscription, previous_subscription);
        assert_eq!(account.account.health, AccountHealthState::Degraded);
        assert_eq!(
            account.account.quota.error.as_ref().unwrap().code,
            "quota_transport"
        );
    }

    #[test]
    fn provider_access_denial_blocks_without_erasing_quota() {
        let mut account = account();
        let mut data = refresh(95.0, 10);
        data.allowed = Some(false);
        data.limit_reached = Some(false);
        apply_quota_success(&mut account, data).unwrap();
        assert_eq!(account.account.health, AccountHealthState::Blocked);
        assert_eq!(
            account
                .account
                .quota
                .primary
                .as_ref()
                .unwrap()
                .available_basis_points,
            Some(9_500)
        );
    }

    #[test]
    fn successful_quota_refresh_clears_only_global_failure_state() {
        let mut account = account();
        account.cooldowns.insert("*".into(), 60_000);
        account.cooldowns.insert("gpt-test".into(), 30_000);
        account.consecutive_failures = 3;

        apply_quota_success(&mut account, refresh(40.0, 10)).unwrap();

        assert!(!account.cooldowns.contains_key("*"));
        assert_eq!(account.cooldowns.get("gpt-test"), Some(&30_000));
        assert_eq!(account.consecutive_failures, 0);
    }

    #[test]
    fn exhausted_window_keeps_the_latest_reset_as_global_retry_time() {
        let mut account = account();
        let mut data = refresh(0.0, 10);
        data.quota.primary.as_mut().unwrap().reset =
            Some(ResetTime::AbsoluteUnixMilliseconds(5_000));
        data.quota.secondary = Some(QuotaWindowInput {
            kind: QuotaWindowKind::Secondary,
            available_percent: Some(30.0),
            explicitly_full: Some(false),
            reset: Some(ResetTime::AbsoluteUnixMilliseconds(10_000)),
            window_minutes: Some(10_080),
            provider_cycle_id: None,
            observed_at_ms: 10,
        });
        apply_quota_success(&mut account, data).unwrap();

        assert_eq!(account.cooldowns.get("*"), Some(&5_000));
    }
}
