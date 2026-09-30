use super::*;

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
    assert_eq!(hint.retry_after_ms, Some(120_000));
    assert!(hint.global);
}

#[test]
fn rate_limit_body_hint_accepts_relative_reset_seconds() {
    let hint = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"code":"rate_limit","resets_in_seconds":"17"}}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(hint.retry_after_ms, Some(17_000));
    assert!(!hint.global);
}

#[test]
fn rate_limit_body_hint_accepts_retry_after_and_message_delays() {
    let retry_after = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"code":"rate_limit_exceeded","retry_after":"2.5"}}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(retry_after.retry_after_ms, Some(2_500));

    let seconds = cooldown::rate_limit_body_hint_at(
            br#"{"response":{"error":{"code":"rate_limit_exceeded","message":"Please try again in 11.054s."}}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
    assert_eq!(seconds.retry_after_ms, Some(11_054));

    let millis = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"message":"Please try again in 250ms."}}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(millis.retry_after_ms, Some(250));
}

#[test]
fn rate_limit_body_hint_accepts_top_level_quota_variants() {
    let hint = cooldown::rate_limit_body_hint_at(
        br#"{"code":"rate_limit_reached","resets_in_seconds":9}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(hint.retry_after_ms, Some(9_000));
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
    assert_eq!(
        cooldown::retry_delay_ms(Some(1_000), Some(120_000), 1_000),
        120_000
    );
    assert_eq!(cooldown::retry_delay_ms(Some(0), None, 1_000), 0);
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
