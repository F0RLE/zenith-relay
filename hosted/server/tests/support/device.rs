use super::checkpoint;
use crate::support::Process;
use sqlx::PgPool;
use std::{fs, path::Path};
use uuid::Uuid;
use zenith_relay_hosted_device::{Destination, DeviceKey};
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
    let code = k.approval_code();
    let other = DeviceKey::generate(target).unwrap();
    assert_ne!(other.approval_code(), code);
    super::age_current_passkey(db).await;
    fs::write(dir.join("device-review"), &code).unwrap();
    checkpoint(browser, &dir.join("device-approved")).await;
    assert!(k.connections(None).await.unwrap().items.is_empty());
    assert_eq!(
        other.connections(None).await.err(),
        Some("device_not_authorized")
    );
    limits::check(db, origin, k.target().device()).await;

    let raw = k.protected_record();
    let secret = raw.split_once('\n').unwrap().1;
    let (iid, device) = invitation
        .split_once("relay-device=")
        .unwrap()
        .1
        .split_once('.')
        .unwrap();
    let bearer = format!("rhd1.{iid}.{device}.{secret}");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for path in [
        "admin/overview",
        "admin/connections",
        "admin/devices",
        "account",
        "models",
        "keys",
    ] {
        let req = client.get(format!("{origin}/api/hosted/v1/{path}"));
        assert_eq!(req.bearer_auth(&bearer).send().await.unwrap().status(), 401);
    }
    let read = || client.get(format!("{origin}/api/hosted/v1/device/connections"));
    let other_installation = format!("rhd1.{}.{device}.{secret}", Uuid::new_v4());
    for request in [
        read().bearer_auth(&code),
        read().bearer_auth(&bearer).header("origin", origin),
        read().bearer_auth(&bearer).header("cookie", "test=device"),
        read().bearer_auth(other_installation),
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
    // Restore only this disposable fixture's owner epoch before testing ordinary restarts.
    sqlx::query("UPDATE relay_hosted.users SET authorization_epoch=authorization_epoch-1 WHERE id=(SELECT owner_user_id FROM relay_hosted.installations)")
        .execute(db).await.unwrap();
    fs::write(dir.join("device-checked"), []).unwrap();
    k
}
