use super::*;
use axum::{body::Body, extract::Request, http::StatusCode, response::Response, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[test]
fn failed_stats_keep_only_a_fenced_success_and_unsupported_discards_it() {
    let good =
        SourceProviderStats::empty(SourceStatsProvider::Sub2Api, SourceStatsStatus::Available)
            .observed(None, 123);
    let retained =
        SourceProviderStats::empty(SourceStatsProvider::Sub2Api, SourceStatsStatus::RateLimited)
            .observed(Some(&good), 456);
    assert_eq!(retained.as_of_ms, Some(123));
    assert!(retained.stale);
    assert_eq!(retained.refresh_error, Some(SourceStatsStatus::RateLimited));
    let unsupported = SourceProviderStats::empty(
        SourceStatsProvider::Unsupported,
        SourceStatsStatus::Unsupported,
    )
    .observed(Some(&good), 789);
    assert_eq!(unsupported.status, SourceStatsStatus::Unsupported);
    assert_eq!(unsupported.as_of_ms, None);
    assert!(!unsupported.stale);
}

#[test]
fn source_stats_scheduler_distinguishes_transient_failure_from_unsupported() {
    use crate::scheduler::refresh::{source_stats_outcome, RefreshOutcome};
    let available =
        SourceProviderStats::empty(SourceStatsProvider::Sub2Api, SourceStatsStatus::Available);
    assert_eq!(
        source_stats_outcome(&available, 100),
        RefreshOutcome::Success
    );
    let failed =
        SourceProviderStats::empty(SourceStatsProvider::Sub2Api, SourceStatsStatus::RateLimited);
    assert_eq!(
        source_stats_outcome(&failed, 100),
        RefreshOutcome::FailedRetryAt(60_100)
    );
    let unsupported = SourceProviderStats::empty(
        SourceStatsProvider::Unsupported,
        SourceStatsStatus::Unsupported,
    );
    assert_eq!(
        source_stats_outcome(&unsupported, 100),
        RefreshOutcome::Unsupported
    );
}

#[tokio::test]
async fn rate_limited_stats_return_retry_after_even_without_a_valid_payload() {
    use axum::{routing::get, Router};
    let app = Router::new().route(
        "/v1/usage",
        get(|| async {
            (
                StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", "2")],
                "not JSON",
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let read = read_source_provider_stats(&format!("http://{address}/v1"), "synthetic-key").await;
    assert_eq!(read.value.unwrap().status, SourceStatsStatus::RateLimited);
    assert!(read.retry_after_ms.is_some_and(|delay| delay >= 1_000));
    server.abort();
}

#[test]
fn amounts_preserve_zero_debt_decimal_precision_and_bounds() {
    for (input, expected) in [
        (json!(0), Some(0)),
        (json!("-1.25"), Some(-1_250_000)),
        (json!("0.0000005"), Some(1)),
        (json!("-4e-7"), Some(0)),
        (json!("9223372036854.775807"), Some(i64::MAX)),
        (json!("-9223372036854.775808"), Some(i64::MIN)),
        (json!("9223372036854.775808"), None),
        (json!("1e100"), None),
        (json!("NaN"), None),
        (json!("--1"), None),
        (json!(null), None),
    ] {
        assert_eq!(
            formats::amount(Some(&input), 1_000_000),
            expected,
            "{input}"
        );
    }
}

#[test]
fn sub2api_wallet_uses_actual_charges_and_preserves_missing_spend() {
    let mut payload = json!({"mode":"unrestricted", "unit":"USD", "balance":"12.34",
        "usage":{"total":{"cost":90, "actual_cost":"8.25", "requests":123, "total_tokens":"456"}}});
    let stats = formats::sub2api_stats(&payload).unwrap();
    assert_eq!(stats.balance_micro_usd, Some(12_340_000));
    assert_eq!(stats.spent_micro_usd, Some(8_250_000));
    assert_eq!((stats.requests, stats.total_tokens), (Some(123), Some(456)));
    payload["usage"]["total"]
        .as_object_mut()
        .unwrap()
        .remove("actual_cost");
    payload["balance"] = json!(-1);
    let stats = formats::sub2api_stats(&payload).unwrap();
    assert_eq!(stats.spent_micro_usd, None);
    assert_eq!(stats.balance_micro_usd, Some(-1_000_000));
    assert!(!stats.balance_unlimited);
    assert_eq!(
        formats::sub2api_stats(&json!({"balance":100})),
        Err(SourceStatsStatus::Unsupported)
    );
}

#[test]
fn sub2api_key_quota_and_subscription_are_distinct_from_wallet() {
    let stats = formats::sub2api_stats(&json!({"mode":"quota_limited", "quota":{"limit":100,"used":20,"remaining":80,"unit":"USD"}})).unwrap();
    assert_eq!(stats.balance_kind, SourceBalanceKind::KeyQuota);
    assert_eq!(stats.balance_micro_usd, Some(80_000_000));
    assert_eq!(stats.spent_micro_usd, None);
    let stats = formats::sub2api_stats(
        &json!({"mode":"unrestricted", "unit":"USD", "remaining":-1,"subscription":{}}),
    )
    .unwrap();
    assert_eq!(stats.balance_kind, SourceBalanceKind::Subscription);
    assert!(stats.balance_unlimited);
    assert_eq!(stats.balance_micro_usd, None);
    let stats = formats::sub2api_stats(
        &json!({"mode":"quota_limited", "rate_limits":[{"remaining":4},{"remaining":0}]}),
    )
    .unwrap();
    assert!(!stats.balance_unlimited);
    assert_eq!(stats.balance_micro_usd, Some(0));

    let stats = formats::sub2api_stats(&json!({
        "mode":"unrestricted", "unit":"USD", "remaining":7,
        "subscription":{"daily_limit_usd":10,"daily_usage_usd":3,"monthly_limit_usd":100,"monthly_usage_usd":25}
    })).unwrap();
    assert_eq!(stats.balance_kind, SourceBalanceKind::Subscription);
    assert_eq!(stats.balance_micro_usd, Some(7_000_000));
}

#[test]
fn sub2api_does_not_infer_subscription_allowance_from_incomplete_windows() {
    let payload = json!({
        "mode":"unrestricted", "unit":"USD",
        "subscription":{"daily_limit_usd":0,"daily_usage_usd":0,"monthly_limit_usd":100},
        "usage":{"total":{"actual_cost":2}}
    });
    let stats = formats::sub2api_stats(&payload).unwrap();
    assert_eq!(stats.balance_kind, SourceBalanceKind::Subscription);
    assert_eq!(stats.balance_micro_usd, None);
    assert!(!stats.balance_unlimited);
    assert_eq!(stats.spent_micro_usd, Some(2_000_000));
}

#[test]
fn new_api_uses_advertised_conversion_and_never_guesses_quota_divisor() {
    let payload = json!({"code":true,"data":{"object":"token_usage","total_available":3_000_000,"total_used":600_000,"unlimited_quota":false}});
    let metadata =
        json!({"success":true,"data":{"quota_per_unit":600_000,"quota_display_type":"CNY"}});
    let stats = formats::new_api_stats(&payload, Some(&metadata)).unwrap();
    assert_eq!(stats.balance_micro_usd, Some(5_000_000));
    assert_eq!(stats.spent_micro_usd, Some(1_000_000));
    let stats = formats::new_api_stats(&payload, None).unwrap();
    assert_eq!(stats.balance_micro_usd, None);
    assert_eq!(stats.amounts[0].currency, SourceStatsCurrency::Credits);
    assert_eq!(stats.amounts[0].balance_micros, Some(3_000_000_000_000));
    let stats = formats::new_api_stats(&json!({"code":true,"data":{"object":"token_usage","unlimited_quota":true,"total_available":0}}), None).unwrap();
    assert!(stats.balance_unlimited);
    assert_eq!(stats.balance_kind, SourceBalanceKind::KeyQuota);
    assert!(stats.amounts.is_empty());
    for rejected in [
        json!({"code":false,"data":{"object":"token_usage","total_available":1}}),
        json!({"code":true,"success":false,"data":{"object":"token_usage","total_available":1}}),
        json!({"code":"true","data":{"object":"token_usage","total_available":1}}),
    ] {
        assert_eq!(
            formats::new_api_stats(&rejected, None),
            Err(SourceStatsStatus::InvalidResponse)
        );
    }
    assert!(formats::new_api_stats(
        &json!({"success":true,"data":{"object":"token_usage","total_used":1}}),
        None
    )
    .is_ok());
}

#[test]
fn billing_converts_cents_and_respects_display_currency() {
    let subscription = json!({"object":"billing_subscription","hard_limit_usd":"12.50"});
    let usage = json!({"object":"list","total_usage":"225"});
    let usd_metadata = json!({"success":true,"data":{"quota_display_type":"USD"}});
    let stats = formats::billing_stats(&subscription, &usage, Some(&usd_metadata)).unwrap();
    assert_eq!(stats.balance_micro_usd, Some(10_250_000));
    assert_eq!(stats.spent_micro_usd, Some(2_250_000));
    assert_eq!(
        formats::billing_stats(&subscription, &usage, None),
        Err(SourceStatsStatus::InvalidResponse)
    );
    for (meta, currency) in [
        (
            json!({"quota_display_type":"CNY"}),
            SourceStatsCurrency::Cny,
        ),
        (
            json!({"display_in_currency":false}),
            SourceStatsCurrency::Credits,
        ),
    ] {
        let stats = formats::billing_stats(
            &subscription,
            &usage,
            Some(&json!({"success":true,"data":meta})),
        )
        .unwrap();
        assert_eq!(stats.balance_micro_usd, None);
        assert_eq!(stats.amounts[0].currency, currency);
    }
}

#[test]
fn openrouter_inference_key_does_not_invent_wallet_or_reset_balance() {
    let stats = formats::openrouter_key_stats(
        &json!({"data":{"limit":100,"usage":500,"limit_remaining":74.5}}),
    )
    .unwrap();
    assert_eq!(stats.balance_kind, SourceBalanceKind::KeyQuota);
    assert_eq!(stats.balance_micro_usd, Some(74_500_000));
    let stats = formats::openrouter_key_stats(&json!({"data":{"limit":100,"usage":500}})).unwrap();
    assert_eq!(stats.balance_micro_usd, None);
    let stats = formats::openrouter_key_stats(&json!({"data":{"limit":null,"usage":0}})).unwrap();
    assert!(stats.balance_unlimited);
    assert_eq!(stats.balance_micro_usd, None);
}

#[test]
fn deepseek_preserves_each_currency() {
    let stats = formats::deepseek_stats(&json!({"is_available":true,"balance_infos":[
        {"currency":"CNY","total_balance":"110.00"}, {"currency":"USD","total_balance":"2.50"}]}))
    .unwrap();
    assert_eq!(stats.amounts.len(), 2);
    assert_eq!(stats.balance_micro_usd, Some(2_500_000));
    assert_eq!(stats.amounts[0].balance_micros, Some(110_000_000));
}

#[tokio::test]
async fn siliconflow_retired_balance_endpoint_is_not_probed() {
    let (base, seen, task) = mock_server(vec![]).await;
    let client = StatsClient::new(&base, "synthetic").unwrap();
    assert_eq!(
        fetch_stats(&client, SourceStatsProvider::SiliconFlow).await,
        Err(SourceStatsStatus::Unsupported)
    );
    assert!(seen.lock().unwrap().is_empty());
    task.abort();
}

#[test]
fn old_server_stats_remain_readable() {
    let stats: SourceProviderStats =
        serde_json::from_value(json!({"provider":"zenith","balanceMicroUsd":0,
        "spentMicroUsd":0,"requests":null,"totalTokens":null}))
        .unwrap();
    assert_eq!(stats.status, SourceStatsStatus::Available);
    assert!(stats.amounts.is_empty());
}

#[test]
fn endpoints_keep_origin_prefix_and_remove_query_credentials() {
    let client = StatsClient::new(
        "https://example.test/proxy/v1?secret=synthetic#fragment",
        "synthetic",
    )
    .unwrap();
    assert_eq!(
        client.endpoint("usage", false).unwrap().as_str(),
        "https://example.test/proxy/v1/usage"
    );
    assert_eq!(
        client.endpoint("api/usage/token/", true).unwrap().as_str(),
        "https://example.test/proxy/api/usage/token/"
    );
    assert!(client.endpoint("https://elsewhere.test/", true).is_err());
    assert!(StatsClient::new("https://user:synthetic@example.test/v1", "synthetic").is_err());
    assert!(StatsClient::new("http://example.test/v1", "synthetic").is_err());
    let copied = StatsClient::new("https://example.test/proxy/v1/models", "synthetic").unwrap();
    assert_eq!(
        copied.endpoint("usage", false).unwrap().path(),
        "/proxy/v1/usage"
    );
}

type ObservedRequests = Arc<Mutex<Vec<(String, bool)>>>;

async fn mock_server(
    responses: Vec<(&'static str, u16, Value)>,
) -> (String, ObservedRequests, tokio::task::JoinHandle<()>) {
    let observed: ObservedRequests = Arc::default();
    let seen = observed.clone();
    let app = Router::new().fallback(move |request: Request| {
        let responses = responses.clone();
        let seen = seen.clone();
        async move {
            let path = request.uri().path();
            seen.lock().unwrap().push((
                path.to_owned(),
                request.headers().contains_key("authorization"),
            ));
            let (status, body) = responses
                .iter()
                .find(|(expected, _, _)| *expected == path)
                .map(|(_, status, body)| (*status, body.to_string()))
                .unwrap_or((404, "{}".to_owned()));
            Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .header("location", "/credential-redirect")
                .body(Body::from(body))
                .unwrap()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}/proxy/v1"), observed, task)
}

#[tokio::test]
async fn custom_sub2api_is_discovered_without_name_or_dashboard_requests() {
    let (base, seen, task) = mock_server(vec![(
        "/proxy/v1/usage",
        200,
        json!({"mode":"unrestricted","balance":12.34,"unit":"USD"}),
    )])
    .await;
    let stats = fetch_source_provider_stats(&base, "synthetic")
        .await
        .unwrap();
    assert_eq!(stats.provider, SourceStatsProvider::Sub2Api);
    assert_eq!(stats.balance_micro_usd, Some(12_340_000));
    assert_eq!(
        *seen.lock().unwrap(),
        [("/proxy/v1/usage".to_owned(), true)]
    );
    task.abort();
}

#[tokio::test]
async fn missing_usage_falls_back_to_new_api_and_metadata_is_public() {
    let (base, seen, task) = mock_server(vec![
        (
            "/proxy/api/usage/token/",
            200,
            json!({"code":true,"data":{"object":"token_usage","total_available":5,"total_used":0}}),
        ),
        (
            "/proxy/api/status",
            200,
            json!({"success":true,"data":{"quota_per_unit":10}}),
        ),
    ])
    .await;
    let stats = fetch_source_provider_stats(&base, "synthetic")
        .await
        .unwrap();
    assert_eq!(stats.provider, SourceStatsProvider::NewApi);
    assert_eq!(stats.balance_micro_usd, Some(500_000));
    assert_eq!(
        seen.lock().unwrap().last(),
        Some(&("/proxy/api/status".to_owned(), false))
    );
    task.abort();
}

#[tokio::test]
async fn rate_limit_stops_probing_and_denials_are_not_unsupported() {
    for (code, expected, count) in [
        (429, SourceStatsStatus::RateLimited, 1),
        (401, SourceStatsStatus::Unauthorized, 4),
    ] {
        let (base, seen, task) = mock_server(vec![("/proxy/v1/usage", code, json!({}))]).await;
        let stats = fetch_source_provider_stats(&base, "synthetic")
            .await
            .unwrap();
        assert_eq!(stats.status, expected);
        assert_eq!(seen.lock().unwrap().len(), count);
        task.abort();
    }
}

#[tokio::test]
async fn redirects_are_not_followed_and_oversized_payload_is_rejected() {
    let (base, seen, task) = mock_server(vec![("/proxy/v1/usage", 302, json!({}))]).await;
    let stats = fetch_source_provider_stats(&base, "synthetic")
        .await
        .unwrap();
    assert_eq!(stats.status, SourceStatsStatus::Unsupported);
    assert!(!seen
        .lock()
        .unwrap()
        .iter()
        .any(|(path, _)| path.contains("redirect")));
    task.abort();
    let (base, _, task) = mock_server(vec![(
        "/proxy/v1/usage",
        StatusCode::OK.as_u16(),
        json!({"padding":"x".repeat(1024 * 1024)}),
    )])
    .await;
    assert_eq!(
        fetch_source_provider_stats(&base, "synthetic")
            .await
            .unwrap()
            .status,
        SourceStatsStatus::InvalidResponse
    );
    task.abort();
}

#[tokio::test]
async fn billing_discovery_uses_matching_versioned_paths_and_display_units() {
    let (base, seen, task) = mock_server(vec![
        ("/proxy/v1/dashboard/billing/subscription", 200, json!({"object":"billing_subscription","hard_limit_usd":25})),
        ("/proxy/v1/dashboard/billing/usage", 200, json!({"object":"list","total_usage":450})),
        ("/proxy/api/status", 200, json!({"success":true,"data":{"quota_display_type":"CNY","display_token_stat_enabled":true}})),
    ]).await;
    let stats = fetch_source_provider_stats(&base, "synthetic")
        .await
        .unwrap();
    assert_eq!(stats.provider, SourceStatsProvider::Billing);
    assert_eq!(stats.balance_kind, SourceBalanceKind::KeyQuota);
    assert_eq!(stats.balance_micro_usd, None);
    assert_eq!(stats.amounts[0].balance_micros, Some(20_500_000));
    assert!(seen
        .lock()
        .unwrap()
        .iter()
        .all(|(path, _)| path.starts_with("/proxy/")));
    task.abort();
}

#[tokio::test]
async fn billing_without_status_does_not_guess_usd_for_unknown_site_currency() {
    let (base, _, task) = mock_server(vec![
        (
            "/proxy/v1/dashboard/billing/subscription",
            200,
            json!({"object":"billing_subscription","hard_limit_usd":25}),
        ),
        (
            "/proxy/v1/dashboard/billing/usage",
            200,
            json!({"object":"list","total_usage":450}),
        ),
    ])
    .await;
    let stats = fetch_source_provider_stats(&base, "synthetic")
        .await
        .unwrap();
    assert_eq!(stats.provider, SourceStatsProvider::Billing);
    assert_eq!(stats.status, SourceStatsStatus::InvalidResponse);
    assert_eq!(stats.balance_micro_usd, None);
    task.abort();
}

#[tokio::test]
async fn openrouter_queries_credits_only_for_a_management_key() {
    for management in [false, true] {
        let (base, seen, task) = mock_server(vec![
            ("/proxy/v1/key", 200, json!({"data":{"limit":50,"limit_remaining":20,"usage":30,"is_management_key":management}})),
            ("/proxy/v1/credits", 200, json!({"data":{"total_credits":80,"total_usage":30}})),
        ]).await;
        let stats = fetch_stats(
            &StatsClient::new(&base, "synthetic").unwrap(),
            SourceStatsProvider::OpenRouter,
        )
        .await
        .unwrap();
        assert_eq!(
            stats.balance_micro_usd,
            Some(if management { 50_000_000 } else { 20_000_000 })
        );
        assert_eq!(seen.lock().unwrap().len(), if management { 2 } else { 1 });
        task.abort();
    }
}
