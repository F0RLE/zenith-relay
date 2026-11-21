use crate::{
    PROVIDER, proof, state,
    support::{Cluster, Process, port, ready},
};
use reqwest::{Certificate, Client};
use sqlx::PgPool;
use std::{fs, path::Path, process::Command, time::Duration};
use uuid::Uuid;
use zenith_relay_hosted::{
    config::Config,
    db,
    vault::{Purpose, Vault},
};
use zenith_relay_hosted_device::{
    COPY_COMMIT_PATH, COPY_STATUS_PATH, CopyKey, CopyPlan, CopyProxy, Destination, DeviceKey,
};

#[path = "source_copy_proxy.rs"]
mod proxy;

async fn checkpoint(browser: &mut Process, path: &Path) {
    tokio::time::timeout(Duration::from_secs(40), async {
        while !path.is_file() {
            assert!(
                browser.0.try_wait().unwrap().is_none(),
                "copy browser exited early"
            );
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("copy browser checkpoint");
}

async fn wait_exit(child: &mut Process) {
    tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            if let Some(exit) = child.0.try_wait().unwrap() {
                assert!(exit.success());
                return;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("fixture did not exit");
}

async fn grant_roles(admin: &PgPool) {
    sqlx::raw_sql(
        "GRANT USAGE ON SCHEMA relay_hosted TO fixture_setup, fixture_runtime;
         GRANT SELECT ON relay_hosted.closed_installation, relay_hosted._sqlx_migrations TO fixture_setup, fixture_runtime;
         GRANT SELECT ON relay_hosted.pending_owner TO fixture_setup;
         GRANT EXECUTE ON FUNCTION relay_hosted.bind_first_owner(uuid,bigint,text,uuid,bytea,jsonb,bigint) TO fixture_setup;",
    ).execute(admin).await.unwrap();
    for (sql, var, role) in [
        (
            include_str!("../../runtime-grants.sql"),
            ":\"runtime_role\"",
            "fixture_runtime",
        ),
        (
            include_str!("../../vault-grants.sql"),
            ":\"vault_role\"",
            "fixture_vault",
        ),
        (
            include_str!("../../qualification-grants.sql"),
            ":\"qualification_role\"",
            "fixture_qualifier",
        ),
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.replace(var, role)))
            .execute(admin)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL, built web assets, OpenSSL CLI and Chromium"]
async fn owner_reviews_one_https_copy_and_the_worker_keeps_its_authenticated_proxy() {
    let c = Cluster::new();
    let root = c.temp.path();
    let admin = PgPool::connect(&c.dsn("fixture_boot")).await.unwrap();
    c.create_logins(
        &admin,
        &[
            "fixture_setup",
            "fixture_runtime",
            "fixture_vault",
            "fixture_qualifier",
        ],
    )
    .await;
    let mut ports = std::collections::BTreeSet::new();
    while ports.len() < 3 {
        ports.insert(port());
    }
    let [edge_port, web_port, transfer_port]: [u16; 3] =
        ports.into_iter().collect::<Vec<_>>().try_into().unwrap();
    let origin = format!("https://localhost:{edge_port}");
    let web_bind = format!("http://127.0.0.1:{web_port}");
    let transfer_bind = format!("127.0.0.1:{transfer_port}");
    let toml = include_str!("../../config.example.toml")
        .replace("http://localhost:1443", &origin)
        .replace("127.0.0.1:1443", &format!("127.0.0.1:{web_port}"))
        .replace(
            "[server]",
            "[server]\nweb_api_intake = true\nweb_model_checks = true",
        );
    let conf = root.join("copy.toml");
    fs::write(&conf, &toml).unwrap();
    let config = Config::parse(&toml).unwrap();
    db::migrate(&admin, &origin).await.unwrap();
    grant_roles(&admin).await;

    let cert = root.join("edge.pem");
    let tls_key = root.join("edge-key.pem");
    let cert_result = crate::support::fixture_files::openssl()
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost",
            "-addext",
            "basicConstraints=critical,CA:FALSE",
            "-addext",
            "extendedKeyUsage=serverAuth",
            "-out",
        ])
        .arg(&cert)
        .arg("-keyout")
        .arg(&tls_key)
        .output()
        .expect("OpenSSL CLI");
    assert!(
        cert_result.status.success(),
        "cannot generate the fixture TLS certificate"
    );
    let http = Client::builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .tls_certs_only([Certificate::from_pem(&fs::read(&cert).unwrap()).unwrap()])
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web");
    let dist = web.join("dist");
    let _edge = Process(
        Command::new("bun")
            .current_dir(&web)
            .arg("tests/source-copy-edge.mjs")
            .args([
                edge_port.to_string(),
                web_port.to_string(),
                transfer_port.to_string(),
            ])
            .arg(&cert)
            .arg(&tls_key)
            .spawn()
            .unwrap(),
    );
    let enrollment = root.join("enrollment");
    let exchange = root.join("exchange");
    let vault_dir = root.join("vault");
    let vault_key = root.join("vault-key");
    for dir in [&enrollment, &exchange, &vault_dir] {
        fs::create_dir(dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    fs::write(&vault_key, [43u8; 32]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&vault_key, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut setup = Process(
        c.cli("fixture_setup", &conf)
            .arg("setup-owner")
            .arg("--web-dist")
            .arg(&dist)
            .arg("--code-dir")
            .arg(&enrollment)
            .spawn()
            .unwrap(),
    );
    ready(&mut setup, &http, &format!("{origin}/setup/owner")).await;
    let code = crate::support::fixture_files::setup_code(&enrollment);
    let mut browser = Process(
        Command::new("bun")
            .current_dir(&web)
            .env("RELAY_TEST_EDGE_CERT", &cert)
            .env("NODE_EXTRA_CA_CERTS", &cert)
            .arg("tests/owner-passkey.mjs")
            .arg(&origin)
            .arg(code)
            .arg("source-copy")
            .arg(&exchange)
            .spawn()
            .unwrap(),
    );
    wait_exit(&mut setup).await;
    let mut web_server = Process(
        c.cli("fixture_runtime", &conf)
            .arg("serve")
            .arg("--web-dist")
            .arg(&dist)
            .spawn()
            .unwrap(),
    );
    ready(
        &mut web_server,
        &http,
        &format!("{origin}/api/hosted/v1/public/config"),
    )
    .await;
    let start_worker = || {
        Process(
            c.cli("fixture_vault", &conf)
                .args(["serve-transfers", "--bind", &transfer_bind, "--vault-dir"])
                .arg(&vault_dir)
                .env("ZENITH_RELAY_HOSTED_VAULT_KEY_FILE", &vault_key)
                .spawn()
                .unwrap(),
        )
    };
    let mut worker = start_worker();
    let (iid, owner): (Uuid, Uuid) =
        sqlx::query_as("SELECT id,owner_user_id FROM relay_hosted.installations")
            .fetch_one(&admin)
            .await
            .unwrap();
    let device = Uuid::new_v4();
    let target = Destination::parse(&format!(
        "{origin}/admin/connections#relay-device={iid}.{device}"
    ))
    .unwrap();
    let paired = DeviceKey::generate(target.clone()).unwrap();
    sqlx::query(
        "INSERT INTO relay_hosted.management_devices (id,installation_id,owner_id,installation_epoch,
          owner_epoch,label,scope,revision,invited_at,approve_by,approved_at,expires_at,verifier,manage_pool,read_scopes)
         SELECT $1,i.id,u.id,i.authorization_epoch,u.authorization_epoch,'Copy fixture','connections.read',2,
          now(),now()+interval '5 minutes',now(),now()+interval '24 hours',$2,true,'{overview.read}'
         FROM relay_hosted.installations i JOIN relay_hosted.users u ON u.id=i.owner_user_id",
    ).bind(device).bind(paired.verifier().as_slice()).execute(&admin).await.unwrap();
    let route = proxy::Fixture::start().await;
    let source = Uuid::new_v4();
    let plan = CopyPlan {
        origin: origin.clone(),
        installation_id: iid,
        device_id: device,
        operation_id: Uuid::new_v4(),
        source_id: source,
        label: "Synthetic copied source".to_owned(),
        endpoint: route.endpoint.clone(),
        proxy: CopyProxy::from_route(&route.proxy),
    };
    let key = CopyKey::create(&target, plan.clone(), PROVIDER, Some(&route.proxy)).unwrap();
    let bearer = proof(&key);
    let status = format!("{origin}{COPY_STATUS_PATH}");
    let commit = format!("{origin}{COPY_COMMIT_PATH}");
    let post = || {
        http.post(&commit)
            .bearer_auth(bearer.as_str())
            .header("content-type", "application/octet-stream")
            .body(format!("{PROVIDER}\n{}", route.proxy))
    };
    let wait_status = |mut child: Process| {
        let http = http.clone();
        let status = status.clone();
        let bearer = bearer.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    assert!(
                        child.0.try_wait().unwrap().is_none(),
                        "transfer worker exited early"
                    );
                    if let Ok(response) =
                        http.get(&status).bearer_auth(bearer.as_str()).send().await
                        && response.status().is_success()
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(40)).await;
                }
            })
            .await
            .expect("transfer worker readiness");
            child
        }
    };
    worker = wait_status(worker).await;
    assert_eq!(post().send().await.unwrap().status(), 401);
    assert_eq!(
        http.get(&status)
            .bearer_auth(key.review().code())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("{web_bind}{COPY_COMMIT_PATH}"))
            .send()
            .await
            .unwrap()
            .status(),
        404,
    );
    checkpoint(&mut browser, &exchange.join("owner-ready")).await;
    assert_eq!(
        sqlx::query(
            "UPDATE relay_hosted.admin_sessions SET created_at=created_at-interval '3 minutes'"
        )
        .execute(&admin)
        .await
        .unwrap()
        .rows_affected(),
        1
    );
    fs::write(exchange.join("review"), key.review().code()).unwrap();
    checkpoint(&mut browser, &exchange.join("approved")).await;
    assert_eq!(
        post()
            .header("origin", origin.as_str())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(state(post().send().await.unwrap()).await, "committed");
    let record = key.protected_record();
    drop(worker);
    let _worker = wait_status(start_worker()).await;
    let restored = CopyKey::restore(&record).unwrap();
    assert_eq!(
        state(
            http.get(&status)
                .bearer_auth(proof(&restored).as_str())
                .send()
                .await
                .unwrap()
        )
        .await,
        "committed"
    );
    let scope = proxy::activate_scope(&admin, owner).await;
    let web_route = proxy::Fixture::start().await;
    fs::write(
        exchange.join("web-model-route"),
        serde_json::json!({ "endpoint": web_route.endpoint, "proxy": web_route.proxy, "key": PROVIDER }).to_string(),
    )
    .unwrap();
    fs::write(exchange.join("committed"), []).unwrap();

    checkpoint(&mut browser, &exchange.join("simulate-check-interruption")).await;
    let abandoned: Uuid = fs::read_to_string(exchange.join("simulate-check-interruption"))
        .unwrap()
        .parse()
        .unwrap();
    let qp = db::connect(&config, &c.dsn("fixture_qualifier"))
        .await
        .unwrap();
    let take = || {
        sqlx::query_scalar::<_, Option<serde_json::Value>>(
            "SELECT relay_hosted.take_web_model_check($1)",
        )
        .bind(&origin)
        .fetch_one(&qp)
    };
    let (left, right) = tokio::join!(take(), take());
    let taken: Vec<_> = [left.unwrap(), right.unwrap()]
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0]["id"], abandoned.to_string());
    sqlx::query("UPDATE relay_hosted.web_model_checks SET started_at=clock_timestamp()-interval '2 minutes' WHERE operation_id=$1")
        .bind(abandoned).execute(&admin).await.unwrap();
    assert!(
        take().await.unwrap().is_none(),
        "claimed checks never run again"
    );

    checkpoint(&mut browser, &exchange.join("start-model-worker")).await;
    assert_eq!(
        web_route.counts(),
        [0, 0, 0],
        "save, options and status reads do not probe"
    );
    let _qualifier = Process(
        c.cli("fixture_qualifier", &conf)
            .args(["qualify-text", "--watch", "--vault-dir"])
            .arg(&vault_dir)
            .env("ZENITH_RELAY_HOSTED_VAULT_KEY_FILE", &vault_key)
            .env(
                "ZENITH_RELAY_HOSTED_VAULT_DATABASE_URL_FILE",
                c.role_file("fixture_vault"),
            )
            .spawn()
            .unwrap(),
    );
    checkpoint(&mut browser, &exchange.join("desktop-pool-ready")).await;
    let reviewed_source = fs::read_to_string(exchange.join("desktop-pool-ready")).unwrap();
    let device_record = paired.protected_record();
    let device_secret = device_record.split_once('\n').unwrap().1;
    let device_bearer = zeroize::Zeroizing::new(format!("rhd1.{iid}.{device}.{device_secret}"));
    let endpoint = format!("{origin}/api/hosted/v1/device/pool");
    for action in ["include", "exclude"] {
        let read = || {
            http.get(&endpoint)
                .bearer_auth(device_bearer.as_str())
                .send()
        };
        let state: serde_json::Value = read().await.unwrap().json().await.unwrap();
        let row = state["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == reviewed_source)
            .unwrap();
        assert_eq!(state["data"]["pool_access"], true);
        assert_eq!(row["pool"]["action"], action);
        assert_eq!(row["proxy_bound"], true);
        let body = serde_json::json!({"source_id":reviewed_source, "action":action, "expected_revision":row["pool"]["revision"]});
        let request = || {
            http.post(&endpoint)
                .bearer_auth(device_bearer.as_str())
                .json(&body)
        };
        if action == "include" {
            assert_eq!(
                request()
                    .header("origin", &origin)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                401
            );
        }
        let response = request().send().await.unwrap();
        assert_eq!(response.status(), 200);
        let review: serde_json::Value = response.json().await.unwrap();
        let code = review["data"]["result"]["operation_id"].as_str().unwrap();
        fs::write(exchange.join(format!("desktop-pool-{action}")), code).unwrap();
        checkpoint(
            &mut browser,
            &exchange.join(format!("desktop-pool-{action}-done")),
        )
        .await;
        let updated: serde_json::Value = read().await.unwrap().json().await.unwrap();
        let row = updated["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == reviewed_source)
            .unwrap();
        assert_eq!(row["pool"]["in_pool"], action == "include");
        assert_eq!(row["credential_state"], "stored");
        assert_eq!(row["proxy_bound"], true);
        assert!(updated["data"]["pool_review"].is_null());
        let metrics = http
            .get(format!(
                "{origin}/api/hosted/v1/device/overview/metrics?hours=24"
            ))
            .bearer_auth(device_bearer.as_str())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(
            metrics["data"]["result"]["metrics"]["pool"],
            serde_json::json!({"members":if action=="include" {"1"} else {"0"},"ready":"0","busy":"0"})
        );
    }
    wait_exit(&mut browser).await;
    assert_eq!(
        web_route.counts(),
        [2, 1, 0],
        "one short check; the unlisted model is never dispatched"
    );

    let source = fs::read_to_string(exchange.join("web-source"))
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let web_vault_pool = db::connect(&config, &c.dsn("fixture_vault")).await.unwrap();
    let web_vault = Vault::open(&web_vault_pool, &origin, &vault_dir, &vault_key)
        .await
        .unwrap();
    let stored_key = web_vault.read(source, Purpose::Provider, 1).await.unwrap();
    let stored_proxy = web_vault.read(source, Purpose::Proxy, 1).await.unwrap();
    assert!(stored_key.as_slice() == PROVIDER.as_bytes());
    assert!(stored_proxy.as_slice() == web_route.proxy.as_bytes());
    let routes: (bool, bool, bool) = sqlx::query_as(
        "SELECT relay_hosted.source_copy_route($1,$2,$3,true),
            relay_hosted.source_copy_route($1,$2,$3,false),
            relay_hosted.source_copy_route($1,$2,$4,true)",
    )
    .bind(iid)
    .bind(source)
    .bind(&web_route.endpoint)
    .bind(&route.endpoint)
    .fetch_one(&web_vault_pool)
    .await
    .unwrap();
    assert_eq!(routes, (true, false, false));
    let created: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM relay_hosted.web_source_intakes WHERE committed_at IS NOT NULL",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(created, 1);

    route
        .verify(&c, &config, scope, &plan, &vault_dir, &vault_key)
        .await;
}
