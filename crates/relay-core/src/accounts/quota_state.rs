use super::{provider_account_failure, AccountHealthState, ProviderAccountFailure};
use crate::quota::{
    QuotaErrorState, QuotaNormalizationError, QuotaRefreshFailure, QuotaRefreshResult,
    QuotaSnapshot, QuotaTransition, QuotaWindowKind, Subscription,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AccountQuotaOutcome {
    Updated { transitions: Vec<QuotaTransition> },
    Failed { code: String, retryable: bool },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountQuotaUpdate {
    pub quota: QuotaSnapshot,
    pub subscription: Subscription,
    pub health: AccountHealthState,
    pub last_error_code: Option<String>,
    pub outcome: AccountQuotaOutcome,
    pub exhaustion_transitions: Vec<QuotaTransition>,
}

pub fn reduce_account_quota(
    previous_quota: &QuotaSnapshot,
    previous_subscription: &Subscription,
    previous_health: AccountHealthState,
    previous_last_error_code: Option<&str>,
    result: Result<QuotaRefreshResult, QuotaRefreshFailure>,
    failure_observed_at_ms: u64,
) -> Result<AccountQuotaUpdate, QuotaNormalizationError> {
    match result {
        Ok(mut data) => {
            data.quota
                .preserve_subscription_metadata(previous_subscription);
            let (quota, subscription) = data.quota.normalize(previous_quota)?;
            let transitions = [QuotaWindowKind::Primary, QuotaWindowKind::Secondary]
                .into_iter()
                .filter_map(|kind| {
                    quota
                        .window(kind)
                        .and_then(|window| window.full_transition_from(previous_quota.window(kind)))
                })
                .collect();
            let exhaustion_transitions = [QuotaWindowKind::Primary, QuotaWindowKind::Secondary]
                .into_iter()
                .filter_map(|kind| {
                    quota.window(kind).and_then(|window| {
                        window.exhaustion_transition_from(previous_quota.window(kind))
                    })
                })
                .collect();
            let health = if data.allowed == Some(false) && data.reported_limit_reached != Some(true)
            {
                AccountHealthState::Blocked
            } else {
                AccountHealthState::Healthy
            };
            let quota_owned_error = previous_last_error_code.is_some_and(|code| {
                code.starts_with("quota_")
                    || previous_quota
                        .error
                        .as_ref()
                        .is_some_and(|error| error.code == code)
            });
            let (health, last_error_code) = if health == AccountHealthState::Blocked {
                (health, Some("quota_forbidden".to_string()))
            } else if previous_last_error_code.is_some() && !quota_owned_error {
                (
                    previous_health,
                    previous_last_error_code.map(str::to_string),
                )
            } else {
                (health, None)
            };
            Ok(AccountQuotaUpdate {
                quota,
                subscription: subscription.unwrap_or_else(|| previous_subscription.clone()),
                health,
                last_error_code,
                outcome: AccountQuotaOutcome::Updated { transitions },
                exhaustion_transitions,
            })
        }
        Err(failure) => {
            let mut quota = previous_quota.clone();
            quota.error = Some(QuotaErrorState::new(&failure.code, failure_observed_at_ms));
            let (health, last_error_code) = match provider_account_failure(&failure.code) {
                Some(ProviderAccountFailure::Authentication) => {
                    (AccountHealthState::Unhealthy, Some(failure.code.clone()))
                }
                Some(ProviderAccountFailure::Blocked) => {
                    (AccountHealthState::Blocked, Some(failure.code.clone()))
                }
                None if failure.http_status() == Some(401) => {
                    (AccountHealthState::Unhealthy, Some(failure.code.clone()))
                }
                None => (
                    previous_health,
                    previous_last_error_code.map(str::to_string),
                ),
            };
            Ok(AccountQuotaUpdate {
                quota,
                subscription: previous_subscription.clone(),
                health,
                last_error_code,
                outcome: AccountQuotaOutcome::Failed {
                    code: failure.code,
                    retryable: failure.retryable,
                },
                exhaustion_transitions: Vec::new(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{providers::chatgpt::parse_codex_usage, quota::SubscriptionInput, CandidateQuota};

    fn subscription() -> Subscription {
        Subscription::normalize(SubscriptionInput {
            plan_type: Some("plus".into()),
            active_until_ms: Some(10_000),
            forbidden: false,
            observed_at_ms: 1,
        })
    }

    #[test]
    fn parser_reducer_and_routing_quota_form_one_pipeline() {
        let data = parse_codex_usage(
            br#"{
                "plan_type":"plus",
                "rate_limit":{"allowed":true,"primary_window":{"used_percent":20}}
            }"#,
            2_000,
        )
        .unwrap();
        let update = reduce_account_quota(
            &QuotaSnapshot::default(),
            &subscription(),
            AccountHealthState::Unknown,
            None,
            Ok(data),
            2_000,
        )
        .unwrap();

        assert_eq!(update.health, AccountHealthState::Healthy);
        assert_eq!(update.subscription.active_until_ms, Some(10_000));
        assert_eq!(
            CandidateQuota::from_snapshot(&update.quota, 2_000, 60_000),
            CandidateQuota::Available(8_000)
        );
    }

    #[test]
    fn failures_preserve_last_good_quota_and_only_terminal_auth_changes_health() {
        let previous = reduce_account_quota(
            &QuotaSnapshot::default(),
            &subscription(),
            AccountHealthState::Unknown,
            None,
            Ok(parse_codex_usage(
                br#"{"rate_limit":{"primary_window":{"used_percent":40}}}"#,
                10,
            )
            .unwrap()),
            10,
        )
        .unwrap();
        let transient = reduce_account_quota(
            &previous.quota,
            &previous.subscription,
            previous.health,
            previous.last_error_code.as_deref(),
            Err(QuotaRefreshFailure::new("quota_transport", true)),
            20,
        )
        .unwrap();
        assert_eq!(transient.quota.primary, previous.quota.primary);
        assert_eq!(transient.health, AccountHealthState::Healthy);
        assert_eq!(transient.last_error_code, None);

        let unauthorized = reduce_account_quota(
            &transient.quota,
            &transient.subscription,
            transient.health,
            transient.last_error_code.as_deref(),
            Err(crate::quota::classify_quota_http_failure(
                401,
                br#"{"detail":{"code":"future_auth_error"}}"#,
            )),
            30,
        )
        .unwrap();
        assert_eq!(unauthorized.health, AccountHealthState::Unhealthy);
        assert_eq!(
            unauthorized.last_error_code.as_deref(),
            Some("future_auth_error")
        );
    }

    #[test]
    fn successful_quota_refresh_only_clears_quota_owned_errors() {
        let data = || {
            parse_codex_usage(
                br#"{"rate_limit":{"primary_window":{"used_percent":20}}}"#,
                20,
            )
            .unwrap()
        };
        let runtime_failure = reduce_account_quota(
            &QuotaSnapshot::default(),
            &subscription(),
            AccountHealthState::Degraded,
            Some("upstream_rate_limited"),
            Ok(data()),
            20,
        )
        .unwrap();
        assert_eq!(runtime_failure.health, AccountHealthState::Degraded);
        assert_eq!(
            runtime_failure.last_error_code.as_deref(),
            Some("upstream_rate_limited")
        );

        let previous_quota = QuotaSnapshot {
            error: Some(QuotaErrorState::new("token_invalidated", 10)),
            ..QuotaSnapshot::default()
        };
        let recovered = reduce_account_quota(
            &previous_quota,
            &subscription(),
            AccountHealthState::Unhealthy,
            Some("token_invalidated"),
            Ok(data()),
            20,
        )
        .unwrap();
        assert_eq!(recovered.health, AccountHealthState::Healthy);
        assert_eq!(recovered.last_error_code, None);
    }

    #[test]
    fn weekly_exhaustion_transition_is_emitted_only_on_positive_to_zero() {
        let previous = reduce_account_quota(
            &QuotaSnapshot::default(),
            &subscription(),
            AccountHealthState::Unknown,
            None,
            Ok(parse_codex_usage(
                br#"{"rate_limit":{"secondary_window":{"used_percent":50,"limit_window_seconds":604800,"reset_at":1700000600}}}"#,
                1_000,
            )
            .unwrap()),
            1_000,
        )
        .unwrap();
        let exhausted = reduce_account_quota(
            &previous.quota,
            &previous.subscription,
            previous.health,
            previous.last_error_code.as_deref(),
            Ok(parse_codex_usage(
                br#"{"rate_limit":{"secondary_window":{"used_percent":100,"limit_window_seconds":604800,"reset_at":1700000600}}}"#,
                2_000,
            )
            .unwrap()),
            2_000,
        )
        .unwrap();
        assert_eq!(exhausted.exhaustion_transitions.len(), 1);
        assert_eq!(
            exhausted.exhaustion_transitions[0].window_kind,
            QuotaWindowKind::Secondary
        );
        let repeated = reduce_account_quota(
            &exhausted.quota,
            &exhausted.subscription,
            exhausted.health,
            exhausted.last_error_code.as_deref(),
            Ok(parse_codex_usage(
                br#"{"rate_limit":{"secondary_window":{"used_percent":100,"limit_window_seconds":604800,"reset_at":1700000600}}}"#,
                3_000,
            )
            .unwrap()),
            3_000,
        )
        .unwrap();
        assert!(repeated.exhaustion_transitions.is_empty());
    }
}
