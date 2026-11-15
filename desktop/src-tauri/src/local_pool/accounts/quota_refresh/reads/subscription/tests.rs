use super::*;
use zenith_relay_core::quota::{QuotaRefreshResult, SubscriptionInput};

#[tokio::test]
async fn optional_expiry_404_keeps_partial_metadata_and_distinct_failure_diagnostics() {
    use axum::{Json, Router, http::StatusCode, routing::get};
    use serde_json::json;
    for (check_missing, status, content_type, mitigation, optional_missing) in [
        (false, StatusCode::NOT_FOUND, "application/json", None, true),
        (true, StatusCode::NOT_FOUND, "application/json", None, false),
        (false, StatusCode::NOT_FOUND, "text/html", None, false),
        (
            false,
            StatusCode::NOT_FOUND,
            "application/json",
            Some("challenge"),
            false,
        ),
        (
            false,
            StatusCode::FORBIDDEN,
            "application/json",
            None,
            false,
        ),
        (
            false,
            StatusCode::TOO_MANY_REQUESTS,
            "application/json",
            None,
            false,
        ),
        (
            false,
            StatusCode::SERVICE_UNAVAILABLE,
            "application/json",
            None,
            false,
        ),
    ] {
        let app = Router::new()
            .route("/check", get(move || async move {
                (if check_missing { StatusCode::NOT_FOUND } else { StatusCode::OK },
                    Json(json!({"accounts": [{"account": {"id": "synthetic-account", "plan_type": "plus"}}]})))
            }))
            .route("/subscriptions", get(move || async move {
                let mut response = axum::http::Response::builder()
                    .status(status)
                    .header("content-type", content_type);
                if let Some(value) = mitigation {
                    response = response.header("cf-mitigated", value);
                }
                response.body(axum::body::Body::from("{}" )).unwrap()
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = CodexSubscriptionClient::with_endpoints(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            format!("{base}/check").parse().unwrap(),
            format!("{base}/subscriptions").parse().unwrap(),
        )
        .unwrap();
        let result = client
            .read_authorized(
                HeaderValue::from_static("Bearer synthetic-subscription-test"),
                "synthetic-account",
                10_000,
            )
            .await;
        server.abort();
        let failure = if check_missing {
            result.unwrap_err()
        } else {
            let read = result.unwrap();
            assert_eq!(read.metadata.plan_type.as_deref(), Some("plus"));
            assert_eq!(read.metadata.active_until_ms, None);
            read.fallback_failure.unwrap()
        };
        assert_eq!(optional_expiry_missing(&failure), optional_missing);
    }
}

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
        token_revision: None,
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

#[test]
fn partial_subscription_keeps_same_plan_expiry_and_its_original_freshness() {
    let input = SubscriptionInput {
        plan_type: Some("plus".into()),
        active_until_ms: Some(5_000),
        observed_at_ms: 1_000,
        forbidden: false,
    };
    let mut subscription = Subscription::normalize(input.clone());
    let mut result = QuotaRefreshResult::default();
    result.quota.subscription = Some(input.clone());
    let mut quota = QuotaRefreshOutcome::Updated(Box::new(result));
    apply_partial_subscription_read(
        &mut quota,
        &mut subscription,
        CodexSubscriptionMetadata {
            plan_type: Some("plus".into()),
            ..Default::default()
        },
        10_000,
    );
    assert_eq!(subscription.active_until_ms, Some(5_000));
    assert_eq!(subscription.updated_at_ms, Some(1_000));
    assert_eq!(
        subscription.status,
        zenith_relay_core::quota::SubscriptionStatus::Expired
    );
    let QuotaRefreshOutcome::Updated(result) = quota else {
        panic!("quota success must survive");
    };
    assert_eq!(result.quota.subscription, Some(input));
}

#[test]
fn partial_subscription_plan_change_drops_the_previous_plans_expiry_and_freshness() {
    let mut subscription = Subscription::normalize(SubscriptionInput {
        plan_type: Some("plus".into()),
        active_until_ms: Some(20_000),
        observed_at_ms: 1_000,
        forbidden: false,
    });
    let mut quota = QuotaRefreshOutcome::Updated(Box::default());
    apply_partial_subscription_read(
        &mut quota,
        &mut subscription,
        CodexSubscriptionMetadata {
            plan_type: Some("free".into()),
            ..Default::default()
        },
        10_000,
    );
    assert_eq!(subscription.plan_type.as_deref(), Some("free"));
    assert_eq!(subscription.active_until_ms, None);
    assert_eq!(subscription.updated_at_ms, None);
}

#[test]
fn partial_subscription_neither_fabricates_quota_success_nor_clears_access_rejection() {
    let failure = QuotaRefreshFailure::new(error_codes::QUOTA_UPSTREAM, true);
    let mut subscription = Subscription::normalize(SubscriptionInput {
        plan_type: Some("plus".into()),
        active_until_ms: Some(20_000),
        observed_at_ms: 1_000,
        forbidden: true,
    });
    let mut quota = QuotaRefreshOutcome::Failed {
        failure: failure.clone(),
        subscription: subscription.clone(),
    };
    apply_partial_subscription_read(
        &mut quota,
        &mut subscription,
        CodexSubscriptionMetadata {
            plan_type: Some("plus".into()),
            ..Default::default()
        },
        10_000,
    );
    let QuotaRefreshOutcome::Failed {
        failure: actual,
        subscription: observed,
    } = quota
    else {
        panic!("partial metadata cannot fabricate quota success");
    };
    assert_eq!(actual, failure);
    assert_eq!(observed, subscription);
    assert_eq!(
        subscription.status,
        zenith_relay_core::quota::SubscriptionStatus::Forbidden
    );
    assert_eq!(subscription.updated_at_ms, Some(1_000));
}
