use super::{Client, CodexSubscriptionClient, Url};
use crate::error_codes;

#[test]
fn subscription_client_rejects_unencrypted_remote_endpoints() {
    let secure = Url::parse("https://subscription.example.invalid/check").unwrap();
    let http = Client::new();
    for endpoint in [
        "http://subscription.example.invalid/check",
        "http://192.168.1.10/check",
        "http://[2001:db8::1]/check",
        "http://localhost.example.invalid/check",
        "http://127.0.0.1.example.invalid/check",
    ] {
        let insecure = Url::parse(endpoint).unwrap();
        for (accounts, subscriptions) in [
            (insecure.clone(), secure.clone()),
            (secure.clone(), insecure),
        ] {
            let result =
                CodexSubscriptionClient::with_endpoints(http.clone(), accounts, subscriptions);
            assert!(
                matches!(result, Err(ref error)
                    if error.code == error_codes::SUBSCRIPTION_CONFIGURATION && !error.retryable),
                "unencrypted remote endpoint must be rejected before sending credentials: {endpoint}"
            );
        }
    }
}

#[test]
fn subscription_client_accepts_https_and_loopback_endpoints() {
    let http = Client::new();
    for endpoint in [
        "https://subscription.example.invalid/check",
        "http://localhost:8123/check",
        "http://127.0.0.1:8123/check",
        "http://127.0.0.2:8123/check",
        "http://[::1]:8123/check",
    ] {
        let endpoint = Url::parse(endpoint).unwrap();
        assert!(
            CodexSubscriptionClient::with_endpoints(http.clone(), endpoint.clone(), endpoint)
                .is_ok()
        );
    }
}

#[test]
fn subscription_client_rejects_non_http_and_url_credentials() {
    let http = Client::new();
    for endpoint in [
        "file:///tmp/subscription",
        "ftp://subscription.example.invalid/check",
        "https://user:password@subscription.example.invalid/check",
        "http://user:password@127.0.0.1:8123/check",
    ] {
        let endpoint = Url::parse(endpoint).unwrap();
        assert!(
            CodexSubscriptionClient::with_endpoints(http.clone(), endpoint.clone(), endpoint)
                .is_err()
        );
    }
}
