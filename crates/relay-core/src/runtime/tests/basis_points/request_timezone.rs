use super::*;

#[tokio::test]
async fn proxy_timezone_hot_updates_expire_without_inventing_a_fallback() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let runtime = runtime(&server).await;
    let environment = "<environment_context><current_date>2026-10-18</current_date><timezone>UTC</timezone></environment_context>";
    let request = || json!({"input":environment});
    let evidence = |timezone: &str, observed_at_ms| {
        Some(crate::request_timezone::ProxyTimezone {
            timezone: timezone.into(),
            observed_at_ms,
        })
    };
    for timezone in ["Europe/Amsterdam", "Asia/Tokyo"] {
        runtime
            .set_account_proxy_timezone("account-1", evidence(timezone, crate::unix_time_ms()))
            .unwrap();
        let mut body = request();
        runtime.apply_request_timezone("account-1", &mut body);
        assert!(body["input"]
            .as_str()
            .unwrap()
            .contains(&format!("<timezone>{timezone}</timezone>")));
        assert!(body["input"].as_str().unwrap().contains("2026-10-18"));
    }
    assert!(runtime
        .set_account_proxy_timezone("account-1", evidence("Invalid/Zone", crate::unix_time_ms()))
        .is_err());
    let mut body = request();
    runtime.apply_request_timezone("account-1", &mut body);
    assert!(body["input"].as_str().unwrap().contains("Asia/Tokyo"));
    runtime
        .set_account_proxy_timezone(
            "account-1",
            evidence(
                "Asia/Tokyo",
                crate::unix_time_ms() - crate::request_timezone::PROXY_TIMEZONE_MAX_AGE_MS,
            ),
        )
        .unwrap();
    let mut body = request();
    runtime.apply_request_timezone("account-1", &mut body);
    assert_eq!(body, request());
    runtime
        .set_account_proxy_timezone("account-1", None)
        .unwrap();
    runtime.apply_request_timezone("account-1", &mut body);
    assert_eq!(body, request());
    runtime
        .set_account_proxy_timezone(
            "not-an-account",
            evidence("Asia/Tokyo", crate::unix_time_ms()),
        )
        .unwrap();
    runtime.apply_request_timezone("not-an-account", &mut body);
    assert_eq!(body, request());
}
