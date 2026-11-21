mod support;

use sqlx::PgPool;
use std::{fs, path::Path, process::Command, time::Duration};
use support::{Cluster, Process, fixture_files::setup_code, ready};
use zenith_relay_hosted::db;

const RUNTIME: &str = "fixture_runtime";
const SETUP: &str = "fixture_setup";

#[tokio::test]
#[ignore = "requires disposable PostgreSQL, built web assets and Chromium"]
async fn backup_passkey_is_proven_and_last_two_revocations_cannot_both_succeed() {
    let cluster = Cluster::new();
    let admin = PgPool::connect(&cluster.dsn("fixture_boot")).await.unwrap();
    cluster.create_logins(&admin, &[SETUP, RUNTIME]).await;
    let port = support::port();
    let origin = format!("http://localhost:{port}");
    let cfg = cluster.temp.path().join("server.toml");
    fs::write(
        &cfg,
        include_str!("../config.example.toml").replace("1443", &port.to_string()),
    )
    .unwrap();
    db::migrate(&admin, &origin).await.unwrap();
    sqlx::raw_sql(
        "GRANT USAGE ON SCHEMA relay_hosted TO fixture_runtime,fixture_setup;
         GRANT SELECT ON relay_hosted.closed_installation,relay_hosted._sqlx_migrations TO fixture_runtime,fixture_setup;
         GRANT SELECT ON relay_hosted.pending_owner TO fixture_setup;
         GRANT EXECUTE ON FUNCTION relay_hosted.bind_first_owner(uuid,bigint,text,uuid,bytea,jsonb,bigint) TO fixture_setup;",
    ).execute(&admin).await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(
        include_str!("../runtime-grants.sql").replace(":\"runtime_role\"", RUNTIME),
    ))
    .execute(&admin)
    .await
    .unwrap();
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT NOT has_table_privilege('fixture_runtime','relay_hosted.admin_credentials','SELECT,INSERT,UPDATE')
            AND NOT has_table_privilege('fixture_runtime','relay_hosted.owner_passkey_changes','SELECT,INSERT')
            AND NOT has_function_privilege('fixture_setup','relay_hosted.owner_passkey_context(text,bytea)','EXECUTE')",
    ).fetch_one(&admin).await.unwrap());

    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web");
    let dist = web.join("dist");
    let setup_dir = tempfile::tempdir_in(cluster.temp.path()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(setup_dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let mut setup = Process(
        cluster
            .cli(SETUP, &cfg)
            .args(["setup-owner", "--web-dist"])
            .arg(&dist)
            .arg("--code-dir")
            .arg(setup_dir.path())
            .spawn()
            .unwrap(),
    );
    ready(&mut setup, &http, &format!("{origin}/setup/owner")).await;
    let mut browser = Process(
        Command::new("bun")
            .current_dir(&web)
            .args(["tests/owner-passkey.mjs", &origin])
            .arg(setup_code(setup_dir.path()))
            .arg("manage-passkeys")
            .spawn()
            .unwrap(),
    );
    await_exit(&mut setup, 30).await;
    let mut server = Process(
        cluster
            .cli(RUNTIME, &cfg)
            .args(["serve", "--web-dist"])
            .arg(&dist)
            .spawn()
            .unwrap(),
    );
    ready(
        &mut server,
        &http,
        &format!("{origin}/api/hosted/v1/public/config"),
    )
    .await;
    await_exit(&mut browser, 50).await;

    let state: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM relay_hosted.active_owner_credentials),
            (SELECT count(*) FROM relay_hosted.admin_credentials),
            (SELECT count(*) FROM relay_hosted.owner_passkey_changes WHERE action='added'),
            (SELECT count(*) FROM relay_hosted.owner_passkey_changes WHERE action='revoked'),
            (SELECT count(*) FROM relay_hosted.admin_sessions s JOIN relay_hosted.admin_credentials k USING (credential_id)
                WHERE k.revoked_at IS NOT NULL)",
    ).fetch_one(&admin).await.unwrap();
    assert_eq!(state, (1, 2, 1, 1, 0));
    assert!(
        sqlx::query("DELETE FROM relay_hosted.owner_passkey_changes")
            .execute(&admin)
            .await
            .is_err()
    );

    // Exercise the transaction's checks too, independently of the HTTP guards.
    for (lifetime, expected) in [(0_i64, "expired"), (10_000, "last_passkey")] {
        let result: String = sqlx::query_scalar(
            "SELECT relay_hosted.change_owner_passkey(i.public_origin,s.verifier,s.user_id,
                k.credential_id,k.revision,0,false,false,
                CASE WHEN $1=0 THEN 0 ELSE floor(extract(epoch FROM clock_timestamp())*1000)::bigint+$1 END,
                k.passkey_id,NULL,NULL,NULL)
             FROM relay_hosted.installations i JOIN relay_hosted.admin_sessions s ON s.user_id=i.owner_user_id
             JOIN relay_hosted.admin_credentials k USING (credential_id) WHERE k.revoked_at IS NULL LIMIT 1",
        ).bind(lifetime).fetch_one(&admin).await.unwrap();
        assert_eq!(result, expected);
    }

    drop(server);
    admin.close().await;
    cluster.stop();
    cluster.start();
    let restarted = PgPool::connect(&cluster.dsn("fixture_boot")).await.unwrap();
    let state: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM relay_hosted.active_owner_credentials),
            (SELECT count(*) FROM relay_hosted.owner_passkey_changes)",
    )
    .fetch_one(&restarted)
    .await
    .unwrap();
    assert_eq!(state, (1, 2));
}

async fn await_exit(process: &mut Process, seconds: u64) {
    tokio::time::timeout(Duration::from_secs(seconds), async {
        loop {
            if let Some(status) = process.0.try_wait().unwrap() {
                assert!(status.success(), "fixture exited with {status}");
                return;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("fixture did not finish");
}
