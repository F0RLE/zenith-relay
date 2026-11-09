use super::*;
use crate::scheduler::refresh::http::{HttpAdmissionError, HttpSendError, ManagementHttpScope};
use reqwest::header::HeaderValue;

#[test]
fn subscription_send_failures_keep_safe_causes_and_retry_policy() {
    for (error, code, retryable, reason) in [
        (
            HttpSendError::Stale,
            error_codes::SUBSCRIPTION_TRANSPORT,
            false,
            "stale_scope",
        ),
        (
            HttpSendError::Admission(HttpAdmissionError::InvalidOrigin),
            error_codes::SUBSCRIPTION_CONFIGURATION,
            false,
            "invalid_origin",
        ),
        (
            HttpSendError::Admission(HttpAdmissionError::Full),
            error_codes::SUBSCRIPTION_TRANSPORT,
            true,
            "admission_full",
        ),
        (
            HttpSendError::Admission(HttpAdmissionError::TimedOut),
            error_codes::SUBSCRIPTION_TRANSPORT,
            true,
            "admission_timeout",
        ),
        (
            HttpSendError::Transport { timeout: true },
            error_codes::SUBSCRIPTION_TRANSPORT,
            true,
            "timeout",
        ),
        (
            HttpSendError::Transport { timeout: false },
            error_codes::SUBSCRIPTION_TRANSPORT,
            true,
            "network",
        ),
    ] {
        for stage in ["accounts_check", "subscriptions"] {
            let failure = super::super::client::send_failure(error, stage);
            assert_eq!(failure.code, code);
            assert_eq!(failure.retryable, retryable);
            assert_eq!(failure.transport_failure(), Some(reason));
            assert_eq!(failure.endpoint_stage(), Some(stage));
            assert_eq!(failure.http_status(), None);
            assert_eq!(failure.response_mitigation(), None);
        }
    }
}

#[tokio::test]
async fn subscription_read_does_not_retry_a_stale_scope() {
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .fallback(|State(calls): State<Arc<AtomicUsize>>| async move {
            calls.fetch_add(1, Ordering::SeqCst);
            StatusCode::OK
        })
        .with_state(calls.clone());
    let (client, server) = check_client(router).await;
    let checks = Arc::new(AtomicUsize::new(0));
    let captured = checks.clone();
    let client = client.with_http_scope(ManagementHttpScope::checked(move || {
        captured.fetch_add(1, Ordering::SeqCst);
        false
    }));
    let failure = client
        .fetch("synthetic-secret", "account-target", 100)
        .await
        .unwrap_err();
    server.abort();
    assert!(!failure.retryable);
    assert_eq!(failure.transport_failure(), Some("stale_scope"));
    assert_eq!(failure.endpoint_stage(), Some("accounts_check"));
    assert_eq!(checks.load(Ordering::SeqCst), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!format!("{failure:?}").contains("synthetic-secret"));
}

#[tokio::test]
async fn subscription_fallback_does_not_retry_a_stale_scope_or_discard_metadata() {
    use std::sync::atomic::AtomicBool;

    let current = Arc::new(AtomicBool::new(true));
    let captured = current.clone();
    let router = Router::new().fallback(get(move || {
        let current = captured.clone();
        async move {
            current.store(false, Ordering::SeqCst);
            Json(json!({"accounts":[{
                "account":{"id":"account-target","plan_type":"plus"}
            }]}))
        }
    }));
    let (client, server) = check_client(router).await;
    let stale_checks = Arc::new(AtomicUsize::new(0));
    let captured = stale_checks.clone();
    let client = client.with_http_scope(ManagementHttpScope::checked(move || {
        let fresh = current.load(Ordering::SeqCst);
        if !fresh {
            captured.fetch_add(1, Ordering::SeqCst);
        }
        fresh
    }));
    let read = client
        .read_authorized(
            HeaderValue::from_static("Bearer synthetic-secret"),
            "account-target",
            100,
        )
        .await
        .unwrap();
    server.abort();
    assert_eq!(read.metadata.plan_type.as_deref(), Some("plus"));
    assert_eq!(read.metadata.active_until_ms, None);
    let failure = read.fallback_failure.unwrap();
    assert!(!failure.retryable);
    assert_eq!(failure.transport_failure(), Some("stale_scope"));
    assert_eq!(failure.endpoint_stage(), Some("subscriptions"));
    assert_eq!(stale_checks.load(Ordering::SeqCst), 1);
}
