use super::{
    SETUP,
    support::{Cluster, Process},
};
use reqwest::Client;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{fs, path::Path, process::Command, time::Duration};

pub async fn verify(
    c: &Cluster,
    cfg: &Path,
    origin: &str,
    db: &PgPool,
    http: &Client,
    serve: &impl Fn() -> Process,
) {
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web");
    let root = tempfile::tempdir_in(c.temp.path()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let setup_with = |settings: &Path, dir: &Path| {
        let mut cmd = c.cli(SETUP, settings);
        cmd.args(["setup-owner", "--web-dist"])
            .arg(web.join("dist"))
            .arg("--code-dir")
            .arg(dir);
        cmd
    };
    let setup = || setup_with(cfg, root.path());
    let get_file = || {
        fs::read_dir(root.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
    };
    let request = |step: &str, token: &str, site: &str, data: Value| {
        http.post(format!("{origin}/api/hosted/v1/setup/{step}"))
            .bearer_auth(token)
            .header("origin", site)
            .json(&data)
            .send()
    };
    let has_owner = || {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM relay_hosted.admin_assignments")
            .fetch_one(db)
    };
    let page = format!("{origin}/setup/owner");
    let mut child = Process(setup().spawn().unwrap());
    super::support::ready(&mut child, http, &page).await;
    let path = get_file();
    let code = fs::read_to_string(&path).unwrap();
    assert_eq!(
        request("start", &"0".repeat(64), origin, json!({}))
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        request("start", &code, "https://foreign.example", json!({}))
            .await
            .unwrap()
            .status(),
        403
    );
    let r = request("start", &code, origin, json!({}))
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let session = r["data"]["session"].as_str().unwrap();
    assert_eq!(
        request("start", &code, origin, json!({}))
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        request("register", session, origin, json!({}))
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        request("register", session, origin, json!({}))
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(has_owner().await.unwrap(), 0);
    drop(child);
    // SIGKILL can leave a file. Its code must not survive a process restart.
    if path.is_file() {
        fs::remove_file(path).unwrap();
    }
    let mut child = Process(setup().spawn().unwrap());
    super::support::ready(&mut child, http, &page).await;
    assert_eq!(
        request("start", &code, origin, json!({}))
            .await
            .unwrap()
            .status(),
        403
    );
    let browser = || {
        let mut cmd = Command::new("bun");
        cmd.current_dir(&web)
            .args(["tests/owner-passkey.mjs", origin])
            .arg(get_file());
        cmd
    };
    let used = get_file();
    assert!(
        browser()
            .arg("invalid-signature")
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(has_owner().await.unwrap(), 0);
    drop(child);
    if used.is_file() {
        fs::remove_file(used).unwrap();
    }
    let mut child = Process(setup().spawn().unwrap());
    super::support::ready(&mut child, http, &page).await;
    let other = tempfile::tempdir_in(c.temp.path()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(other.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let other_cfg = c.temp.path().join("second-setup.toml");
    let bind = format!("127.0.0.1:{}", super::support::port());
    let toml = fs::read_to_string(cfg).unwrap().replace(
        &format!("127.0.0.1:{}", origin.rsplit(':').next().unwrap()),
        &bind,
    );
    fs::write(&other_cfg, toml).unwrap();
    let mut competitor = Process(setup_with(&other_cfg, other.path()).spawn().unwrap());
    let other_origin = format!("http://{bind}");
    super::support::ready(
        &mut competitor,
        http,
        &format!("{other_origin}/setup/owner"),
    )
    .await;
    let other_file = fs::read_dir(other.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut authenticators = Process(
        browser()
            .args(["race", &other_origin])
            .arg(other_file)
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            assert!(
                authenticators.0.try_wait().unwrap().is_none(),
                "browser stopped before setup"
            );
            if let (Some(a), Some(b)) = (
                child.0.try_wait().unwrap(),
                competitor.0.try_wait().unwrap(),
            ) {
                assert_ne!(a.success(), b.success());
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(has_owner().await.unwrap(), 1);
    let grant = sqlx::query_scalar::<_, bool>(
        "SELECT i.setup_state='claimed' AND i.authorization_epoch=2 AND s.mode='setup' AND s.admission_epoch=1
         FROM relay_hosted.installations i JOIN relay_hosted.service_control s ON i.id=s.installation_id
         JOIN relay_hosted.admin_credentials c ON c.user_id=i.owner_user_id",
    ).fetch_one(db).await.unwrap();
    assert!(grant);
    assert!(!setup().output().unwrap().status.success());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    assert_eq!(fs::read_dir(other.path()).unwrap().count(), 0);
    // Keep both ephemeral authenticators alive across the change to the normal listener.
    let mut normal = serve();
    super::support::ready(
        &mut normal,
        http,
        &format!("{origin}/api/hosted/v1/public/config"),
    )
    .await;
    let guard_ready = c.temp.path().join("restart-owner-guard");
    tokio::time::timeout(Duration::from_secs(60), async {
        while !guard_ready.exists() {
            assert!(authenticators.0.try_wait().unwrap().is_none());
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("browser did not finish sign-in/logout checks");
    let session_count = || {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM relay_hosted.admin_sessions")
            .fetch_one(db)
    };
    assert_eq!(session_count().await.unwrap(), 1);
    // A competing process cannot clear the session without owning the listener.
    let mut duplicate = serve();
    assert!(!duplicate.0.wait().unwrap().success());
    assert_eq!(session_count().await.unwrap(), 1);
    drop(normal);
    let mut restarted = serve();
    super::support::ready(
        &mut restarted,
        http,
        &format!("{origin}/api/hosted/v1/public/config"),
    )
    .await;
    assert!(authenticators.0.wait().unwrap().success());
    assert_eq!(session_count().await.unwrap(), 0);
    verify_session_fences(db, origin).await;
}

async fn verify_session_fences(db: &PgPool, origin: &str) {
    let verifier = [27_u8; 32]; // Opaque DB test value; never accepted as a browser cookie.
    for change in [
        "UPDATE relay_hosted.admin_sessions SET last_seen_at=now()-interval '21 minutes'",
        "UPDATE relay_hosted.admin_sessions SET created_at=now()-interval '9 hours',expires_at=now()-interval '1 second'",
        "UPDATE relay_hosted.installations SET authorization_epoch=authorization_epoch+1",
        "UPDATE relay_hosted.users SET authorization_epoch=authorization_epoch+1",
        "UPDATE relay_hosted.users SET disabled_at=now()",
        "UPDATE relay_hosted.admin_assignments SET revoked_at=now()",
        "UPDATE relay_hosted.admin_credentials SET revoked_at=now()",
    ] {
        let mut tx = db.begin().await.unwrap();
        sqlx::query("INSERT INTO relay_hosted.admin_sessions SELECT $1,user_id,credential_id,
            installation_epoch,user_epoch,now(),now(),now()+interval '8 hours' FROM relay_hosted.active_owner_credentials")
            .bind(verifier.as_slice()).execute(&mut *tx).await.unwrap();
        let read = || {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT relay_hosted.read_admin_overview($1,$2)",
            )
            .bind(origin)
            .bind(verifier.as_slice())
        };
        assert_eq!(
            read().fetch_one(&mut *tx).await.unwrap().as_deref(),
            Some("setup")
        );
        let analytics = |origin: &str| {
            sqlx::query_scalar::<_, Option<serde_json::Value>>(
                "SELECT relay_hosted.owner_usage_overview($1,$2,24,NULL)",
            )
            .bind(origin.to_owned())
            .bind(verifier.as_slice())
        };
        let last_seen =
            "SELECT last_seen_at::text FROM relay_hosted.admin_sessions WHERE verifier=$1";
        let stamp = || sqlx::query_scalar::<_, String>(last_seen).bind(verifier.as_slice());
        let before = stamp().fetch_one(&mut *tx).await.unwrap();
        let analytics_read = analytics(origin)
            .fetch_one(&mut *tx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(analytics_read["totals"]["requests"], "0");
        assert_eq!(stamp().fetch_one(&mut *tx).await.unwrap(), before);
        assert!(
            analytics("https://foreign.example")
                .fetch_one(&mut *tx)
                .await
                .unwrap()
                .is_none()
        );
        sqlx::query(change).execute(&mut *tx).await.unwrap();
        assert!(
            analytics(origin)
                .fetch_one(&mut *tx)
                .await
                .unwrap()
                .is_none()
        );
        assert!(read().fetch_one(&mut *tx).await.unwrap().is_none());
        tx.rollback().await.unwrap();
    }
}
