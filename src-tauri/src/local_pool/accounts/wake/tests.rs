use super::runtime_exec::{metrics_from_response, MAX_RESPONSE_BYTES};
use super::*;
use axum::body::Body;
use axum::http::{Response, StatusCode};
use serde_json::json;
use std::time::Instant;
use zenith_relay_core::automations::{WakeCompletionOutcome, WakeVerificationOutcome};

const ACCESS_TOKEN: &str = "access-private-secret";
const PROVIDER_ACCOUNT_ID: &str = "provider-private-account";

#[tokio::test]
async fn response_content_and_credentials_never_escape_metrics_or_debug() {
    let private_response = "generated-private-response";
    let response = Response::new(Body::from(
        json!({
            "output_text": private_response,
            "provider_account_id": PROVIDER_ACCOUNT_ID,
            "token": ACCESS_TOKEN,
            "usage": {"input_tokens": 1}
        })
        .to_string(),
    ));
    let metrics = metrics_from_response(response, Instant::now())
        .await
        .unwrap();
    assert_eq!(metrics.input_tokens, Some(1));
    let serialized = serde_json::to_string(&metrics).unwrap();
    let debug = format!("{metrics:?}");
    for secret in [private_response, PROVIDER_ACCOUNT_ID, ACCESS_TOKEN] {
        assert!(!serialized.contains(secret));
        assert!(!debug.contains(secret));
    }
}

#[tokio::test]
async fn oversized_and_malformed_success_responses_fail_closed() {
    let response = Response::new(Body::from(vec![b'x'; MAX_RESPONSE_BYTES + 1]));
    let failure = metrics_from_response(response, Instant::now())
        .await
        .unwrap_err();
    assert_eq!(failure.code, WakeExecutionErrorCode::ResponseTooLarge);
    assert!(!failure.retryable);

    let response = Response::new(Body::from("not-json-private-body"));
    let failure = metrics_from_response(response, Instant::now())
        .await
        .unwrap_err();
    assert_eq!(failure.code, WakeExecutionErrorCode::InvalidResponse);
    assert!(!serde_json::to_string(&failure)
        .unwrap()
        .contains("not-json-private-body"));
}

#[tokio::test]
async fn http_failures_have_typed_retryability_without_body_leaks() {
    for (status, code, retryable) in [
        (
            StatusCode::UNAUTHORIZED,
            WakeExecutionErrorCode::Unauthorized,
            false,
        ),
        (
            StatusCode::FORBIDDEN,
            WakeExecutionErrorCode::Forbidden,
            false,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            WakeExecutionErrorCode::RateLimited,
            true,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            WakeExecutionErrorCode::Upstream,
            true,
        ),
    ] {
        let response = Response::builder()
            .status(status)
            .body(Body::from("provider-secret-body"))
            .unwrap();
        let failure = metrics_from_response(response, Instant::now())
            .await
            .unwrap_err();
        assert_eq!(failure.code, code);
        assert_eq!(failure.retryable, retryable);
        assert_eq!(failure.http_status, Some(status.as_u16()));
        assert!(!serde_json::to_string(&failure)
            .unwrap()
            .contains("provider-secret-body"));
    }
}

#[test]
fn completion_helper_uses_only_metrics_error_and_verification() {
    let success = Ok(WakeExecutionMetrics {
        http_status: 200,
        latency_ms: 10,
        input_tokens: Some(1),
        output_tokens: Some(1),
        total_tokens: Some(2),
    });
    let confirmed = completion_from_execution(
        &success,
        WakeVerificationOutcome::ConfirmedCountdownAdvanced,
        100,
    );
    assert_eq!(confirmed.outcome, WakeCompletionOutcome::Confirmed);
    assert_eq!(confirmed.input_tokens, Some(1));

    let unconfirmed =
        completion_from_execution(&success, WakeVerificationOutcome::Unconfirmed, 100);
    assert_eq!(unconfirmed.outcome, WakeCompletionOutcome::Unconfirmed);

    let failure = Err(WakeExecutionFailure::runtime(
        WakeExecutionErrorCode::RateLimited,
        true,
        Some(429),
        12,
    ));
    let failed = completion_from_execution(&failure, WakeVerificationOutcome::Unconfirmed, 100);
    assert_eq!(failed.outcome, WakeCompletionOutcome::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("wake_rate_limited"));
    let serialized = format!("{failed:?}");
    assert!(!serialized.contains(ACCESS_TOKEN));
    assert!(!serialized.contains(PROVIDER_ACCOUNT_ID));
}
