use super::*;
use axum::http::HeaderValue;

#[tokio::test]
async fn token_count_error_retry_headers_keep_the_shared_response_boundary() {
    for protocol in [WireApi::Messages, WireApi::Gemini] {
        for scenario in 0..4 {
            let mut upstream = Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header(RETRY_AFTER, "12")
                .header("set-cookie", "synthetic=session")
                .header("x-private", "synthetic-private")
                .body(reqwest::Body::from("synthetic private provider body"))
                .unwrap();
            match scenario {
                1 => {
                    upstream
                        .headers_mut()
                        .append(RETRY_AFTER, HeaderValue::from_static("120"));
                }
                2 => {
                    upstream
                        .headers_mut()
                        .append("connection", HeaderValue::from_static("keep-alive"));
                    upstream.headers_mut().append(
                        "connection",
                        HeaderValue::from_bytes(b"Retry-After,\x80").unwrap(),
                    );
                }
                3 => {
                    upstream
                        .headers_mut()
                        .insert("connection", HeaderValue::from_static("keep-alive"));
                }
                _ => {}
            }
            let response = response(upstream.into(), protocol).await;
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
            assert_eq!(
                response.headers().contains_key(RETRY_AFTER),
                matches!(scenario, 0 | 3)
            );
            for name in ["connection", "set-cookie", "x-private"] {
                assert!(
                    !response.headers().contains_key(name),
                    "{name} crossed the error boundary"
                );
            }
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            assert!(
                !String::from_utf8(body.to_vec())
                    .unwrap()
                    .contains("synthetic private")
            );
        }
    }
}
