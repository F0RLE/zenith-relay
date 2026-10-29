use super::*;
use serde_json::json;

fn body() -> serde_json::Value {
    json!({
        "success": true, "ip": "203.0.113.9", "country_code": "NL",
        "region": "North Holland", "city": "Amsterdam",
        "timezone": {"id": "Europe/Amsterdam"},
    })
}

#[test]
fn observed_exit_has_automatic_city_and_validated_timezone() {
    let result = parse_location(
        &serde_json::to_vec(&body()).unwrap(),
        "203.0.113.9".parse().unwrap(),
        Some("NL"),
    )
    .unwrap();
    assert_eq!(result.country_code, "NL");
    assert_eq!(result.city.as_deref(), Some("Amsterdam"));
    assert_eq!(result.region.as_deref(), Some("North Holland"));
    assert_eq!(result.timezone.as_deref(), Some("Europe/Amsterdam"));
}

#[test]
fn rotating_exit_and_conflicting_countries_cannot_mix_geography() {
    let bytes = serde_json::to_vec(&body()).unwrap();
    assert!(parse_location(&bytes, "203.0.113.10".parse().unwrap(), Some("NL")).is_err());
    assert_eq!(
        parse_location(&bytes, "203.0.113.9".parse().unwrap(), Some("US")).err(),
        Some("proxy_location_mismatch")
    );
}

#[test]
fn timezone_and_country_are_not_guessed() {
    let mut value = body();
    value["timezone"]["id"] = json!("+03:00");
    value["city"] = json!("invalid\ncity");
    let ip = "203.0.113.9".parse().unwrap();
    let result = parse_location(&serde_json::to_vec(&value).unwrap(), ip, Some("NL")).unwrap();
    assert!(result.timezone.is_none());
    assert!(result.city.is_none());
    value["country_code"] = json!("XX");
    assert!(parse_location(&serde_json::to_vec(&value).unwrap(), ip, None).is_err());
}

#[tokio::test]
async fn enrichment_uses_the_selected_proxy_and_the_explicit_observed_ip() {
    use axum::{Json, Router, http::HeaderMap, routing::get};
    let app = Router::new().route(
        "/203.0.113.9",
        get(|headers: HeaderMap| async move {
            assert!(headers.contains_key("proxy-authorization"));
            assert!(!headers.contains_key("authorization"));
            Json(body())
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = ProxyConfig::parse(&format!(
        "http://synthetic:secret@{}",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let result = lookup(
        &proxy,
        "203.0.113.9".parse().unwrap(),
        Some("NL"),
        "http://unreachable.invalid/",
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    assert_eq!(result.city.as_deref(), Some("Amsterdam"));
    assert!(
        lookup(
            &proxy,
            "203.0.113.10".parse().unwrap(),
            Some("NL"),
            "http://unreachable.invalid/",
            Duration::from_secs(1)
        )
        .await
        .is_err()
    );
    server.abort();
}
