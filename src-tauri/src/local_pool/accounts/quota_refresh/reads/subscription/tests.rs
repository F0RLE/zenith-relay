use super::*;
use zenith_relay_core::quota::{QuotaRefreshResult, SubscriptionInput};

#[test]
fn independent_subscription_read_survives_an_empty_quota_reply() {
    let mut quota = QuotaRefreshOutcome::Updated(Box::default());
    let mut subscription = Subscription::default();
    apply_subscription_read(
        &mut quota,
        &mut subscription,
        CodexSubscriptionMetadata {
            plan_type: Some("plus".into()),
            active_until_ms: Some(20_000),
            ..Default::default()
        },
        10_000,
    );
    let QuotaRefreshOutcome::Updated(result) = quota else {
        panic!("metadata must not replace quota success");
    };
    assert_eq!(subscription.active_until_ms, Some(20_000));
    assert_eq!(subscription.updated_at_ms, Some(10_000));
    assert_eq!(
        Subscription::normalize(result.quota.subscription.unwrap()),
        subscription,
    );
}

#[test]
fn successful_subscription_read_removes_an_old_expiry_from_both_observations() {
    let input = SubscriptionInput {
        plan_type: Some("plus".into()),
        active_until_ms: Some(5_000),
        forbidden: false,
        observed_at_ms: 1_000,
    };
    let mut subscription = Subscription::normalize(input.clone());
    let mut result = QuotaRefreshResult::default();
    result.quota.subscription = Some(input);
    let mut quota = QuotaRefreshOutcome::Updated(Box::new(result));
    apply_subscription_read(
        &mut quota,
        &mut subscription,
        CodexSubscriptionMetadata {
            plan_type: Some("plus".into()),
            ..Default::default()
        },
        10_000,
    );
    let QuotaRefreshOutcome::Updated(result) = quota else {
        panic!("metadata must not replace quota success");
    };
    assert_eq!(subscription.active_until_ms, None);
    assert_eq!(
        Subscription::normalize(result.quota.subscription.unwrap()),
        subscription,
    );
}

#[test]
fn independent_subscription_success_preserves_quota_failure() {
    let failure = QuotaRefreshFailure::new(error_codes::QUOTA_UPSTREAM, true);
    let mut subscription = Subscription::default();
    let mut quota = QuotaRefreshOutcome::Failed {
        failure: failure.clone(),
        subscription: subscription.clone(),
    };
    apply_subscription_read(
        &mut quota,
        &mut subscription,
        CodexSubscriptionMetadata {
            plan_type: Some("plus".into()),
            active_until_ms: Some(20_000),
            ..Default::default()
        },
        10_000,
    );
    let QuotaRefreshOutcome::Failed {
        failure: actual,
        subscription: observed,
    } = quota
    else {
        panic!("subscription success must not fabricate quota success");
    };
    assert_eq!(actual, failure);
    assert_eq!(observed, subscription);
    assert_eq!(subscription.active_until_ms, Some(20_000));
}

#[tokio::test]
async fn missing_oauth_skips_subscription_without_using_agent_authorization() {
    let prepared = PreparedAccountAuthorization {
        oauth_client_kind: Default::default(),
        authorization: HeaderValue::from_static("Bearer synthetic-agent-identity"),
        subscription_authorization: None,
        tokens: None,
        agent_task_id: Some("synthetic-task".into()),
        provider_account_id: "synthetic-account".into(),
        proxy: None,
    };
    let metadata = request_subscription_metadata(
        &prepared,
        Duration::from_millis(1),
        10_000,
        &ManagementHttpScope::default(),
    )
    .await
    .unwrap();
    assert_eq!(metadata, None);
}
