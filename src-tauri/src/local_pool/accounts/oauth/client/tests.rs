use super::*;
use axum::{extract::Request, routing::post, Json, Router};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn both_oauth_clients_refresh_through_the_selected_normalized_proxy() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let app = Router::new().route("/oauth/token", post(move |request: Request| {
        let captured = captured.clone();
        async move {
            let uri = request.uri().to_string();
            let headers = request.headers().clone();
            let body = axum::body::to_bytes(request.into_body(), 16 * 1024).await.unwrap();
            let content_type = headers.get("content-type").unwrap().to_str().unwrap();
            let payload: serde_json::Value = if content_type.starts_with("application/json") {
                serde_json::from_slice(&body).unwrap()
            } else {
                assert!(content_type.starts_with("application/x-www-form-urlencoded"));
                let fields: std::collections::HashMap<String, String> =
                    url::form_urlencoded::parse(&body).into_owned().collect();
                serde_json::to_value(fields).unwrap()
            };
            captured.lock().unwrap().push((uri, headers, payload));
            Json(json!({"access_token":"synthetic-access","expires_in":3600,"token_type":"Bearer"}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for address in [
        format!("http://synthetic-user:synthetic-secret@127.0.0.1:{port}"),
        format!("127.0.0.1:{port}:synthetic-user:synthetic-secret"),
    ] {
        let proxy = ProxyConfig::parse(&address).unwrap();
        for kind in [OAuthClientKind::Codex, OAuthClientKind::ExcelBps] {
            let official = CodexOAuthClient::new_with_proxy_for_kind(kind, Some(&proxy)).unwrap();
            assert_eq!(
                official.authorize_endpoint.host_str(),
                Some("auth.openai.com")
            );
            assert_eq!(official.token_endpoint.host_str(), Some("auth.openai.com"));
            let client = CodexOAuthClient::with_endpoints_and_proxy_for_kind(
                kind,
                Url::parse("http://authorization.invalid/oauth/authorize").unwrap(),
                Url::parse("http://authorization.invalid/oauth/token").unwrap(),
                Some(&proxy),
            )
            .unwrap();
            let result = client
                .exchange_refresh_token("synthetic-refresh", 1_000)
                .await
                .unwrap();
            assert_eq!(result.access_token(), "synthetic-access");
        }
    }
    let observed = requests.lock().unwrap();
    assert_eq!(observed.len(), 4);
    for (index, (uri, headers, payload)) in observed.iter().enumerate() {
        let uri = Url::parse(uri).unwrap();
        assert_eq!(uri.host_str(), Some("authorization.invalid"));
        assert_eq!(uri.path(), "/oauth/token");
        assert!(headers.contains_key("proxy-authorization"));
        assert!(!headers.contains_key("authorization"));
        let kind = if index % 2 == 0 {
            OAuthClientKind::Codex
        } else {
            OAuthClientKind::ExcelBps
        };
        let content_type = headers.get("content-type").unwrap().to_str().unwrap();
        assert_eq!(
            content_type.starts_with("application/json"),
            kind == OAuthClientKind::Codex
        );
        assert_eq!(
            uri.query_pairs()
                .any(|(key, value)| key == "unified" && value == "true"),
            kind == OAuthClientKind::ExcelBps
        );
        assert_eq!(payload["grant_type"], "refresh_token");
        assert_eq!(payload["client_id"], kind.client_id());
        assert_eq!(payload["refresh_token"], "synthetic-refresh");
    }
    drop(observed);
    server.abort();
}
