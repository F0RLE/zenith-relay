use std::collections::BTreeMap;
use zenith_relay_core::providers::chatgpt::SUBSCRIPTION_REFRESH_INTERVAL_MS;
use zenith_relay_core::quota::QuotaRefreshFailure;
use zenith_relay_core::scheduler::refresh::RefreshIdentity;

/// Transport floors for optional subscription reads, not quota/access health.
/// The existing global subscription lock serializes both requests and floors.
#[derive(Default)]
pub(crate) struct SubscriptionRefreshState {
    retry_at: BTreeMap<String, (RefreshIdentity, u64)>,
}

impl SubscriptionRefreshState {
    pub(crate) fn can_refresh(&mut self, identity: &RefreshIdentity, now_ms: u64) -> bool {
        self.retry_at.retain(|_, (_, until)| *until > now_ms);
        !self
            .retry_at
            .get(&identity.member_id)
            .is_some_and(|(saved, _)| saved == identity)
    }

    pub(crate) fn record_failure(
        &mut self,
        identity: &RefreshIdentity,
        now_ms: u64,
        failure: &QuotaRefreshFailure,
    ) {
        // An optional read without Retry-After still needs a floor. Permanent
        // rejection waits for the normal metadata interval; transient failure
        // can recover sooner. Neither changes quota/access observations.
        let minimum_delay = if failure.retryable {
            60_000
        } else {
            SUBSCRIPTION_REFRESH_INTERVAL_MS
        };
        self.respect_retry_after(
            identity,
            now_ms,
            failure
                .retry_after_ms()
                .unwrap_or_default()
                .max(minimum_delay),
        );
    }

    pub(crate) fn respect_retry_after(
        &mut self,
        identity: &RefreshIdentity,
        now_ms: u64,
        delay_ms: u64,
    ) {
        self.retry_at.retain(|_, (_, until)| *until > now_ms);
        let until = now_ms.saturating_add(delay_ms);
        let entry = self
            .retry_at
            .entry(identity.member_id.clone())
            .or_insert_with(|| (identity.clone(), until));
        if entry.0 == *identity {
            entry.1 = entry.1.max(until);
        } else {
            *entry = (identity.clone(), until);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_failure_without_retry_after_does_not_repeat_on_each_quota_poll() {
        for (code, retryable, delay) in [
            (
                "subscription_forbidden",
                false,
                SUBSCRIPTION_REFRESH_INTERVAL_MS,
            ),
            (
                "subscription_unauthorized",
                false,
                SUBSCRIPTION_REFRESH_INTERVAL_MS,
            ),
            ("subscription_rate_limited", true, 60_000),
            ("subscription_transport", true, 60_000),
        ] {
            let identity = RefreshIdentity::new("account", 1, 2);
            let failure = QuotaRefreshFailure::new(code, retryable);
            let mut state = SubscriptionRefreshState::default();
            state.record_failure(&identity, 100, &failure);
            assert!(!state.can_refresh(&identity, 100 + delay - 1), "{code}");
            assert!(state.can_refresh(&identity, 100 + delay), "{code}");
        }
    }

    #[test]
    fn subscription_failure_preserves_longer_provider_pause_and_allows_new_identity() {
        let identity = RefreshIdentity::new("account", 1, 2);
        let mut state = SubscriptionRefreshState::default();
        state.respect_retry_after(&identity, 100, 2 * SUBSCRIPTION_REFRESH_INTERVAL_MS);
        state.record_failure(
            &identity,
            200,
            &QuotaRefreshFailure::new("subscription_forbidden", false),
        );
        assert!(!state.can_refresh(&identity, 100 + 2 * SUBSCRIPTION_REFRESH_INTERVAL_MS - 1));
        assert!(state.can_refresh(&RefreshIdentity::new("other", 1, 2), 200));
        assert!(state.can_refresh(&RefreshIdentity::new("account", 2, 2), 200));
        assert!(state.can_refresh(&RefreshIdentity::new("account", 1, 3), 200));
        assert!(state.can_refresh(&identity, 100 + 2 * SUBSCRIPTION_REFRESH_INTERVAL_MS));
    }

    #[tokio::test]
    async fn subscription_http_failures_respect_provider_delays_and_local_floors() {
        use axum::http::StatusCode;
        use zenith_relay_core::providers::chatgpt::CodexSubscriptionClient;

        for (status, retry_after, delay) in [
            (
                StatusCode::FORBIDDEN,
                None,
                SUBSCRIPTION_REFRESH_INTERVAL_MS,
            ),
            (
                StatusCode::FORBIDDEN,
                Some("0"),
                SUBSCRIPTION_REFRESH_INTERVAL_MS,
            ),
            (StatusCode::FORBIDDEN, Some("3600"), 3_600_000),
            (StatusCode::TOO_MANY_REQUESTS, None, 60_000),
            (StatusCode::TOO_MANY_REQUESTS, Some("120"), 120_000),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/subscription", listener.local_addr().unwrap());
            let app = axum::Router::new().fallback(move || async move {
                let mut response = axum::http::Response::builder().status(status);
                if let Some(delay) = retry_after {
                    response = response.header("retry-after", delay);
                }
                response.body(axum::body::Body::empty()).unwrap()
            });
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let http = reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(3))
                .build()
                .unwrap();
            let client = CodexSubscriptionClient::with_endpoints(
                http,
                endpoint.parse().unwrap(),
                endpoint.parse().unwrap(),
            )
            .unwrap();
            let failure = client
                .fetch_authorized(
                    reqwest::header::HeaderValue::from_static("Bearer synthetic-test"),
                    "synthetic-account",
                    100,
                )
                .await
                .unwrap_err();
            server.abort();
            assert_eq!(failure.http_status(), Some(status.as_u16()));
            let identity = RefreshIdentity::new("account", 1, 2);
            let mut state = SubscriptionRefreshState::default();
            state.record_failure(&identity, 100, &failure);
            assert!(!state.can_refresh(&identity, 100 + delay - 1));
            assert!(state.can_refresh(&identity, 100 + delay));
        }
    }

    #[test]
    fn subscription_retry_floor_is_scoped_and_cannot_be_shortened() {
        let identity = RefreshIdentity::new("account", 1, 2);
        let other = RefreshIdentity::new("other", 1, 2);
        let mut state = SubscriptionRefreshState::default();
        state.respect_retry_after(&identity, 100, 60_000);
        state.respect_retry_after(&identity, 200, 1);
        assert!(!state.can_refresh(&identity, 60_099));
        assert!(state.can_refresh(&other, 60_099));
        assert!(state.can_refresh(&identity, 60_100));
        assert!(state.retry_at.is_empty());
    }

    #[test]
    fn new_login_or_configuration_does_not_inherit_an_old_subscription_pause() {
        let identity = RefreshIdentity::new("account", 1, 2);
        let mut state = SubscriptionRefreshState::default();
        state.respect_retry_after(&identity, 100, 60_000);
        assert!(state.can_refresh(&RefreshIdentity::new("account", 2, 2), 200));
        assert!(state.can_refresh(&RefreshIdentity::new("account", 1, 3), 200));
        state.respect_retry_after(&RefreshIdentity::new("account", 2, 2), 200, 1_000);
        assert_eq!(state.retry_at.len(), 1);
        assert!(state.can_refresh(&RefreshIdentity::new("account", 2, 2), 1_200));
    }
}
