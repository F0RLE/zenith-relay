use crate::local_pool::models::LocalAccountRecord;
use zenith_relay_core::{
    accounts::{reduce_account_quota, AccountQuotaOutcome, AccountQuotaUpdate},
    quota::{QuotaRefreshFailure, QuotaRefreshResult, QuotaTransition},
};

#[derive(Clone, Debug, PartialEq)]
pub struct AppliedQuota {
    pub transitions: Vec<QuotaTransition>,
}

pub fn apply_quota_success(
    account: &mut LocalAccountRecord,
    data: QuotaRefreshResult,
) -> Result<AppliedQuota, &'static str> {
    let observed_at_ms = data.quota.observed_at_ms;
    let update = reduce_account_quota(
        &account.account.quota,
        &account.account.subscription,
        account.account.health,
        account.account.last_error_code.as_deref(),
        Ok(data),
        observed_at_ms,
    )
    .map_err(|_| "quota response could not be normalized")?;
    let transitions = match &update.outcome {
        AccountQuotaOutcome::Updated { transitions } => transitions.clone(),
        AccountQuotaOutcome::Failed { .. } => return Err("quota result kind is invalid"),
    };
    apply_update(account, update);
    account
        .economics
        .set_account_context("chatgpt", account.account.subscription.plan_type.as_deref());
    account
        .economics
        .set_value_revision(zenith_relay_core::quota::quota_valuation_revision());
    account.economics.observe_quota(&account.account.quota);
    Ok(AppliedQuota { transitions })
}

pub fn apply_quota_failure(
    account: &mut LocalAccountRecord,
    failure: &QuotaRefreshFailure,
    now_ms: u64,
) {
    let update = reduce_account_quota(
        &account.account.quota,
        &account.account.subscription,
        account.account.health,
        account.account.last_error_code.as_deref(),
        Err(failure.clone()),
        now_ms,
    )
    .expect("quota failure reduction does not normalize provider data");
    apply_update(account, update);
}

fn apply_update(account: &mut LocalAccountRecord, update: AccountQuotaUpdate) {
    account.account.quota = update.quota;
    account.account.subscription = update.subscription;
    account.account.health = update.health;
    account.account.last_error_code = update.last_error_code;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::accounts::{credentials::StoredCodexCredentials, records};
    use zenith_relay_core::{
        accounts::{AccountAuthMode, AccountAuthState, AccountHealthState},
        quota::{
            QuotaRefreshData, QuotaWindowInput, QuotaWindowKind, ResetTime, SubscriptionInput,
            SubscriptionStatus,
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

    fn refresh(percent: f64, observed_at_ms: u64) -> QuotaRefreshResult {
        QuotaRefreshResult {
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
                limit_reached: false,
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
            reported_limit_reached: Some(false),
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
        assert_eq!(account.account.health, AccountHealthState::Healthy);
        assert_eq!(account.account.auth_state, AccountAuthState::Active);
        assert_eq!(account.account.last_error_code, None);
        assert_eq!(
            account.account.quota.error.as_ref().unwrap().code,
            "quota_transport"
        );
    }

    #[test]
    fn quota_failure_does_not_hide_an_active_routing_failure() {
        let mut account = account();
        account.account.health = AccountHealthState::Blocked;
        account.account.last_error_code = Some("upstream_rate_limited".into());

        apply_quota_failure(
            &mut account,
            &QuotaRefreshFailure::new("quota_transport", true),
            20,
        );

        assert_eq!(
            account.account.last_error_code.as_deref(),
            Some("upstream_rate_limited")
        );
        assert_eq!(account.account.health, AccountHealthState::Blocked);
        assert_eq!(
            account.account.quota.error.as_ref().unwrap().code,
            "quota_transport"
        );
    }

    #[test]
    fn terminal_quota_probe_failure_does_not_disable_a_working_account() {
        for code in ["quota_forbidden", "quota_unauthorized"] {
            let mut account = account();
            apply_quota_success(&mut account, refresh(40.0, 10)).unwrap();

            apply_quota_failure(&mut account, &QuotaRefreshFailure::new(code, false), 20);

            assert_eq!(account.account.auth_state, AccountAuthState::Active);
            assert_eq!(account.account.health, AccountHealthState::Healthy);
            assert_eq!(account.account.last_error_code, None);
            assert_eq!(account.account.quota.error.as_ref().unwrap().code, code);
        }
    }

    #[test]
    fn exact_provider_failure_excludes_an_unusable_account() {
        let mut invalidated = account();
        apply_quota_failure(
            &mut invalidated,
            &QuotaRefreshFailure::new("token_invalidated", false),
            20,
        );
        assert_eq!(invalidated.account.auth_state, AccountAuthState::Active);
        assert_eq!(invalidated.account.health, AccountHealthState::Unhealthy);

        let mut deactivated = account();
        apply_quota_failure(
            &mut deactivated,
            &QuotaRefreshFailure::new("deactivated_workspace", false),
            20,
        );
        assert_eq!(deactivated.account.health, AccountHealthState::Blocked);

        let mut future_unauthorized = account();
        apply_quota_failure(
            &mut future_unauthorized,
            &zenith_relay_core::quota::classify_quota_http_failure(
                401,
                br#"{"detail":{"code":"future_auth_error"}}"#,
            ),
            20,
        );
        assert_eq!(
            future_unauthorized.account.health,
            AccountHealthState::Unhealthy
        );
        assert_eq!(
            future_unauthorized.account.last_error_code.as_deref(),
            Some("future_auth_error")
        );
    }

    #[test]
    fn provider_access_denial_blocks_without_erasing_quota() {
        let mut account = account();
        let mut data = refresh(95.0, 10);
        data.allowed = Some(false);
        data.reported_limit_reached = Some(false);
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
    fn successful_quota_refresh_preserves_runtime_failure_state() {
        let mut account = account();
        account.account.health = AccountHealthState::Degraded;
        account.account.last_error_code = Some("upstream_rate_limited".into());
        account.cooldowns.insert("*".into(), 60_000);
        account.cooldowns.insert("gpt-test".into(), 30_000);
        account.consecutive_failures = 3;

        apply_quota_success(&mut account, refresh(40.0, 10)).unwrap();

        assert_eq!(account.cooldowns.get("*"), Some(&60_000));
        assert_eq!(account.cooldowns.get("gpt-test"), Some(&30_000));
        assert_eq!(account.consecutive_failures, 3);
        assert_eq!(account.account.health, AccountHealthState::Degraded);
        assert_eq!(
            account.account.last_error_code.as_deref(),
            Some("upstream_rate_limited")
        );
    }

    #[test]
    fn exhausted_window_uses_quota_without_creating_a_cooldown() {
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

        assert_eq!(
            account
                .account
                .quota
                .primary
                .as_ref()
                .unwrap()
                .available_basis_points,
            Some(0)
        );
        assert!(account.cooldowns.is_empty());
    }
}
