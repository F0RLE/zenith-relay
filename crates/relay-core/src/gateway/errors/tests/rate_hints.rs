use super::*;

fn observed_delay(hint: RateLimitBodyHint) -> Option<u64> {
    hint.retry_clock.map(|clock| {
        clock.not_before_ms
            - clock
                .observed_at_ms
                .expect("parsed hint has its observation time")
    })
}

#[test]
fn retry_after_supports_delta_seconds_and_http_dates() {
    let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(RETRY_AFTER, reqwest::header::HeaderValue::from_static("17"));
    assert_eq!(cooldown::retry_after_ms(&headers, now), Some(17_000));

    headers.insert(
        RETRY_AFTER,
        reqwest::header::HeaderValue::from_static("518400"),
    );
    assert_eq!(cooldown::retry_after_ms(&headers, now), Some(518_400_000));

    let date = httpdate::fmt_http_date(now + Duration::from_secs(23));
    headers.insert(RETRY_AFTER, date.parse().unwrap());
    assert_eq!(cooldown::retry_after_ms(&headers, now), Some(23_000));
}

#[test]
fn rate_limit_body_hint_uses_reset_time_and_marks_usage_limits_global() {
    let hint = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"type":"usage_limit_reached","resets_at":1700000120,"resets_in_seconds":1}}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(observed_delay(hint), Some(120_000));
    assert!(hint.global);
}

#[test]
fn absolute_reset_hints_preserve_millisecond_precision() {
    let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_900);
    for (reset, expected_ms) in [
        (serde_json::json!(1_700_000_001), 100),
        (serde_json::json!("1700000001"), 100),
        (serde_json::json!(1_700_000_001_125_u64), 225),
        (serde_json::json!("1700000001125"), 225),
        (serde_json::json!(1_700_000_000_950_u64), 50),
        (serde_json::json!("1700000000950"), 50),
    ] {
        let payload = serde_json::json!({"error":{"resets_at":reset,"resets_in_seconds":17}});
        let hint = cooldown::rate_limit_body_hint_value(&payload, now);
        assert_eq!(observed_delay(hint), Some(expected_ms), "{reset}");
    }
}

#[test]
fn expired_reset_hints_use_relative_evidence_without_underflow() {
    let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_900);
    for reset in [
        serde_json::json!(1_700_000_000),
        serde_json::json!(1_700_000_000_899_u64),
        serde_json::json!(1_700_000_000_900_u64),
    ] {
        let payload = serde_json::json!({"error":{"resets_at":reset}});
        assert_eq!(
            observed_delay(cooldown::rate_limit_body_hint_value(&payload, now)),
            None
        );
        let payload = serde_json::json!({"error":{"resets_at":reset,"retry_after":2.5}});
        assert_eq!(
            observed_delay(cooldown::rate_limit_body_hint_value(&payload, now)),
            Some(2_500)
        );
    }
    let distant = serde_json::json!({"resets_at":u64::MAX});
    assert_eq!(
        observed_delay(cooldown::rate_limit_body_hint_value(&distant, now)),
        Some(MAX_RATE_LIMIT_RETRY_HINT_MS),
    );
}

#[test]
fn rate_limit_body_hint_accepts_relative_reset_seconds() {
    let hint = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"code":"rate_limit","resets_in_seconds":"17"}}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(observed_delay(hint), Some(17_000));
    assert!(!hint.global);
}

#[test]
fn rate_limit_body_hint_accepts_retry_after_and_message_delays() {
    let retry_after = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"code":"rate_limit_exceeded","retry_after":"2.5"}}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(observed_delay(retry_after), Some(2_500));

    let seconds = cooldown::rate_limit_body_hint_at(
            br#"{"response":{"error":{"code":"rate_limit_exceeded","message":"Please try again in 11.054s."}}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
    assert_eq!(observed_delay(seconds), Some(11_054));

    let millis = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"message":"Please try again in 250ms."}}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(observed_delay(millis), Some(250));
}

#[test]
fn rate_limit_body_hint_accepts_top_level_quota_variants() {
    let hint = cooldown::rate_limit_body_hint_at(
        br#"{"code":"rate_limit_reached","resets_in_seconds":9}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(observed_delay(hint), Some(9_000));
    assert!(hint.global);
}

#[test]
fn quota_exhaustion_scope_is_global_without_a_global_body_hint() {
    assert_eq!(
        cooldown::rate_limit_scope("upstream_quota_exhausted", false, "gpt-5"),
        "*"
    );
    assert_eq!(
        cooldown::rate_limit_scope("upstream_rate_limited", false, "gpt-5"),
        "gpt-5"
    );
}

#[test]
fn websocket_connection_limit_is_account_global() {
    let hint = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"code":"websocket_connection_limit_reached"}}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert!(hint.global);
}

#[test]
fn rate_limit_delay_uses_the_stronger_hint_and_keeps_explicit_zero() {
    let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(RETRY_AFTER, HeaderValue::from_static("1"));
    let hint = cooldown::rate_limit_body_hint_at(br#"{"retry_after":120}"#, now)
        .with_retry_headers_at(&headers, now);
    assert_eq!(
        hint.retry_clock_for_account(true).unwrap().not_before_ms,
        1_700_000_120_000,
    );
    headers.insert(RETRY_AFTER, HeaderValue::from_static("0"));
    let hint = RateLimitBodyHint::default().with_retry_headers_at(&headers, now);
    assert_eq!(
        hint.retry_clock_for_account(true).unwrap().not_before_ms,
        1_700_000_000_000,
    );
}

#[test]
fn source_recovery_delay_overrides_automatic_but_not_provider_retry_after() {
    assert_eq!(
        cooldown::source_cooldown_ms(5_000, Some(60_000), false),
        60_000
    );
    assert_eq!(
        cooldown::source_cooldown_ms(120_000, Some(60_000), true),
        120_000
    );
    assert_eq!(cooldown::source_cooldown_ms(5_000, None, false), 5_000);
}

#[test]
fn organization_rate_limit_requires_explicit_model_and_quota_identity() {
    let error = serde_json::json!({
        "code": "rate_limit_exceeded", "type": "tokens",
        "message": "Rate limit reached for synthetic-model in organization org-synthetic on tokens per min (TPM): Limit 1000, Used 1000, Requested 100. Please try again in 2ms."
    });
    let hint = |value: &Value| cooldown::rate_limit_body_hint_value(value, UNIX_EPOCH);
    let expected = hint(&serde_json::json!({"error": error}));
    assert!(expected.shared_bucket.is_some());
    assert!(!expected.global);
    assert_eq!(observed_delay(expected), Some(2));
    for value in [
        error.clone(),
        serde_json::json!({"body": {"error": error}}),
        serde_json::json!({"response": {"error": error}}),
    ] {
        assert_eq!(hint(&value).shared_bucket, expected.shared_bucket);
    }
    for (from, to) in [
        ("org-synthetic", "org-independent"),
        ("synthetic-model", "different-model"),
    ] {
        let mut other = error.clone();
        other["message"] = Value::String(error["message"].as_str().unwrap().replace(from, to));
        assert_ne!(hint(&other).shared_bucket, expected.shared_bucket);
    }
    for message in [
        "Too many requests. Please try again in 2ms.",
        "Rate limit reached for synthetic-model in organization  on tokens per min",
        "Rate limit reached for synthetic-model in organization org/synthetic on tokens per min",
        "Request too large for synthetic-model in organization org-synthetic on tokens per min",
    ] {
        let mut other = error.clone();
        other["message"] = Value::String(message.into());
        assert!(hint(&other).shared_bucket.is_none());
    }
    let mut other = error.clone();
    other["code"] = serde_json::json!("usage_limit_reached");
    assert!(hint(&other).shared_bucket.is_none());
    assert!(hint(&other).global);
}
#[test]
fn quota_rejection_and_rate_throttling_keep_distinct_wait_reasons() {
    use crate::scheduler::CooldownReason;

    for (category, expected) in [
        (
            crate::error_codes::UPSTREAM_QUOTA_EXHAUSTED,
            CooldownReason::QuotaExhausted,
        ),
        ("upstream_rate_limited", CooldownReason::RateLimit),
    ] {
        let reason = super::super::cooldown::failure_cooldown_reason(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            category,
            true,
        );
        assert_eq!(reason, expected);
        assert!(reason.is_rate_limit());
    }
}
