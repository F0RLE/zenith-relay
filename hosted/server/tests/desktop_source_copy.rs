#[path = "support/source_copy_browser.rs"]
mod browser;
#[allow(dead_code)]
mod support;

use reqwest::{Client, Response};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::fs;
use support::Cluster;
use uuid::Uuid;
use zenith_relay_hosted::{
    config::Config,
    db, transfers,
    vault::{Purpose, Vault},
};
use zenith_relay_hosted_device::{
    COPY_COMMIT_PATH, COPY_STATUS_PATH, CopyKey, CopyPlan, CopyProxy, Destination, DeviceKey,
};

const ORIGIN: &str = "https://relay.example";
const PROVIDER: &str = "fixture-provider-copy-key";
const PROXY: &str = "http://fixture-proxy:fixture-password@127.0.0.1:9080/";
const ENDPOINT: &str = "http://127.0.0.1:9081/v1";
const SESSION: [u8; 32] = [24; 32];

async fn review(runtime: &PgPool, key: &CopyKey, approver: &[u8], approve: bool) -> Value {
    let r = key.review();
    let p = &r.plan;
    sqlx::query_scalar("SELECT relay_hosted.review_source_copy($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(ORIGIN)
        .bind(approver)
        .bind(p.installation_id)
        .bind(p.device_id)
        .bind(p.operation_id)
        .bind(p.source_id)
        .bind(sqlx::types::Json(p))
        .bind(r.verifier_bytes().unwrap().as_slice())
        .bind(r.commitment_bytes().unwrap().as_slice())
        .bind(approve)
        .fetch_one(runtime)
        .await
        .unwrap()
}

async fn worker(vault: &Vault) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = format!("http://{}", listener.local_addr().unwrap());
    let app = transfers::router(ORIGIN, vault.clone()).await.unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (local, task)
}

fn proof(key: &CopyKey) -> zeroize::Zeroizing<String> {
    let record = key.protected_record();
    zeroize::Zeroizing::new(record.split_once('\n').unwrap().1.to_owned())
}

async fn state(response: Response) -> Value {
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let reply = response.json::<Value>().await.unwrap();
    assert!(!reply.to_string().contains("fixture-password"));
    assert!(!reply.to_string().contains(PROVIDER));
    reply["data"]["state"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated PostgreSQL cluster; set RELAY_TEST_PG_BIN"]
async fn copy_is_fenced_atomic_and_never_uses_the_read_grant_to_write() {
    let c = Cluster::new();
    let bootstrap = PgPool::connect(&c.dsn("fixture_boot")).await.unwrap();
    c.create_logins(&bootstrap, &["fixture_runtime", "fixture_vault"])
        .await;
    let conf = include_str!("../config.example.toml")
        .replace("http://localhost:1443", ORIGIN)
        .replace("\"localhost\"", "\"relay.example\"");
    let config = Config::parse(&conf).unwrap();
    db::migrate(&bootstrap, ORIGIN).await.unwrap();
    sqlx::raw_sql("GRANT USAGE ON SCHEMA relay_hosted TO fixture_runtime;
        GRANT SELECT ON relay_hosted.closed_installation, relay_hosted._sqlx_migrations TO fixture_runtime;")
        .execute(&bootstrap).await.unwrap();
    for (sql, var, role) in [
        (
            include_str!("../runtime-grants.sql"),
            ":\"runtime_role\"",
            "fixture_runtime",
        ),
        (
            include_str!("../vault-grants.sql"),
            ":\"vault_role\"",
            "fixture_vault",
        ),
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.replace(var, role)))
            .execute(&bootstrap)
            .await
            .unwrap();
    }
    let runtime = db::connect(&config, &c.dsn("fixture_runtime"))
        .await
        .unwrap();
    db::check_runtime(&runtime, ORIGIN).await.unwrap();
    let vault_pool = db::connect(&config, &c.dsn("fixture_vault")).await.unwrap();
    let dir = c.temp.path().join("copy-vault");
    fs::create_dir(&dir).unwrap();
    let key_file = c.temp.path().join("copy-vault-key");
    fs::write(&key_file, [31; 32]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&key_file, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let vault = Vault::open(&vault_pool, ORIGIN, &dir, &key_file)
        .await
        .unwrap();
    assert!(
        transfers::router("http://localhost:1443", vault.clone())
            .await
            .is_err()
    );
    assert!(
        Vault::open(&runtime, ORIGIN, &dir, &key_file)
            .await
            .is_err()
    );

    let iid = sqlx::query_scalar::<_, Uuid>("SELECT id FROM relay_hosted.installations")
        .fetch_one(&bootstrap)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    let device = Uuid::new_v4();
    let credential = [18u8; 32];
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT relay_hosted.bind_first_owner($1,1,$2,$3,$4,'{}',9223372036854775807)"
        )
        .bind(iid)
        .bind(ORIGIN)
        .bind(owner)
        .bind(credential.as_slice())
        .fetch_one(&bootstrap)
        .await
        .unwrap()
    );
    sqlx::query("INSERT INTO relay_hosted.admin_sessions
        SELECT $1,$2,$3,authorization_epoch,1,now()-interval '3 minutes',now(),now()+interval '8 hours'
        FROM relay_hosted.installations WHERE id=$4")
        .bind(SESSION.as_slice()).bind(owner).bind(credential.as_slice()).bind(iid)
        .execute(&bootstrap).await.unwrap();
    let dest = Destination::parse(&format!(
        "{ORIGIN}/admin/connections#relay-device={iid}.{device}"
    ))
    .unwrap();
    let paired = DeviceKey::generate(dest.clone()).unwrap();
    sqlx::query("INSERT INTO relay_hosted.management_devices (id,installation_id,owner_id,installation_epoch,
        owner_epoch,label,scope,revision,invited_at,approve_by,approved_at,expires_at,verifier)
        SELECT $1,id,$2,authorization_epoch,1,'Test desktop','connections.read',2,
            now(),now()+interval '5 minutes',now(),now()+interval '24 hours',$3
        FROM relay_hosted.installations WHERE id=$4")
        .bind(device).bind(owner).bind(paired.verifier().as_slice()).bind(iid).execute(&bootstrap).await.unwrap();
    let new_key = |proxy: Option<&str>, source_id| {
        CopyKey::create(
            &dest,
            CopyPlan {
                origin: ORIGIN.to_owned(),
                installation_id: iid,
                device_id: device,
                operation_id: Uuid::new_v4(),
                source_id,
                label: "Synthetic local source".into(),
                endpoint: ENDPOINT.into(),
                proxy: proxy.and_then(CopyProxy::from_route),
            },
            PROVIDER,
            proxy,
        )
        .unwrap()
    };
    let source = Uuid::new_v4();
    let key = new_key(Some(PROXY), source);
    let auth = proof(&key);
    let persisted = key.protected_record();
    let (local, task) = worker(&vault).await;
    let http = Client::builder().no_proxy().build().unwrap();
    let status = || {
        http.get(format!("{local}{COPY_STATUS_PATH}"))
            .bearer_auth(auth.as_str())
    };
    let submit = |value: String| {
        http.post(format!("{local}{COPY_COMMIT_PATH}"))
            .bearer_auth(auth.as_str())
            .header("content-type", "application/octet-stream")
            .body(value)
    };
    let count = || {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM relay_hosted.connection_secret_slots WHERE connection_id=$1",
        )
        .bind(source)
        .fetch_one(&bootstrap)
    };
    let body = format!("{PROVIDER}\n{PROXY}");
    assert_eq!(
        state(status().send().await.unwrap()).await,
        "awaiting_approval"
    );
    assert_eq!(submit(body.clone()).send().await.unwrap().status(), 401);
    assert_eq!(
        review(&runtime, &key, &paired.verifier(), true).await["state"],
        "denied"
    );
    assert_eq!(
        review(&runtime, &key, &SESSION, false).await["state"],
        "awaiting_approval"
    );
    assert_eq!(
        review(&runtime, &key, &SESSION, true).await["state"],
        "fresh_required"
    );
    assert_eq!(count().await.unwrap(), 0);
    sqlx::query("UPDATE relay_hosted.admin_sessions SET created_at=now()")
        .execute(&bootstrap)
        .await
        .unwrap();
    assert_eq!(
        review(&runtime, &key, &SESSION, true).await["state"],
        "approved"
    );
    assert_eq!(
        review(&runtime, &key, &SESSION, true).await["state"],
        "approved"
    );
    assert_eq!(
        review(&runtime, &new_key(Some(PROXY), source), &SESSION, true).await["state"],
        "conflict"
    );
    assert_eq!(state(status().send().await.unwrap()).await, "approved");
    let pairing = paired.protected_record();
    let read_bearer = format!(
        "rhd1.{iid}.{device}.{}",
        pairing.split_once('\n').unwrap().1
    );
    for r in [
        submit(body.clone()).header("origin", ORIGIN),
        submit(body.clone()).header("cookie", "fixture=1"),
        http.post(format!("{local}{COPY_COMMIT_PATH}"))
            .bearer_auth(&read_bearer)
            .header("content-type", "application/octet-stream")
            .body(body.clone()),
        http.get(format!("{local}{COPY_STATUS_PATH}"))
            .bearer_auth(key.review().code()),
        http.get(format!("{local}{COPY_STATUS_PATH}"))
            .bearer_auth(auth.replace(&iid.to_string(), &Uuid::new_v4().to_string())),
    ] {
        assert_eq!(r.send().await.unwrap().status(), 401);
    }
    assert_eq!(
        state(submit(format!("{PROVIDER}\n")).send().await.unwrap()).await,
        "conflict"
    );
    assert_eq!(
        state(
            submit(format!("changed-key\n{PROXY}"))
                .send()
                .await
                .unwrap()
        )
        .await,
        "conflict"
    );
    assert_eq!(
        state(
            submit(body.replace("fixture-password", "changed-proxy"))
                .send()
                .await
                .unwrap()
        )
        .await,
        "conflict"
    );
    assert_eq!(count().await.unwrap(), 0);

    sqlx::raw_sql("CREATE FUNCTION relay_hosted.reject_copy_fixture() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RAISE EXCEPTION 'fixture aborted'; END $$;
        CREATE CONSTRAINT TRIGGER reject_copy_fixture AFTER UPDATE ON relay_hosted.desktop_source_copies
        DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_copy_fixture();")
        .execute(&bootstrap).await.unwrap();
    assert_eq!(submit(body.clone()).send().await.unwrap().status(), 503);
    assert_eq!(state(status().send().await.unwrap()).await, "approved");
    assert_eq!(count().await.unwrap(), 0);
    assert_eq!(vault.collect_unreferenced().await.unwrap(), 2);
    sqlx::raw_sql(
        "DROP TRIGGER reject_copy_fixture ON relay_hosted.desktop_source_copies;
        DROP FUNCTION relay_hosted.reject_copy_fixture();",
    )
    .execute(&bootstrap)
    .await
    .unwrap();

    // Discard the first reply and restart the worker. The persisted native operation reconciles without the keys.
    drop(submit(body.clone()).send().await.unwrap());
    task.abort();
    let reopened = Vault::open(&vault_pool, ORIGIN, &dir, &key_file)
        .await
        .unwrap();
    let (local, task) = worker(&reopened).await;
    let saved = CopyKey::restore(&persisted).unwrap();
    assert_eq!(
        state(
            http.get(format!("{local}{COPY_STATUS_PATH}"))
                .bearer_auth(proof(&saved).as_str())
                .send()
                .await
                .unwrap()
        )
        .await,
        "committed"
    );
    assert_eq!(
        state(
            http.post(format!("{local}{COPY_COMMIT_PATH}"))
                .bearer_auth(proof(&saved).as_str())
                .header("content-type", "application/octet-stream")
                .body("replacement\n")
                .send()
                .await
                .unwrap()
        )
        .await,
        "committed"
    );
    assert_eq!(count().await.unwrap(), 2);
    assert_eq!(
        &*vault.read(source, Purpose::Provider, 1).await.unwrap(),
        PROVIDER.as_bytes()
    );
    assert_eq!(
        &*vault.read(source, Purpose::Proxy, 1).await.unwrap(),
        PROXY.as_bytes()
    );
    let inventory: Value =
        sqlx::query_scalar("SELECT relay_hosted.device_connections($1,$2,$3,$4,NULL)")
            .bind(ORIGIN)
            .bind(iid)
            .bind(device)
            .bind(paired.verifier().as_slice())
            .fetch_one(&runtime)
            .await
            .unwrap();
    assert_eq!(inventory["items"][0]["label"], "Synthetic local source");
    assert_eq!(inventory["items"][0]["proxy_bound"], true);
    assert_eq!(inventory["items"][0]["verified_models"], json!([]));
    assert!(!inventory.to_string().contains(PROVIDER));
    assert!(!inventory.to_string().contains("fixture-password"));
    let matches = |endpoint, proxy| {
        sqlx::query_scalar::<_, bool>("SELECT relay_hosted.source_copy_route($1,$2,$3,$4)")
            .bind(iid)
            .bind(source)
            .bind(endpoint)
            .bind(proxy)
            .fetch_one(&vault_pool)
    };
    assert!(matches(ENDPOINT, true).await.unwrap());
    assert!(!matches(ENDPOINT, false).await.unwrap());
    assert!(!matches("http://127.0.0.1:9082/v1", true).await.unwrap());
    assert_eq!(
        review(&runtime, &new_key(Some(PROXY), source), &SESSION, true).await["state"],
        "conflict"
    );

    let direct = new_key(None, Uuid::new_v4());
    assert_eq!(
        review(&runtime, &direct, &SESSION, true).await["state"],
        "approved"
    );
    let apply = |k: &CopyKey| {
        http.post(format!("{local}{COPY_COMMIT_PATH}"))
            .bearer_auth(proof(k).as_str())
            .header("content-type", "application/octet-stream")
            .body(format!("{PROVIDER}\n"))
    };
    assert_eq!(
        state(apply(&direct).send().await.unwrap()).await,
        "committed"
    );
    assert!(
        vault
            .read(direct.review().plan.source_id, Purpose::Proxy, 1)
            .await
            .is_err()
    );

    let expired = new_key(None, Uuid::new_v4());
    assert_eq!(
        review(&runtime, &expired, &SESSION, true).await["state"],
        "approved"
    );
    sqlx::query("UPDATE relay_hosted.desktop_source_copies SET
        approved_at=approved_at-interval '11 minutes',expires_at=expires_at-interval '11 minutes' WHERE operation_id=$1")
        .bind(expired.review().plan.operation_id).execute(&bootstrap).await.unwrap();
    assert_eq!(
        state(apply(&expired).send().await.unwrap()).await,
        "expired"
    );
    let revoked = new_key(None, Uuid::new_v4());
    assert_eq!(
        review(&runtime, &revoked, &SESSION, true).await["state"],
        "approved"
    );
    sqlx::query(
        "UPDATE relay_hosted.management_devices SET revoked_at=now(),revision=3 WHERE id=$1",
    )
    .bind(device)
    .execute(&bootstrap)
    .await
    .unwrap();
    assert_eq!(
        state(apply(&revoked).send().await.unwrap()).await,
        "expired"
    );
    assert_eq!(
        state(
            http.get(format!("{local}{COPY_STATUS_PATH}"))
                .bearer_auth(auth.as_str())
                .send()
                .await
                .unwrap()
        )
        .await,
        "committed"
    );
    task.abort();
}
