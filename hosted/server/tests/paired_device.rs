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
