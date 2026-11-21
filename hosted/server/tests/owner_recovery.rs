#[allow(dead_code)]
#[path = "support/payment_fixture.rs"]
mod payment;
mod support;

use serde_json::json;
use sqlx::PgPool;
use std::{fs, path::Path, process::Command, time::Duration};
use support::{Cluster, Process, fixture_files::setup_code, ready};
use uuid::Uuid;
use zenith_relay_hosted::{config::Config, db, payments::reconcile_paid};

const RUNTIME: &str = "fixture_runtime";
const RECOVERY: &str = "fixture_recovery";
const SETUP: &str = "fixture_setup";
const RECOVERY_PAGE: &str = "/setup/recovery";

#[tokio::test]
#[ignore = "requires disposable PostgreSQL, built web assets and Chromium"]
async fn local_recovery_replaces_authority_once_without_changing_customer_money() {
    let cluster = Cluster::new();
    let admin = PgPool::connect(&cluster.dsn("fixture_boot")).await.unwrap();
    cluster
        .create_logins(&admin, &[SETUP, RECOVERY, RUNTIME])
        .await;
    let port = support::port();
    let other_port = (0..).map(|_| support::port()).find(|p| *p != port).unwrap();
    let origin = format!("http://localhost:{port}");
    let other_origin = format!("http://127.0.0.1:{other_port}");
    let root = cluster.temp.path();
    let config_path = root.join("server.toml");
    let other_config = root.join("other-server.toml");
    let toml = include_str!("../config.example.toml").replace("1443", &port.to_string());
    fs::write(&config_path, &toml).unwrap();
    fs::write(
        &other_config,
        toml.replace(
            &format!("127.0.0.1:{port}"),
            &format!("127.0.0.1:{other_port}"),
        ),
    )
    .unwrap();
    let config = Config::parse(&toml).unwrap();
    db::migrate(&admin, &origin).await.unwrap();
    grant_roles(&admin).await;
    let runtime = db::connect(&config, &cluster.dsn(RUNTIME)).await.unwrap();
    let installation: Uuid = sqlx::query_scalar("SELECT id FROM relay_hosted.installations")
        .fetch_one(&admin)
        .await
        .unwrap();
    let (operation, other_operation) = (Uuid::new_v4(), Uuid::new_v4());
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web");
    let dist = web.join("dist");
    let setup_dir = tempfile::tempdir_in(root).unwrap();
    let first_dir = tempfile::tempdir_in(root).unwrap();
    let second_dir = tempfile::tempdir_in(root).unwrap();
    let exchange = tempfile::tempdir_in(root).unwrap();
    #[cfg(unix)]
    for dir in [&setup_dir, &first_dir, &second_dir] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let recover = |role: &str, cfg: &Path, code: &Path, iid: Uuid, op: Uuid| {
        let mut cmd = cluster.cli(role, cfg);
        cmd.arg("recover-owner")
            .arg("--installation")
            .arg(iid.to_string())
            .arg("--operation")
            .arg(op.to_string())
            .arg("--web-dist")
            .arg(&dist)
            .arg("--code-dir")
            .arg(code);
        cmd
    };
    let recovery = || {
        recover(
            RECOVERY,
            &config_path,
            first_dir.path(),
            installation,
            operation,
        )
    };
    assert!(!recovery().output().unwrap().status.success());

    let url = |path: &str| format!("{origin}{path}");
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let mut setup = Process(
        cluster
            .cli(SETUP, &config_path)
            .args(["setup-owner", "--web-dist"])
            .arg(&dist)
            .arg("--code-dir")
            .arg(setup_dir.path())
            .spawn()
            .unwrap(),
    );
    ready(&mut setup, &http, &url("/setup/owner")).await;
    let mut browser = Process(
        Command::new("bun")
            .current_dir(&web)
            .args(["tests/owner-passkey.mjs", &origin])
            .arg(setup_code(setup_dir.path()))
            .arg("recover-owner")
            .arg(exchange.path())
            .spawn()
            .unwrap(),
    );
    assert!(setup.0.wait().unwrap().success());

    let serve = || {
        Process(
            cluster
                .cli(RUNTIME, &config_path)
                .args(["serve", "--web-dist"])
                .arg(&dist)
                .spawn()
                .unwrap(),
        )
    };
    let config_url = url("/api/hosted/v1/public/config");
    let mut server = serve();
    ready(&mut server, &http, &config_url).await;
    assert_eq!(
        http.get(url(RECOVERY_PAGE)).send().await.unwrap().status(),
        404
    );
    assert_eq!(
        http.post(url("/api/hosted/v1/setup/start"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    checkpoint(&mut browser, exchange.path(), "owner-session-ready").await;
    // The recovery listener must not publish a code while the regular listener owns its port.
    assert!(!recovery().output().unwrap().status.success());
    assert_eq!(fs::read_dir(first_dir.path()).unwrap().count(), 0);
    drop(server);
    for (role, iid) in [
        (RUNTIME, installation),
        (SETUP, installation),
        (RECOVERY, Uuid::new_v4()),
    ] {
        assert!(
            !recover(role, &config_path, first_dir.path(), iid, operation)
                .output()
                .unwrap()
                .status
                .success()
        );
    }

    let before = owner_state(&admin).await;
    assert_eq!((before.2, before.3, before.4), (1, 1, 0));
    let device = paired_device_fixture(&admin).await;
    assert!(device_authorized(&runtime, &origin, installation, device).await);
    let workspace = funded_wallet(&admin).await;
    let service = service_state(&admin).await;
    let mut first = Process(recovery().spawn().unwrap());
    ready(&mut first, &http, &url(RECOVERY_PAGE)).await;
    let mut second = Process(
        recover(
            RECOVERY,
            &other_config,
            second_dir.path(),
            installation,
            other_operation,
        )
        .spawn()
        .unwrap(),
    );
    ready(
        &mut second,
        &http,
        &format!("{other_origin}{RECOVERY_PAGE}"),
    )
    .await;
    assert_eq!(
        http.get(url("/setup/owner")).send().await.unwrap().status(),
        404
    );
    fs::write(
        exchange.path().join("listeners.json"),
        json!({
            "first": setup_code(first_dir.path()),
            "second": setup_code(second_dir.path()),
            "otherOrigin": other_origin,
        })
        .to_string(),
    )
    .unwrap();
    fs::write(exchange.path().join("recovery-ready"), []).unwrap();
    checkpoint(&mut browser, exchange.path(), "replacement-staged").await;
    assert_eq!(owner_state(&admin).await, before);
    assert!(device_authorized(&runtime, &origin, installation, device).await);
    fs::write(exchange.path().join("confirm-replacement"), []).unwrap();

    let first_done = first.0.wait().unwrap().success();
    let second_done = second.0.wait().unwrap().success();
    assert_ne!(first_done, second_done);
    let committed = if first_done {
        operation
    } else {
        other_operation
    };
    // Check revocation before normal startup also invalidates web sessions.
    assert_eq!(owner_state(&admin).await, (before.0, before.1 + 1, 1, 0, 1));
    assert!(!device_authorized(&runtime, &origin, installation, device).await);
    assert_eq!(service_state(&admin).await, service);
    payment::assert_wallet(&admin, workspace, 10_000_000, 1).await;
    let receipt: (Uuid, Uuid, Uuid, i64) = sqlx::query_as(
        "SELECT operation_id,installation_id,owner_id,previous_owner_epoch FROM relay_hosted.owner_recoveries",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(receipt, (committed, installation, before.0, before.1));
    assert!(
        sqlx::query("DELETE FROM relay_hosted.owner_recoveries")
            .execute(&admin)
            .await
            .is_err()
    );
    let retry = recover(
        RECOVERY,
        &config_path,
        first_dir.path(),
        installation,
        committed,
    )
    .output()
    .unwrap();
    assert!(retry.status.success());
    assert_eq!(
        String::from_utf8(retry.stdout).unwrap().trim(),
        "Owner recovery previously committed; no changes."
    );
    for dir in [first_dir.path(), second_dir.path()] {
        assert_eq!(fs::read_dir(dir).unwrap().count(), 0);
    }

    let mut restarted = serve();
    ready(&mut restarted, &http, &config_url).await;
    assert!(browser.0.wait().unwrap().success());
    drop(restarted);
    runtime.close().await;
    admin.close().await;
    cluster.stop();
    cluster.start();
    let admin = PgPool::connect(&cluster.dsn("fixture_boot")).await.unwrap();
    assert_eq!(owner_state(&admin).await, (before.0, before.1 + 1, 1, 1, 1));
    assert!(
        recover(
            RECOVERY,
            &config_path,
            first_dir.path(),
            installation,
            committed,
        )
        .output()
        .unwrap()
        .status
        .success()
    );
    db::fence_restore(&admin, &origin, installation, Uuid::new_v4())
        .await
        .unwrap();
    assert!(
        !recover(
            RECOVERY,
            &config_path,
            first_dir.path(),
            installation,
            Uuid::new_v4()
        )
        .output()
        .unwrap()
        .status
        .success()
    );
}

async fn grant_roles(db: &PgPool) {
    sqlx::raw_sql(
        "GRANT USAGE ON SCHEMA relay_hosted TO fixture_runtime,fixture_setup;
         GRANT SELECT ON relay_hosted.closed_installation,relay_hosted._sqlx_migrations TO fixture_runtime,fixture_setup;
         GRANT SELECT ON relay_hosted.pending_owner TO fixture_setup;
         GRANT EXECUTE ON FUNCTION relay_hosted.bind_first_owner(uuid,bigint,text,uuid,bytea,jsonb,bigint) TO fixture_setup;",
    )
    .execute(db)
    .await
    .unwrap();
    for sql in [
        include_str!("../runtime-grants.sql").replace(":\"runtime_role\"", RUNTIME),
        include_str!("../owner-recovery-grants.sql").replace(":\"recovery_role\"", RECOVERY),
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(db)
            .await
            .unwrap();
    }
}

async fn checkpoint(child: &mut Process, root: &Path, name: &str) {
    tokio::time::timeout(Duration::from_secs(25), async {
        while !root.join(name).is_file() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "browser exited before {name}"
            );
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("owner recovery browser checkpoint");
}

async fn owner_state(db: &PgPool) -> (Uuid, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT u.id,u.authorization_epoch,
            (SELECT count(*) FROM relay_hosted.active_owner_credentials),
            (SELECT count(*) FROM relay_hosted.admin_sessions),
            (SELECT count(*) FROM relay_hosted.owner_recoveries)
         FROM relay_hosted.installations i JOIN relay_hosted.users u ON u.id=i.owner_user_id",
    )
    .fetch_one(db)
    .await
    .unwrap()
}

async fn service_state(db: &PgPool) -> (String, i64, i64, i64) {
    sqlx::query_as(
        "SELECT s.mode,s.revision,s.admission_epoch,i.authorization_epoch
         FROM relay_hosted.service_control s JOIN relay_hosted.installations i ON i.id=s.installation_id",
    )
    .fetch_one(db)
    .await
    .unwrap()
}

const DEVICE_VERIFIER: [u8; 32] = [61; 32];

async fn paired_device_fixture(db: &PgPool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO relay_hosted.management_devices(id,installation_id,owner_id,installation_epoch,
             owner_epoch,label,scope,revision,invited_at,approve_by,approved_at,expires_at,verifier)
         SELECT gen_random_uuid(),i.id,i.owner_user_id,i.authorization_epoch,u.authorization_epoch,
             'Fixture Desktop','connections.read',2,now(),now()+interval '5 minutes',now(),now()+interval '24 hours',$1
         FROM relay_hosted.installations i JOIN relay_hosted.users u ON u.id=i.owner_user_id RETURNING id",
    )
    .bind(DEVICE_VERIFIER.as_slice())
    .fetch_one(db)
    .await
    .unwrap()
}

async fn device_authorized(db: &PgPool, origin: &str, installation: Uuid, device: Uuid) -> bool {
    sqlx::query_scalar("SELECT relay_hosted.device_connections($1,$2,$3,$4,NULL) IS NOT NULL")
        .bind(origin)
        .bind(installation)
        .bind(device)
        .bind(DEVICE_VERIFIER.as_slice())
        .fetch_one(db)
        .await
        .unwrap()
}

async fn funded_wallet(db: &PgPool) -> Uuid {
    let workspace: Uuid = sqlx::query_scalar(
        "WITH person AS (INSERT INTO relay_hosted.users (id) VALUES (gen_random_uuid()) RETURNING id)
         INSERT INTO relay_hosted.workspaces (owner_user_id) SELECT id FROM person RETURNING id",
    )
    .fetch_one(db)
    .await
    .unwrap();
    let adapter = payment::Adapter::new(db, "sandbox", payment::Adapter::signing_key()).await;
    payment::invoice(db, workspace, adapter.binding, "owner-recovery", 10_000_000).await;
    let paid = adapter.paid("pre-recovery", "owner-recovery");
    reconcile_paid(db, &adapter, &paid).await.unwrap();
    workspace
}
