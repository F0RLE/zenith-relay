use super::*;
use axum::{
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[test]
fn geolocation_accepts_only_a_valid_ip_and_iana_zone_not_country_or_offset_guesses() {
    let mut body = json!({"success":true,"ip":"203.0.113.9","country_code":"NL","timezone":{"id":" Europe/Amsterdam "}});
    let location = parse_location(&serde_json::to_vec(&body).unwrap()).unwrap();
    assert_eq!(location.ip.to_string(), "203.0.113.9");
    assert_eq!(location.country_code.as_deref(), Some("NL"));
    assert_eq!(location.timezone.as_deref(), Some("Europe/Amsterdam"));
    for timezone in [
        json!("Invalid/Zone"),
        json!("+03:00"),
        json!(null),
        json!(42),
    ] {
        body["timezone"]["id"] = timezone;
        assert!(parse_location(&serde_json::to_vec(&body).unwrap())
            .unwrap()
            .timezone
            .is_none());
    }
    for invalid in [
        json!({"success":false,"message":"synthetic private failure"}),
        json!({"success":true,"ip":"invalid"}),
        json!({"ip":"203.0.113.9"}),
    ] {
        assert!(parse_location(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
}

#[tokio::test]
async fn discovery_uses_the_proxy_and_only_falls_back_for_connectivity_not_a_zone_guess() {
    let trace_hits = Arc::new(AtomicUsize::new(0));
    let received = trace_hits.clone();
    let router = Router::new()
        .route("/geo", get(|headers: HeaderMap| async move {
            assert!(headers.contains_key("proxy-authorization"));
            assert!(!headers.contains_key("authorization"));
            Json(json!({"success":true,"ip":"203.0.113.9","country_code":"NL","timezone":{"id":"Europe/Amsterdam"}}))
        }))
        .route("/limited", get(|| async { StatusCode::TOO_MANY_REQUESTS }))
        .route("/auth", get(|| async { StatusCode::PROXY_AUTHENTICATION_REQUIRED }))
        .route("/trace", get(move || {
            received.fetch_add(1, Ordering::SeqCst);
            async { "ip=203.0.113.10\nloc=US\n" }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = ProxyConfig::parse(&format!(
        "http://synthetic:secret@{}",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let trace = "http://unreachable.invalid/trace";
    let location = lookup_at(
        &proxy,
        "http://unreachable.invalid/geo",
        trace,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert_eq!(location.timezone.as_deref(), Some("Europe/Amsterdam"));
    assert_eq!(trace_hits.load(Ordering::SeqCst), 0);
    let location = lookup_at(
        &proxy,
        "http://unreachable.invalid/limited",
        trace,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert_eq!(location.ip.to_string(), "203.0.113.10");
    assert_eq!(location.timezone, None);
    assert_eq!(trace_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        lookup_at(
            &proxy,
            "http://unreachable.invalid/auth",
            trace,
            Duration::from_secs(2)
        )
        .await
        .err(),
        Some(error_codes::PROXY_CHECK_AUTH_FAILED)
    );
    assert_eq!(trace_hits.load(Ordering::SeqCst), 1);
    server.abort();
}
