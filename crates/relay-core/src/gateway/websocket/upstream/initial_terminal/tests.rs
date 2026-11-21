use super::*;
use crate::gateway::errors::{RateLimitBodyHint, rate_limit_body_hint_value};
use crate::gateway::websocket::failure::gateway_error_event;
use axum::http::{HeaderMap, HeaderValue};
use serde_json::{Value, json};
use std::time::{Duration, UNIX_EPOCH};

fn terminal(payload: Value) -> EventTerminal {
    EventTerminal {
        body_hint: rate_limit_body_hint_value(
            &payload,
            UNIX_EPOCH + Duration::from_millis(1_700_000_000_900),
        ),
        ..Default::default()
    }
}

#[test]
fn initial_rejection_retains_the_normalized_retry_deadline_in_the_client_error() {
    let terminal = terminal(json!({"error":{"retry_after":30}}));
    let failure = classified_terminal_failure(
        StatusCode::TOO_MANY_REQUESTS,
        error_codes::UPSTREAM_RATE_LIMITED,
        ErrorOrigin::Provider,
        &terminal,
    );
    assert_eq!(failure.retry_at_ms, Some(1_700_000_030_900));
    assert_eq!(failure.category, error_codes::UPSTREAM_RATE_LIMITED);
    assert_eq!(failure.origin, ErrorOrigin::Provider);
    let frame = gateway_error_event(&failure, Some("synthetic-request"), Some("synthetic-lane"));
    assert_eq!(frame["retry_at_ms"], 1_700_000_030_900_u64);
    assert_eq!(frame["stream_id"], "synthetic-lane");
    assert_eq!(frame["error"]["zenith_relay"]["origin"], "provider");
    assert_eq!(
        frame["error"]["zenith_relay"]["category"],
        "upstream_rate_limited"
    );
}

#[test]
fn initial_rejection_does_not_borrow_subscription_resets_for_an_api_source() {
    for explicit in [None, Some(7)] {
        let terminal = terminal(json!({"error":{
            "code":"usage_limit_reached", "resets_in_seconds":3600, "retry_after":explicit
        }}));
        for origin in [ErrorOrigin::Account, ErrorOrigin::Provider] {
            let failure = classified_terminal_failure(
                StatusCode::TOO_MANY_REQUESTS,
                error_codes::UPSTREAM_QUOTA_EXHAUSTED,
                origin,
                &terminal,
            );
            let expected = if origin == ErrorOrigin::Account {
                Some(1_700_003_600_900)
            } else {
                explicit.map(|seconds| 1_700_000_000_900 + seconds * 1_000)
            };
            assert_eq!(failure.retry_at_ms, expected);
            assert_eq!(failure.origin, origin);
            assert_eq!(failure.category, error_codes::UPSTREAM_QUOTA_EXHAUSTED);
        }
    }
}

#[test]
fn initial_rejection_keeps_a_normalized_provider_retry_header() {
    let headers = HeaderMap::from_iter([(
        axum::http::header::RETRY_AFTER,
        HeaderValue::from_static("45"),
    )]);
    let terminal = EventTerminal {
        body_hint: RateLimitBodyHint::default().with_retry_headers_at(
            &headers,
            UNIX_EPOCH + Duration::from_millis(1_700_000_000_900),
        ),
        ..Default::default()
    };
    let failure = classified_terminal_failure(
        StatusCode::TOO_MANY_REQUESTS,
        error_codes::UPSTREAM_RATE_LIMITED,
        ErrorOrigin::Provider,
        &terminal,
    );
    assert_eq!(failure.retry_at_ms, Some(1_700_000_045_900));
}

#[test]
fn retry_evidence_neither_changes_an_unrelated_failure_nor_shortens_an_existing_pause() {
    let terminal = terminal(json!({"retry_after":30}));
    let failure = classified_terminal_failure(
        StatusCode::SERVICE_UNAVAILABLE,
        error_codes::UPSTREAM_UNAVAILABLE,
        ErrorOrigin::Provider,
        &terminal,
    );
    assert_eq!(failure.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(failure.retry_at_ms, None);
    let failure = GatewayFailure::cooldown(1_700_000_060_900)
        .with_retry_hint(terminal.body_hint)
        .with_retry_hint(terminal.body_hint);
    assert_eq!(failure.retry_at_ms, Some(1_700_000_060_900));
}
