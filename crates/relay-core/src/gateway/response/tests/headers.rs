use super::super::{attach_request_id, proxy_json_response, proxy_response, proxy_sse_response};
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Response};

type Factory = fn(reqwest::StatusCode, &HeaderMap, Body) -> Response<Body>;

const FACTORIES: [Factory; 3] = [proxy_response, proxy_json_response, proxy_sse_response];

#[test]
fn relay_request_identity_replaces_owned_headers_without_changing_provider_identity() {
    let mut response = Response::new(Body::empty());
    response.headers_mut().append(
        "x-zenith-relay-request-id",
        HeaderValue::from_static("first"),
    );
    response.headers_mut().append(
        "x-zenith-relay-request-id",
        HeaderValue::from_static("second"),
    );
    response
        .headers_mut()
        .insert("request-id", HeaderValue::from_static("provider-request"));
    attach_request_id(&mut response, " synthetic-relay-request ");
    assert_eq!(
        response.headers()["x-zenith-relay-request-id"],
        "synthetic-relay-request"
    );
    assert_eq!(
        response
            .headers()
            .get_all("x-zenith-relay-request-id")
            .iter()
            .count(),
        1
    );
    assert_eq!(response.headers()["request-id"], "provider-request");
}

#[test]
fn invalid_relay_request_identity_never_becomes_a_response_header() {
    for request_id in ["", " ", "synthetic\nrequest", "\u{1f642}", &"x".repeat(129)] {
        let mut response = Response::new(Body::empty());
        attach_request_id(&mut response, request_id);
        assert!(!response.headers().contains_key("x-zenith-relay-request-id"));
    }
}

#[test]
fn permitted_response_headers_preserve_order_and_opaque_values() {
    let mut upstream = HeaderMap::new();
    upstream.append("cache-control", HeaderValue::from_static("private"));
    upstream.append("cache-control", HeaderValue::from_static("no-store"));
    upstream.append("x-ratelimit-policy", HeaderValue::from_static("first"));
    upstream.append(
        "x-ratelimit-policy",
        HeaderValue::from_bytes(b"second;opaque=\x80").unwrap(),
    );
    for factory in FACTORIES {
        let response = factory(reqwest::StatusCode::OK, &upstream, Body::empty());
        for name in ["cache-control", "x-ratelimit-policy"] {
            assert_eq!(
                response.headers().get_all(name).iter().collect::<Vec<_>>(),
                upstream.get_all(name).iter().collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn response_connection_options_do_not_cross_any_response_boundary() {
    let mut upstream = HeaderMap::new();
    upstream.append(
        "connection",
        HeaderValue::from_static("Keep-Alive, Retry-After"),
    );
    upstream.append(
        "connection",
        HeaderValue::from_bytes(b"x-ratelimit-remaining-tokens,\x80").unwrap(),
    );
    upstream.append("connection", HeaderValue::from_static("content-type"));
    upstream.insert("retry-after", HeaderValue::from_static("12"));
    upstream.insert(
        "x-ratelimit-remaining-tokens",
        HeaderValue::from_static("99"),
    );
    upstream.insert(
        "content-type",
        HeaderValue::from_static("application/x-synthetic"),
    );
    upstream.insert("request-id", HeaderValue::from_static("synthetic-request"));
    upstream.insert("set-cookie", HeaderValue::from_static("synthetic=session"));
    for (index, factory) in FACTORIES.into_iter().enumerate() {
        let response = factory(reqwest::StatusCode::OK, &upstream, Body::empty());
        for name in [
            "connection",
            "retry-after",
            "x-ratelimit-remaining-tokens",
            "set-cookie",
        ] {
            assert!(
                !response.headers().contains_key(name),
                "{name} crossed response boundary"
            );
        }
        assert_eq!(response.headers()["request-id"], "synthetic-request");
        match index {
            0 => assert!(!response.headers().contains_key("content-type")),
            1 => assert_eq!(response.headers()["content-type"], "application/json"),
            _ => assert_eq!(response.headers()["content-type"], "text/event-stream"),
        }
    }
}

#[test]
fn credential_field_names_cannot_bypass_the_response_prefix_allowlist() {
    for name in ["x-ratelimit-api-key", "anthropic-ratelimit-api-key"] {
        let mut upstream = HeaderMap::new();
        upstream.insert(name, HeaderValue::from_static("synthetic-key"));
        for factory in FACTORIES {
            assert!(
                !factory(reqwest::StatusCode::OK, &upstream, Body::empty())
                    .headers()
                    .contains_key(name)
            );
        }
    }
}

#[test]
fn ambiguous_response_singletons_are_not_forwarded_or_chosen_arbitrarily() {
    for name in [
        "content-type",
        "retry-after",
        "request-id",
        "x-request-id",
        "x-should-retry",
        "openai-processing-ms",
    ] {
        let mut upstream = HeaderMap::new();
        upstream.append(name, HeaderValue::from_static("first"));
        upstream.append(name, HeaderValue::from_static("second"));
        for (index, factory) in FACTORIES.into_iter().enumerate() {
            let response = factory(reqwest::StatusCode::OK, &upstream, Body::empty());
            if name == "content-type" && index != 0 {
                assert_eq!(response.headers().get_all(name).iter().count(), 1);
                assert_eq!(
                    response.headers()[name],
                    if index == 1 {
                        "application/json"
                    } else {
                        "text/event-stream"
                    }
                );
            } else {
                assert!(
                    !response.headers().contains_key(name),
                    "ambiguous {name} was forwarded"
                );
            }
        }
    }
}
