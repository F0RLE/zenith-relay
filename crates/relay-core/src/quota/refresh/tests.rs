use super::*;

fn window(kind: QuotaWindowKind, available_percent: f64) -> QuotaWindowInput {
    QuotaWindowInput {
        kind,
        available_percent: Some(available_percent),
        explicitly_full: None,
        reset: None,
        window_minutes: Some(300),
        provider_cycle_id: None,
        observed_at_ms: 1_000,
    }
}

fn supplemental(id: String) -> SupplementalQuotaWindowInput {
    SupplementalQuotaWindowInput {
        id,
        label: " Code Review ".into(),
        service_tier: None,
        window: window(QuotaWindowKind::Primary, 75.0),
    }
}

#[test]
fn supplemental_windows_are_normalized_without_raw_provider_data() {
    let data = QuotaRefreshData {
        supplemental: vec![supplemental("code_review:primary".into())],
        observed_at_ms: 1_000,
        ..Default::default()
    };
    let (snapshot, _) = data.normalize(&QuotaSnapshot::default()).unwrap();
    assert_eq!(snapshot.supplemental[0].id, "code_review:primary");
    assert_eq!(snapshot.supplemental[0].label, "Code Review");
    assert_eq!(
        snapshot.supplemental[0].window.available_basis_points,
        Some(7_500)
    );
}

#[test]
fn supplemental_window_ids_are_unique_bounded_and_safe() {
    let duplicate = QuotaRefreshData {
        supplemental: vec![
            supplemental("code_review:primary".into()),
            supplemental("code_review:primary".into()),
        ],
        ..Default::default()
    };
    assert_eq!(
        duplicate.normalize(&QuotaSnapshot::default()).unwrap_err(),
        QuotaNormalizationError::InvalidSupplementalWindow
    );

    let oversized = QuotaRefreshData {
        supplemental: (0..=MAX_SUPPLEMENTAL_WINDOWS)
            .map(|index| supplemental(format!("additional:{index}")))
            .collect(),
        ..Default::default()
    };
    assert_eq!(
        oversized.normalize(&QuotaSnapshot::default()).unwrap_err(),
        QuotaNormalizationError::InvalidSupplementalWindow
    );

    let unsafe_id = QuotaRefreshData {
        supplemental: vec![supplemental("unsafe id".into())],
        ..Default::default()
    };
    assert_eq!(
        unsafe_id.normalize(&QuotaSnapshot::default()).unwrap_err(),
        QuotaNormalizationError::InvalidSupplementalWindow
    );
}

#[test]
fn quota_refresh_preserves_subscription_expiry_when_usage_only_reports_plan() {
    let previous = Subscription::normalize(SubscriptionInput {
        plan_type: Some("plus".into()),
        active_until_ms: Some(2_000),
        forbidden: false,
        observed_at_ms: 1,
    });
    let mut data = QuotaRefreshData {
        subscription: Some(SubscriptionInput {
            plan_type: Some("plus".into()),
            active_until_ms: None,
            forbidden: false,
            observed_at_ms: 10,
        }),
        ..Default::default()
    };

    data.preserve_subscription_metadata(&previous);

    assert_eq!(data.subscription.unwrap().active_until_ms, Some(2_000));
}

#[test]
fn quota_refresh_preserves_expiry_across_openai_plan_aliases() {
    for (previous_plan, observed_plan) in [
        ("chatgptplusplan", "plus"),
        ("chatgptbusinessplan", "business"),
        ("chatgptteamplan", "business"),
    ] {
        let previous = Subscription::normalize(SubscriptionInput {
            plan_type: Some(previous_plan.into()),
            active_until_ms: Some(2_000),
            forbidden: false,
            observed_at_ms: 1,
        });
        let mut data = QuotaRefreshData {
            subscription: Some(SubscriptionInput {
                plan_type: Some(observed_plan.into()),
                active_until_ms: None,
                forbidden: false,
                observed_at_ms: 10,
            }),
            ..Default::default()
        };

        data.preserve_subscription_metadata(&previous);

        assert_eq!(data.subscription.unwrap().active_until_ms, Some(2_000));
    }
}

#[test]
fn quota_refresh_does_not_copy_an_expiry_between_different_plans() {
    for (previous_plan, observed_plan) in [
        ("free", "plus"),
        ("plus", "business"),
        ("business", "pro"),
        ("team", "free"),
    ] {
        let previous = Subscription::normalize(SubscriptionInput {
            plan_type: Some(previous_plan.into()),
            active_until_ms: Some(2_000),
            forbidden: false,
            observed_at_ms: 1,
        });
        let mut data = QuotaRefreshData {
            subscription: Some(SubscriptionInput {
                plan_type: Some(observed_plan.into()),
                active_until_ms: None,
                forbidden: false,
                observed_at_ms: 10,
            }),
            ..Default::default()
        };

        data.preserve_subscription_metadata(&previous);

        assert_eq!(data.subscription.unwrap().active_until_ms, None);
    }
}

#[test]
fn quota_http_failure_keeps_safe_provider_codes() {
    let invalidated =
        classify_quota_http_failure(401, br#"{"detail":{"code":"token_invalidated"}}"#);
    assert_eq!(invalidated.code, "token_invalidated");
    assert_eq!(invalidated.http_status(), Some(401));
    assert!(!invalidated.retryable);

    assert_eq!(
        classify_quota_http_failure(401, b"task expired").code,
        "quota_unauthorized"
    );

    let workspace =
        classify_quota_http_failure(402, br#"{"error":{"code":"deactivated_workspace"}}"#);
    assert_eq!(workspace.code, "deactivated_workspace");
    assert_eq!(workspace.http_status(), Some(402));
    assert!(!workspace.retryable);

    assert_eq!(
        classify_quota_http_failure(401, b"").code,
        "quota_unauthorized"
    );
    assert!(classify_quota_http_failure(503, b"").retryable);
}
