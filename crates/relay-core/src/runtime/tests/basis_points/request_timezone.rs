use super::*;

#[tokio::test]
async fn proxy_timezone_hot_updates_existing_runtime_and_preserves_explicit_policy() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let runtime = runtime(&server).await;
    let environment = "<environment_context><current_date>2026-10-18</current_date><timezone>UTC</timezone></environment_context>";
    let request = || json!({"input":environment});
    for timezone in ["Europe/Amsterdam", "Asia/Tokyo"] {
        runtime
            .set_account_proxy_timezone("account-1", Some(timezone.into()))
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
        .set_account_proxy_timezone("account-1", Some("Invalid/Zone".into()))
        .is_err());
    let mut body = request();
    runtime.apply_request_timezone("account-1", &mut body);
    assert!(body["input"].as_str().unwrap().contains("Asia/Tokyo"));
    runtime
        .set_request_timezone(crate::RequestTimezonePolicy {
            account_timezone_bindings: std::collections::BTreeMap::from([(
                "account-1".into(),
                false,
            )]),
            ..Default::default()
        })
        .unwrap();
    let mut body = request();
    runtime.apply_request_timezone("account-1", &mut body);
    assert_eq!(body, request());
    runtime.set_request_timezone(Default::default()).unwrap();
    runtime
        .set_account_proxy_timezone("account-1", None)
        .unwrap();
    runtime.apply_request_timezone("account-1", &mut body);
    assert_eq!(body, request());
    runtime
        .set_account_proxy_timezone("not-an-account", Some("Asia/Tokyo".into()))
        .unwrap();
    runtime.apply_request_timezone("not-an-account", &mut body);
    assert_eq!(body, request());
}
