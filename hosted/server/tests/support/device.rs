use super::checkpoint;
use crate::support::Process;
use sqlx::PgPool;
use std::{fs, path::Path};
use uuid::Uuid;
use zenith_relay_hosted_device::{Destination, DeviceKey, ReadScope, UsageFilter};
#[path = "device_limits.rs"]
mod limits;

pub(super) async fn pair(
    browser: &mut Process,
    dir: &Path,
    origin: &str,
    db: &PgPool,
) -> DeviceKey {
    checkpoint(browser, &dir.join("device-invited")).await;
    let invitation = fs::read_to_string(dir.join("device-invited")).unwrap();
    assert!(invitation.starts_with(&format!("{origin}/admin/connections#")));
    let target = Destination::parse(&invitation).unwrap();
    let k = DeviceKey::generate(target.clone()).unwrap();
    assert_eq!(
        k.connections(None).await.err(),
        Some("device_not_authorized")
    );
    assert_eq!(k.access().await.err(), Some("device_not_authorized"));
    let code = k.approval_code();
    let other = DeviceKey::generate(target).unwrap();
    assert_ne!(other.approval_code(), code);
    super::age_current_passkey(db).await;
    fs::write(dir.join("device-review"), &code).unwrap();
    checkpoint(browser, &dir.join("device-approved")).await;
    assert!(k.connections(None).await.unwrap().items.is_empty());
    assert_eq!(
        k.connections_with_pool(None).await.unwrap().pool_access,
        Some(true)
    );
    assert_observations(&k).await;
    assert_eq!(
        other.connections(None).await.err(),
        Some("device_not_authorized")
    );
    limits::check(db, origin, k.target().device()).await;

    let bearer = device_bearer(&k);
    let secret = bearer.rsplit_once('.').unwrap().1;
    let device = k.target().device();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for path in [
        "admin/overview",
        "admin/usage/requests",
        "admin/payment-methods",
        "admin/connections",
        "admin/devices",
        "account",
        "models",
        "keys",
    ] {
        let req = client.get(format!("{origin}/api/hosted/v1/{path}"));
        assert_eq!(
            req.bearer_auth(bearer.as_str())
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    let read = || client.get(format!("{origin}/api/hosted/v1/device/connections"));
    let other_installation = format!("rhd1.{}.{device}.{secret}", Uuid::new_v4());
    for request in [
        read().bearer_auth(&code),
        read().bearer_auth(bearer.as_str()).header("origin", origin),
        read()
            .bearer_auth(bearer.as_str())
            .header("cookie", "test=device"),
        read().bearer_auth(&other_installation),
    ] {
        assert_eq!(request.send().await.unwrap().status(), 401);
    }
    let observe = || client.get(format!("{origin}/api/hosted/v1/device/usage?hours=24"));
    for request in [
        observe().bearer_auth(&code),
        observe()
            .bearer_auth(bearer.as_str())
            .header("origin", origin),
        observe()
            .bearer_auth(bearer.as_str())
            .header("cookie", "test=device"),
        observe().bearer_auth(&other_installation),
        observe()
            .bearer_auth(bearer.as_str())
            .header("authorization", "Bearer duplicated"),
    ] {
        assert_eq!(request.send().await.unwrap().status(), 401);
    }
    let changed=sqlx::query("UPDATE relay_hosted.users SET authorization_epoch=authorization_epoch+1 WHERE id=(SELECT owner_user_id FROM relay_hosted.installations)")
        .execute(db).await.unwrap();
    assert_eq!(changed.rows_affected(), 1);
    assert_eq!(
        k.connections(None).await.err(),
        Some("device_not_authorized")
    );
    assert_revoked(&k).await;
    // Restore only this disposable fixture's owner epoch before testing ordinary restarts.
    sqlx::query("UPDATE relay_hosted.users SET authorization_epoch=authorization_epoch-1 WHERE id=(SELECT owner_user_id FROM relay_hosted.installations)")
        .execute(db).await.unwrap();
    fs::write(dir.join("device-checked"), []).unwrap();
    k
}

fn device_bearer(key: &DeviceKey) -> zeroize::Zeroizing<String> {
    let record = key.protected_record();
    let secret = record.split_once('\n').unwrap().1;
    zeroize::Zeroizing::new(format!(
        "rhd1.{}.{}.{secret}",
        key.target().installation(),
        key.target().device()
    ))
}

fn usage_filter() -> UsageFilter {
    UsageFilter {
        hours: 24,
        model: None,
        execution: None,
        as_of_ms: None,
        before: None,
    }
}

pub(super) async fn assert_observations(k: &DeviceKey) {
    let access = k.access().await.unwrap();
    assert_eq!(access.result, [ReadScope::Overview, ReadScope::Usage]);
    let overview = k.overview(24).await.unwrap();
    let page = k.usage(&usage_filter()).await.unwrap();
    assert_eq!(access.expires_at_s, overview.expires_at_s);
    assert_eq!(page.expires_at_s, overview.expires_at_s);
    assert_eq!(overview.result.hours, 24);
}

pub(super) async fn assert_revoked(k: &DeviceKey) {
    assert_eq!(
        k.connections_with_pool(None).await.err(),
        Some("device_not_authorized")
    );
    assert_eq!(k.access().await.err(), Some("device_not_authorized"));
    assert_eq!(k.overview(24).await.err(), Some("device_not_authorized"));
    assert_eq!(
        k.usage(&usage_filter()).await.err(),
        Some("device_not_authorized")
    );
}

pub(super) async fn reject_customer_case(
    http: &reqwest::Client,
    origin: &str,
    key: &DeviceKey,
    workspace: Uuid,
) {
    let endpoint = format!("{origin}/api/hosted/v1/admin/customers/{workspace}");
    let bearer = device_bearer(key);
    let command = serde_json::json!({
        "operation_id": Uuid::new_v4(),
        "expected_revision": "1",
        "action": "close",
        "reason_code": "verified_customer_request"
    });
    for request in [
        http.get(&endpoint),
        http.post(&endpoint).json(&command),
        http.post(format!("{endpoint}/options")).json(&command),
    ] {
        assert_eq!(
            request
                .bearer_auth(bearer.as_str())
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
}
