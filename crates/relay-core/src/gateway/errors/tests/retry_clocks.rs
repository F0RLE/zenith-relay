use super::*;
use crate::scheduler::ProviderClock;

#[test]
fn normalized_clocks_keep_error_results_compact() {
    assert!(std::mem::size_of::<AttemptFailure>() <= 128);
    assert_eq!(
        std::mem::size_of::<Result<(), Box<AttemptFailure>>>(),
        std::mem::size_of::<usize>(),
    );
}

fn observed() -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(1_700_000_000_900)
}

fn headers(value: &'static str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(RETRY_AFTER, HeaderValue::from_static(value));
    headers
}

#[test]
fn retry_headers_are_normalized_once_even_after_their_deadline() {
    let now = observed();
    for value in ["0", "10", "Tue, 14 Nov 2023 22:13:30 GMT"] {
        let headers = headers(value);
        let hint = RateLimitBodyHint::default().with_retry_headers_at(&headers, now);
        let expected = hint.retry_clock_for_account(true).unwrap();
        for elapsed in [1, 5, 20] {
            let later = hint.with_retry_headers_at(&headers, now + Duration::from_secs(elapsed));
            assert_eq!(later.retry_clock_for_account(true), Some(expected));
        }
    }
}

#[test]
fn header_and_body_hints_compare_deadlines_instead_of_relative_durations() {
    let now = observed();
    let hint = cooldown::rate_limit_body_hint_at(br#"{"retry_after":60}"#, now)
        .with_retry_headers_at(&headers("20"), now + Duration::from_secs(50));
    assert_eq!(
        hint.retry_clock_for_account(true),
        Some(ProviderClock::new(
            1_700_000_070_900,
            Some(1_700_000_050_900)
        ))
    );
}

#[test]
fn deferred_stream_hints_keep_the_selected_deadline_and_its_observation() {
    let now = observed();
    let mut hint = cooldown::rate_limit_body_hint_at(br#"{"retry_after":60}"#, now);
    let later =
        cooldown::rate_limit_body_hint_at(br#"{"retry_after":20}"#, now + Duration::from_secs(50));
    hint.merge_retry_clocks(later);
    let expected = ProviderClock::new(1_700_000_070_900, Some(1_700_000_050_900));
    assert_eq!(hint.retry_clock_for_account(true), Some(expected));
    hint.merge_retry_clocks(cooldown::rate_limit_body_hint_at(
        br#"{"retry_after":5}"#,
        now + Duration::from_secs(60),
    ));
    assert_eq!(hint.retry_clock_for_account(true), Some(expected));
    hint.merge_retry_clocks(cooldown::rate_limit_body_hint_at(
        br#"{"retry_after":10}"#,
        now + Duration::from_secs(60),
    ));
    assert_eq!(hint.retry_clock_for_account(true), Some(expected));
}

#[test]
fn relayed_subscription_clocks_keep_explicit_retry_evidence_separate() {
    let now = observed();
    for (header, expected_source_deadline) in [("5", 1_700_000_010_900), ("15", 1_700_000_018_900)]
    {
        let hint = cooldown::rate_limit_body_hint_at(
            br#"{"error":{"code":"usage_limit_reached","resets_in_seconds":3600,"retry_after":10}}"#,
            now,
        )
        .with_retry_headers_at(&headers(header), now + Duration::from_secs(3));
        assert_eq!(
            hint.retry_clock_for_account(false).unwrap().not_before_ms,
            expected_source_deadline
        );
        assert_eq!(
            hint.retry_clock_for_account(true).unwrap().not_before_ms,
            1_700_003_600_900
        );
    }
}

#[test]
fn outgoing_retry_after_uses_the_remaining_interval() {
    let observed = SystemTime::now() - Duration::from_secs(20);
    let hint = cooldown::rate_limit_body_hint_at(br#"{"retry_after":30}"#, observed);
    let failure = AttemptFailure::classified_with_hint(
        StatusCode::TOO_MANY_REQUESTS,
        error_codes::UPSTREAM_RATE_LIMITED,
        hint,
    );
    let response = crate::gateway::execution::attempt_error_response(
        failure,
        None,
        crate::ErrorOrigin::Provider,
        "synthetic-request",
    );
    let seconds = response.headers()[RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!((1..=10).contains(&seconds), "remaining delay: {seconds}");
}
