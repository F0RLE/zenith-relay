use super::*;
use reqwest::header::{HeaderMap, HeaderValue};
use std::time::{SystemTime, UNIX_EPOCH};

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

#[test]
fn singleton_reads_preserve_opaque_bytes_and_reject_even_identical_duplicates() {
    let mut headers = HeaderMap::new();
    assert!(single_header(&headers, "x-synthetic").is_none());
    let value = HeaderValue::from_bytes(b"opaque=\x80").unwrap();
    headers.append("x-synthetic", value.clone());
    assert_eq!(single_header(&headers, "x-synthetic"), Some(&value));
    headers.append("x-synthetic", value);
    assert!(single_header(&headers, "x-synthetic").is_none());
}

#[test]
fn retry_after_uses_the_longest_valid_field_without_an_order_dependency() {
    let date = httpdate::fmt_http_date(now() + Duration::from_secs(120));
    let values = ["3", "invalid", "0", date.as_str(), "60"];
    for reverse in [false, true] {
        let mut headers = HeaderMap::new();
        let mut values = values.to_vec();
        if reverse {
            values.reverse();
        }
        for value in values {
            headers.append("retry-after", value.parse().unwrap());
        }
        assert_eq!(retry_after_ms(&headers, now()), Some(120_000));
    }
}

#[test]
fn retry_after_requires_unsigned_decimal_seconds_or_one_complete_http_date() {
    for value in ["", "+1", "-1", "1.5", "1, 120", "invalid"] {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", value.parse().unwrap());
        assert_eq!(retry_after_ms(&headers, now()), None, "{value:?}");
    }
    let mut headers = HeaderMap::new();
    headers.append("retry-after", HeaderValue::from_bytes(b"\x80").unwrap());
    headers.append("retry-after", HeaderValue::from_static(" 00017\t"));
    assert_eq!(retry_after_ms(&headers, now()), Some(17_000));

    headers.clear();
    let date = httpdate::fmt_http_date(now() + Duration::from_secs(23));
    headers.insert("retry-after", date.parse().unwrap());
    assert_eq!(retry_after_ms(&headers, now()), Some(23_000));
    headers.insert("retry-after", format!("{date}, 120").parse().unwrap());
    assert_eq!(retry_after_ms(&headers, now()), None);
}

#[test]
fn retry_after_saturates_valid_unrepresentable_delays_without_losing_the_floor() {
    for value in [
        "18446744073709551615",
        "18446744073709551616",
        "99999999999999999999999999999999999999999999999999",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", value.parse().unwrap());
        assert_eq!(retry_after_ms(&headers, now()), Some(u64::MAX));
    }
}

#[test]
fn local_retry_floor_is_separate_from_downstream_connection_header_filtering() {
    let mut headers = HeaderMap::new();
    assert_eq!(retry_after_ms(&headers, now()), None);
    headers.insert("retry-after", HeaderValue::from_static("0"));
    assert_eq!(retry_after_ms(&headers, now()), Some(0));
    headers.insert("retry-after", HeaderValue::from_static("120"));
    headers.insert("connection", HeaderValue::from_static("Retry-After"));
    assert!(is_connection_header(&headers, "retry-after"));
    assert_eq!(retry_after_ms(&headers, now()), Some(120_000));
}
