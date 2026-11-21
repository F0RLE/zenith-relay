use zenith_relay_hosted_device::{Destination, DeviceKey};

const I: &str = "7e06cc3f-88f2-4cdd-b6f4-5a120f68bcbe";
const D: &str = "f3bb7239-3d0f-4c02-bf17-69fbfb928208";

fn invite(origin: &str) -> String {
    format!("{origin}/admin/connections#relay-device={I}.{D}")
}

#[test]
fn invitations_are_canonical_and_http_is_loopback_only() {
    for origin in [
        "https://relay.example",
        "http://localhost:1443",
        "http://[::1]:1443",
        "http://127.0.0.1",
    ] {
        assert_eq!(
            Destination::parse(&invite(origin)).unwrap().invitation(),
            invite(origin)
        );
    }
    let valid = invite("https://relay.example");
    for bad in [
        invite("http://relay.example"),
        invite("https://user@relay.example"),
        invite("https://relay.example:443"),
        invite("https://Relay.Example"),
        invite("file://relay.example"),
        invite("http://127.1"),
        invite("https://relay.example/.."),
        valid.replace('#', "?x#"),
        valid.replace("/admin/", "/other/"),
        valid.replace(I, &I.to_uppercase()),
        valid.replace(D, "00000000-0000-0000-0000-000000000000"),
        format!("{valid}\n"),
    ] {
        assert!(Destination::parse(&bad).is_err());
    }
}

#[test]
fn approval_proves_a_device_key_but_never_authenticates_as_one() {
    let origin = "https://relay.example";
    let destination = Destination::parse(&invite(origin)).unwrap();
    let key = DeviceKey::generate(destination).unwrap();
    let mut record = key.protected_record();
    let restored = DeviceKey::restore(&record).unwrap();
    assert_eq!(key.approval_code(), restored.approval_code());
    let code = key.approval_code();
    assert_eq!(
        DeviceKey::review_proof(key.target().device(), &code),
        Some(key.verifier())
    );
    assert!(DeviceKey::presented(origin, &code).is_none());
    let secret = record.split_once('\n').unwrap().1;
    assert!(!code.contains(secret));
    let bearer = format!("rhd1.{I}.{D}.{secret}");
    assert_eq!(
        DeviceKey::presented(origin, &bearer).unwrap().2,
        key.verifier()
    );
    for (site, token) in [
        ("https://other.example", bearer.clone()),
        (origin, bearer.replace(D, I)),
        (origin, bearer.replace(I, D)),
    ] {
        assert_ne!(
            DeviceKey::presented(site, &token).unwrap().2,
            key.verifier()
        );
    }
    assert!(DeviceKey::review_proof(key.target().device(), &bearer).is_none());
    record.push('\n');
    assert!(DeviceKey::restore(&record).is_none());
}

#[tokio::test]
async fn native_inventory_accepts_only_the_reviewed_page_without_following_redirects() {
    use axum::{Json, Router, response::IntoResponse, routing::get};
    use serde_json::json;
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex, atomic::AtomicBool, atomic::Ordering},
    };
    let row = json!({"id":D,"label":null,"credential_state":"stored","proxy_bound":false,"verified_models":["m"]});
    let valid = json!({"data":{"installation_id":I,"device_id":D,"scope":"connections.read",
        "expires_at_s":1900000000,"items":[row],"next_after":null},"meta":{"api_version":"1","request_id":I}});
    let mut pages = VecDeque::from([Json(valid.clone()).into_response()]);
    for (pointer, replacement) in [
        ("/data/installation_id", json!(D)),
        ("/data/device_id", json!(I)),
        ("/data/scope", json!("unsupported")),
        ("/data/next_after", json!(D)),
        ("/data/items/0/verified_models", json!(["z", "a"])),
    ] {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(pointer).unwrap() = replacement;
        pages.push_back(Json(invalid).into_response());
    }
    for (object, field) in [("/data", "next_after"), ("/data/items/0", "label")] {
        let mut incomplete = valid.clone();
        incomplete
            .pointer_mut(object)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(field);
        pages.push_back(Json(incomplete).into_response());
    }
    pages.push_back("not a JSON response".into_response());
    pages.push_back(
        (
            [("content-type", "application/json")],
            vec![0u8; 768 * 1024 + 1],
        )
            .into_response(),
    );
    let bad_pages = pages.len() - 1;
    pages.push_back(axum::response::Redirect::temporary("/other").into_response());
    let pages = Arc::new(Mutex::new(pages));
    let followed = Arc::new(AtomicBool::new(false));
    let fixture = Router::new()
        .route(
            "/api/hosted/v1/device/connections",
            get({
                let pages = pages.clone();
                move || async move { pages.lock().unwrap().pop_front().unwrap() }
            }),
        )
        .route(
            "/other",
            get({
                let followed = followed.clone();
                move || async move { followed.store(true, Ordering::SeqCst) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let key = DeviceKey::generate(Destination::parse(&invite(&origin)).unwrap()).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, fixture).await.unwrap() });
    assert_eq!(key.connections(None).await.unwrap().items.len(), 1);
    for _ in 0..bad_pages {
        assert_eq!(
            key.connections(None).await.err(),
            Some("invalid_device_response")
        );
    }
    assert_eq!(
        key.connections(None).await.err(),
        Some("device_server_unavailable")
    );
    assert!(!followed.load(Ordering::SeqCst) && pages.lock().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn native_usage_rejects_owner_fields_and_a_mismatched_window() {
    use axum::{Json, Router, routing::get};
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use zenith_relay_hosted_device::UsageFilter;

    let request = json!({"request_id":D,"started_at_ms":1_900_000_000_001_i64,"model":"synthetic",
        "execution":"failed","tokens":null,"partial_tokens":false,"diagnostic_code":"hosted_dispatch_unavailable"});
    let valid = json!({"data":{"installation_id":I,"device_id":D,"expires_at_s":1_900_086_400_i64,
        "result":{"hours":24,"as_of_ms":1_900_000_000_002_i64,"model":null,"execution":null,"items":[request],"next_before":null}},
        "meta":{"api_version":"1","request_id":I}});
    let reads = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/api/hosted/v1/device/usage",
        get(move || {
            let mut body = valid.clone();
            match reads.fetch_add(1, Ordering::Relaxed) {
                1 => body["data"]["result"]["items"][0]["workspace_id"] = json!(I),
                2 => body["data"]["result"]["hours"] = json!(168),
                _ => {}
            };
            async move { Json(body) }
        }),
    );
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", socket.local_addr().unwrap());
    let key = DeviceKey::generate(Destination::parse(&invite(&origin)).unwrap()).unwrap();
    let server = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
    let f = UsageFilter {
        hours: 24,
        model: None,
        execution: None,
        as_of_ms: None,
        before: None,
    };
    assert_eq!(key.usage(&f).await.unwrap().result.items.len(), 1);
    for _ in 0..2 {
        assert_eq!(key.usage(&f).await.err(), Some("invalid_device_response"));
    }
    server.abort();
}

#[tokio::test]
async fn native_pool_preparation_binds_the_review_to_the_source_and_action() {
    use axum::{Json, Router, http::HeaderMap, routing::post};
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use zenith_relay_hosted_device::{PoolAction, PoolChange};

    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/api/hosted/v1/device/pool",
        post(move |headers: HeaderMap, Json(change): Json<PoolChange>| {
            assert!(headers.contains_key("authorization"));
            assert!(!headers.contains_key("origin") && !headers.contains_key("cookie"));
            assert_eq!(change.action.as_str(), "include");
            assert_eq!(change.source_id.to_string(), I);
            assert_eq!(change.expected_revision, "a0".repeat(32));
            let expiry = 1_900_086_400_i64;
            let mut data = json!({"installation_id":I,"device_id":D,"expires_at_s":expiry,
                "result":{"operation_id":D,"source_id":I,"action":"include","expires_at_s":expiry-120}});
            match calls.fetch_add(1, Ordering::Relaxed) {
                1 => data["result"]["source_id"] = json!(D),
                2 => data["result"]["action"] = json!("exclude"),
                3 => data["result"]["expires_at_s"] = json!(expiry + 1),
                _ => {}
            }
            async move { Json(json!({"data":data,"meta":{"api_version":"1","request_id":I}})) }
        }),
    );
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", socket.local_addr().unwrap());
    let key = DeviceKey::generate(Destination::parse(&invite(&origin)).unwrap()).unwrap();
    let server = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
    let mut change = PoolChange {
        source_id: I.parse().unwrap(),
        action: PoolAction::Include,
        expected_revision: "a0".repeat(32),
    };
    assert_eq!(
        key.prepare_pool_change(&change)
            .await
            .unwrap()
            .operation_id
            .to_string(),
        D
    );
    for _ in 0..3 {
        assert_eq!(
            key.prepare_pool_change(&change).await.err(),
            Some("invalid_device_response")
        );
    }
    change.expected_revision.make_ascii_uppercase();
    assert_eq!(
        key.prepare_pool_change(&change).await.err(),
        Some("invalid_pool_review")
    );
    server.abort();
}

#[tokio::test]
async fn native_overview_accepts_scoped_metrics_and_uses_legacy_only_on_404() {
    use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn snapshot() -> serde_json::Value {
        json!({"data":{"installation_id":I,"device_id":D,"expires_at_s":1_900_086_400_i64,
            "result":{"hours":24,"as_of_ms":1_900_000_000_000_i64,"requests":"10","errors":"0",
                "token_samples":"10","partial_tokens":"0","total_tokens":"502","active_requests":"2","active_attempts":"1"}},
            "meta":{"api_version":"1","request_id":I}})
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/api/hosted/v1/device/overview",
            get(|| async { Json(snapshot()) }),
        )
        .route(
            "/api/hosted/v1/device/overview/metrics",
            get(move || {
                let n = calls.fetch_add(1, Ordering::Relaxed);
                async move {
                    let mut body = snapshot();
                    let metrics = &mut body["data"]["result"]["metrics"];
                    *metrics = json!({"pool":{"members":"3","ready":"1","busy":"2"},
                    "generation":{"samples":"4","tokens_per_second":39.5}});
                    match n {
                        1 => return StatusCode::NOT_FOUND.into_response(),
                        2 => return StatusCode::UNAUTHORIZED.into_response(),
                        3 => *metrics = serde_json::Value::Null,
                        4 => metrics["pool"]["ready"] = json!("2"),
                        5 => metrics["generation"]["tokens_per_second"] = json!(0),
                        _ => {}
                    }
                    Json(body).into_response()
                }
            }),
        );
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", socket.local_addr().unwrap());
    let key = DeviceKey::generate(Destination::parse(&invite(&origin)).unwrap()).unwrap();
    let server = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
    let metrics = key.overview(24).await.unwrap().result.metrics.unwrap();
    assert_eq!(metrics.pool.unwrap().busy, "2");
    assert_eq!(metrics.generation.unwrap().tokens_per_second, 39.5);
    assert!(key.overview(24).await.unwrap().result.metrics.is_none());
    assert_eq!(key.overview(24).await.err(), Some("device_not_authorized"));
    for _ in 0..3 {
        assert_eq!(
            key.overview(24).await.err(),
            Some("invalid_device_response")
        );
    }
    server.abort();
}
