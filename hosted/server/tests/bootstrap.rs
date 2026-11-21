#[path = "support/owner.rs"]
mod owner;
#[allow(dead_code)]
#[path = "support/partial_text.rs"]
mod partial_text;
#[path = "support/purchase_budget_upgrade.rs"]
mod purchase_budget_upgrade;
mod support;
#[path = "support/vault.rs"]
mod vault;

use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{fs, path::Path, process::Command, time::Duration};
use support::{Cluster, Process};
use uuid::Uuid;
use zenith_relay_hosted::{config::Config, db};

const MIGRATOR: &str = "fixture_migrator";
const RUNTIME: &str = "fixture_runtime";
const SETUP: &str = "fixture_setup";

#[tokio::test]
#[ignore = "creates a private temporary cluster; set RELAY_TEST_PG_BIN to the PostgreSQL bin directory"]
async fn empty_install_restart_and_fail_closed() {
    let c = Cluster::new();
    let bootstrap = PgPool::connect(&c.dsn("fixture_boot")).await.unwrap();
    let rates: Value = serde_json::from_str(include_str!("fixtures/retail-rates.json")).unwrap();
    for row in rates["valid"].as_array().unwrap() {
        let (wire, charge): (String, String) = sqlx::query_as(
            "SELECT ($1::text::numeric(30,12))::text, ceil($1::text::numeric * $2::bigint)::text",
        )
        .bind(row["input"].as_str().unwrap())
        .bind(row["tokens"].as_str().unwrap().parse::<i64>().unwrap())
        .fetch_one(&bootstrap)
        .await
        .unwrap();
        assert_eq!(wire, row["wire"].as_str().unwrap());
        assert_eq!(
            charge.parse::<i64>().ok(),
            row["micro_usd"].as_str().map(|s| s.parse().unwrap())
        );
    }
    c.create_logins(&bootstrap, &[MIGRATOR, RUNTIME, SETUP, "fixture_vault"])
        .await;
    sqlx::query("GRANT CREATE ON DATABASE postgres TO fixture_migrator")
        .execute(&bootstrap)
        .await
        .unwrap();
    let port = support::port();
    let origin = format!("http://localhost:{port}");
    let url = format!("{origin}/api/hosted/v1/public/config");
    let cfg = c.temp.path().join("server.toml");
    let toml = include_str!("../config.example.toml")
        .replace("1443", &port.to_string())
        .replace("\"Relay\"", "\"Fixture Site\"")
        .replace("# terms =", "terms =");
    fs::write(&cfg, &toml).unwrap();
    let config = Config::parse(&toml).unwrap();
    let runtime = db::connect(&config, &c.dsn(RUNTIME)).await.unwrap();
    assert!(db::check_runtime(&runtime, &origin).await.is_err());
    assert_eq!(
        db::upgrade_plan(&runtime, &origin).await,
        Err("cannot read installation migration history")
    );
    assert!(
        !c.cli(RUNTIME, &cfg)
            .arg("migrate")
            .output()
            .unwrap()
            .status
            .success()
    );
    let migrate = || c.cli(MIGRATOR, &cfg).arg("migrate").spawn().unwrap();
    // Qualify upgrades separately from two current binaries racing on an empty database.
    sqlx::query("CREATE DATABASE fixture_upgrade OWNER fixture_migrator")
        .execute(&bootstrap)
        .await
        .unwrap();
    let previous = db::connect(
        &config,
        &c.dsn(MIGRATOR).replace("/postgres?", "/fixture_upgrade?"),
    )
    .await
    .unwrap();
    let mut conn = previous.acquire().await.unwrap();
    let mut migrations = sqlx::migrate!("./migrations");
    migrations.create_schema("relay_hosted");
    migrations.dangerous_set_table_name("relay_hosted._sqlx_migrations");
    sqlx::query("SELECT set_config('relay_hosted.initial_public_origin', $1, false)")
        .bind(&origin)
        .execute(&mut *conn)
        .await
        .unwrap();
    migrations.run_to(1, &mut *conn).await.unwrap();
    drop(conn);
    let previous_id =
        sqlx::query_scalar::<_, String>("SELECT id::text FROM relay_hosted.installations")
            .fetch_one(&previous)
            .await
            .unwrap();
    assert!(
        db::migrate(&previous, "https://other.example")
            .await
            .is_err()
    );
    let version =
        sqlx::query_scalar::<_, i64>("SELECT max(version) FROM relay_hosted._sqlx_migrations")
            .fetch_one(&previous)
            .await
            .unwrap();
    assert_eq!(version, 1);
    migrations.run_to(19, &previous).await.unwrap();
    let (item, offer, draft) = legacy_text_listing(&previous).await;
    purchase_budget_upgrade::seed_legacy(&previous, offer).await;
    migrations.run_to(20, &previous).await.unwrap();
    let late_purchase = purchase_budget_upgrade::seed_priced(&previous, item, offer).await;
    migrations.run_to(23, &previous).await.unwrap();
    let retained = purchase_budget_upgrade::seed_late_usage(&previous, offer).await;
    migrations.run_to(33, &previous).await.unwrap();
    let legacy_method: Uuid = sqlx::query_scalar(
        "WITH merchant AS (
            INSERT INTO relay_hosted.merchant_bindings(provider_id,environment,merchant_identity)
            VALUES ('fixture','sandbox','legacy-checkout') RETURNING id
         )
         INSERT INTO relay_hosted.checkout_methods(merchant_binding_id,method_code,label,position,
            min_credit_minor,max_credit_minor,fee_minor)
         SELECT id,'card','Legacy card',0,1000,2000,50 FROM merchant RETURNING id",
    )
    .fetch_one(&previous)
    .await
    .unwrap();
    migrations.run_to(35, &previous).await.unwrap();
    let checkpoint = legacy_wallet(&previous, retained.0).await;
    let target = verify_upgrade_plan(&previous, &origin).await;
    migrations.run(&previous).await.unwrap();
    db::migrate(&previous, &origin).await.unwrap();
    assert_eq!(
        db::upgrade_plan(&previous, &origin).await.unwrap(),
        db::UpgradePlan {
            installed: target,
            target,
            pending: vec![]
        }
    );
    assert_eq!(legacy_wallet(&previous, retained.0).await, checkpoint);
    let (reserve, ledger): (i64, bool) = sqlx::query_as(
        "SELECT w.refund_reserved_micro_usd,EXISTS(SELECT FROM relay_hosted.ledger_accounts a
             WHERE a.workspace_id=w.workspace_id AND a.account_kind='customer_refund_reserved')
         FROM relay_hosted.wallets w WHERE w.workspace_id=$1",
    )
    .bind(retained.0)
    .fetch_one(&previous)
    .await
    .unwrap();
    assert_eq!((reserve, ledger), (0, true));
    let legacy_terms: (String, i32, i32, bool) = sqlx::query_as(
        "SELECT fee_payer,fee_minor,merchant_fee_minor,disabled_at IS NULL
        FROM relay_hosted.checkout_methods WHERE id=$1",
    )
    .bind(legacy_method)
    .fetch_one(&previous)
    .await
    .unwrap();
    assert_eq!(legacy_terms, ("customer".into(), 50, 0, true));
    let rule: (bool, bool, i64, Option<i64>, Uuid, Value) = sqlx::query_as(
        "SELECT admitted,enabled,revision,purchase_revision,last_offer,draft FROM relay_hosted.text_model_rules WHERE id=$1",
    )
    .bind(item)
    .fetch_one(&previous)
    .await
    .unwrap();
    assert_eq!(rule, (true, false, 4, None, offer, draft));
    let listed: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT FROM relay_hosted.active_text_listings)")
            .fetch_one(&previous)
            .await
            .unwrap();
    assert!(!listed);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT id::text FROM relay_hosted.installations")
            .fetch_one(&previous)
            .await
            .unwrap(),
        previous_id
    );
    purchase_budget_upgrade::check(&previous, &origin, offer, late_purchase).await;
    purchase_budget_upgrade::check_late_usage(&previous, retained).await;
    previous.close().await;
    let (mut a, mut b) = (migrate(), migrate());
    assert!(a.wait().unwrap().success() && b.wait().unwrap().success());
    let identity = || {
        sqlx::query_as::<_, (String, String, i64)>(
            "SELECT i.id::text, s.mode, s.admission_epoch FROM relay_hosted.installations i
         JOIN relay_hosted.service_control s ON s.installation_id=i.id",
        )
        .fetch_all(&bootstrap)
    };
    let original = identity().await.unwrap();
    assert_eq!(original.len(), 1);
    assert_ne!(original[0].0, previous_id);
    assert!(Uuid::parse_str(&original[0].0).is_ok());
    assert_eq!((&*original[0].1, original[0].2), ("setup", 1));
    sqlx::raw_sql(
        "GRANT USAGE ON SCHEMA relay_hosted TO fixture_runtime, fixture_setup;
         GRANT SELECT ON relay_hosted.closed_installation, relay_hosted._sqlx_migrations TO fixture_runtime, fixture_setup;
         GRANT SELECT ON relay_hosted.pending_owner TO fixture_setup;
         GRANT EXECUTE ON FUNCTION relay_hosted.bind_first_owner(uuid,bigint,text,uuid,bytea,jsonb,bigint) TO fixture_setup;",
    ).execute(&bootstrap).await.unwrap();
    assert!(db::check_runtime(&bootstrap, &origin).await.is_err());
    assert!(db::check_runtime(&runtime, &origin).await.is_err());
    sqlx::raw_sql(sqlx::AssertSqlSafe(
        include_str!("../runtime-grants.sql").replace(":\"runtime_role\"", RUNTIME),
    ))
    .execute(&bootstrap)
    .await
    .unwrap();
    let upgrade_check = || c.cli(RUNTIME, &cfg).arg("upgrade-check").output().unwrap();
    let checked = upgrade_check();
    assert!(checked.status.success());
    let versions = format!("Installed schema: {target}. Target schema: {target}.");
    assert_eq!(
        String::from_utf8(checked.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [versions.as_str(), "No pending migrations."]
    );
    assert!(
        sqlx::query("SELECT generation FROM relay_hosted.connection_secret_slots")
            .execute(&runtime)
            .await
            .is_err()
    );
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT has_function_privilege('fixture_setup','relay_hosted.owner_connections(text,bytea,uuid)','EXECUTE')
                 OR has_function_privilege('fixture_setup','relay_hosted.owner_connections(text,bytea,uuid,text)','EXECUTE')
                 OR has_function_privilege('fixture_runtime','relay_hosted.connection_page(uuid,uuid,text)','EXECUTE')",
        )
        .fetch_one(&bootstrap)
        .await
        .unwrap()
    );
    sqlx::query("GRANT SELECT (passkey) ON relay_hosted.admin_credentials TO fixture_runtime")
        .execute(&bootstrap)
        .await
        .unwrap();
    assert!(db::check_runtime(&runtime, &origin).await.is_err());
    sqlx::query("REVOKE SELECT (passkey) ON relay_hosted.admin_credentials FROM fixture_runtime")
        .execute(&bootstrap)
        .await
        .unwrap();
    sqlx::query("GRANT EXECUTE ON FUNCTION relay_hosted.post_verified_payment(uuid,text,text,text,bigint) TO fixture_runtime")
        .execute(&bootstrap).await.unwrap();
    assert!(db::check_runtime(&runtime, &origin).await.is_err());
    sqlx::query("REVOKE EXECUTE ON FUNCTION relay_hosted.post_verified_payment(uuid,text,text,text,bigint) FROM fixture_runtime")
        .execute(&bootstrap).await.unwrap();
    assert!(db::check_runtime(&runtime, &origin).await.is_ok());
    sqlx::query(
        "GRANT EXECUTE ON FUNCTION relay_hosted.claim_payment_refund(uuid) TO fixture_runtime",
    )
    .execute(&bootstrap)
    .await
    .unwrap();
    assert!(db::check_runtime(&runtime, &origin).await.is_err());
    sqlx::query(
        "REVOKE EXECUTE ON FUNCTION relay_hosted.claim_payment_refund(uuid) FROM fixture_runtime",
    )
    .execute(&bootstrap)
    .await
    .unwrap();
    assert!(db::check_runtime(&runtime, &origin).await.is_ok());
    sqlx::query("REVOKE EXECUTE ON FUNCTION relay_hosted.customer_wallet(text,uuid,bytea,bytea) FROM fixture_runtime")
        .execute(&bootstrap).await.unwrap();
    assert!(db::check_runtime(&runtime, &origin).await.is_err());
    sqlx::query("GRANT EXECUTE ON FUNCTION relay_hosted.customer_wallet(text,uuid,bytea,bytea) TO fixture_runtime")
        .execute(&bootstrap).await.unwrap();
    assert!(db::check_runtime(&runtime, &origin).await.is_ok());
    sqlx::query("REVOKE EXECUTE ON FUNCTION relay_hosted.owner_text_price_inputs(text,bytea,uuid,boolean) FROM fixture_runtime")
        .execute(&bootstrap).await.unwrap();
    assert!(db::check_runtime(&runtime, &origin).await.is_err());
    sqlx::query("GRANT EXECUTE ON FUNCTION relay_hosted.owner_text_price_inputs(text,bytea,uuid,boolean) TO fixture_runtime")
        .execute(&bootstrap).await.unwrap();
    sqlx::query("GRANT EXECUTE ON FUNCTION relay_hosted.text_price_inputs(relay_hosted.text_inventory,boolean) TO fixture_runtime")
        .execute(&bootstrap).await.unwrap();
    assert!(db::check_runtime(&runtime, &origin).await.is_err());
    sqlx::query("REVOKE EXECUTE ON FUNCTION relay_hosted.text_price_inputs(relay_hosted.text_inventory,boolean) FROM fixture_runtime")
        .execute(&bootstrap).await.unwrap();
    assert!(db::check_runtime(&runtime, &origin).await.is_ok());
    assert!(
        db::check_runtime(&runtime, "https://another.example")
            .await
            .is_err()
    );
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT relay_hosted.bind_first_owner(id, authorization_epoch, public_origin,
            gen_random_uuid(), 'fixture', '{}', 0) FROM relay_hosted.pending_owner",
        )
        .fetch_one(&bootstrap)
        .await
        .unwrap()
    );
    assert!(
        sqlx::query("UPDATE relay_hosted.service_control SET revision=2")
            .execute(&runtime)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("SELECT * FROM relay_hosted.installations")
            .execute(&runtime)
            .await
            .is_err()
    );

    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web");
    let with_web = std::env::var("RELAY_HOSTED_TEST_WEB").as_deref() == Ok("1");
    let serve = || {
        let mut cmd = c.cli(RUNTIME, &cfg);
        cmd.arg("serve");
        if with_web {
            cmd.arg("--web-dist").arg(web.join("dist"));
        }
        Process(cmd.spawn().unwrap())
    };
    let http = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let mut server = serve();
    support::ready(&mut server, &http, &url).await;
    let noauth = c.temp.path().join("noauth.toml");
    let other_port = support::port();
    fs::write(
        &noauth,
        toml.replace(
            &format!("bind = \"127.0.0.1:{port}\""),
            &format!("bind = \"127.0.0.1:{other_port}\""),
        )
        .lines()
        .filter(|l| !l.starts_with("[auth]") && !l.starts_with("admin_rp_id"))
        .collect::<Vec<_>>()
        .join("\n"),
    )
    .unwrap();
    let mut noauth_server = Process(c.cli(RUNTIME, &noauth).arg("serve").spawn().unwrap());
    let alt = format!("http://localhost:{other_port}/api/hosted/v1/");
    support::ready(&mut noauth_server, &http, &format!("{alt}public/config")).await;
    assert_eq!(
        http.post(format!("{alt}auth/admin/sign-in/options"))
            .header("origin", &origin)
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    drop(noauth_server);
    vault::verify(&c, &cfg, &config, &origin, &bootstrap, &runtime).await;
    let mut ids = Vec::new();
    for _ in 0..2 {
        let r = http
            .get(&url)
            .header("x-request-id", "caller")
            .header("x-forwarded-host", "foreign.example")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()["cache-control"], "no-store");
        assert!(!r.headers().contains_key("set-cookie"));
        let v = r.json::<Value>().await.unwrap();
        let id = Uuid::parse_str(v["meta"]["request_id"].as_str().unwrap()).unwrap();
        ids.push(id);
        assert_eq!(
            v,
            json!({"data":{"display_name":"Fixture Site","login_methods":[],"studio_enabled":false,"registration_open":false,
            "policy_links":{"terms":"https://docs.example.org/terms"}},
            "meta":{"api_version":"1","request_id":id}})
        );
    }
    assert_ne!(ids[0], ids[1]);
    for headers in [
        ("cookie", "__Host-relay_customer=fixture"),
        ("authorization", "Bearer fixture"),
    ] {
        assert_eq!(
            http.get(format!("{origin}/api/hosted/v1/admin/overview"))
                .header(headers.0, headers.1)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    let options = format!("{origin}/api/hosted/v1/auth/admin/sign-in/options");
    assert_eq!(http.post(&options).send().await.unwrap().status(), 403);
    let r = http
        .post(&options)
        .header("origin", &origin)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v = r.json::<Value>().await.unwrap();
    assert_eq!(v["data"]["publicKey"]["userVerification"], "required");
    assert_eq!(v["data"]["publicKey"]["allowCredentials"], json!([]));
    let endpoints = [
        ("/api/hosted/v1/setup/start", 404),
        ("/api/hosted/v1/admin/setup", 404),
        ("/api/hosted/v1/customer/keys", 404),
        ("/api/hosted/v1/payments", 401),
        ("/api/hosted/v1/payments/webhooks/fixture", 404),
        ("/v1/responses", 404),
        ("/api/hosted/v1/public/config", 405),
    ];
    for (path, status) in endpoints {
        let r = http.post(format!("{origin}{path}")).send().await.unwrap();
        assert_eq!(r.status().as_u16(), status);
        assert_eq!(
            r.json::<Value>().await.unwrap()["error"]["retryable"],
            false
        );
    }
    drop(server);
    assert!(migrate().wait().unwrap().success());
    let mut server = serve();
    support::ready(&mut server, &http, &url).await;
    assert_eq!(identity().await.unwrap(), original);
    if with_web {
        assert!(
            Command::new("bun")
                .current_dir(&web)
                .args(["tests/production-entry.mjs", &origin])
                .status()
                .expect("production browser test")
                .success()
        );
        drop(server);
        owner::verify(&c, &cfg, &origin, &bootstrap, &http, &serve).await;
        assert!(migrate().wait().unwrap().success());
        server = serve();
        support::ready(&mut server, &http, &url).await;
        assert_eq!(identity().await.unwrap(), original);
        assert_eq!(
            http.post(format!("{origin}/api/hosted/v1/setup/start"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }
    // Simulate a newer schema while this older web/server pair is running.
    sqlx::query("INSERT INTO relay_hosted._sqlx_migrations (version, description, success, checksum, execution_time) VALUES (9999, 'fixture', true, '', 0)")
        .execute(&bootstrap).await.unwrap();
    let unavailable = || http.get(&url).send();
    let r = unavailable().await.unwrap();
    assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        http.post(&options)
            .header("origin", &origin)
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    assert_eq!(
        r.json::<Value>().await.unwrap()["error"]["code"],
        "unavailable"
    );
    assert!(db::check_runtime(&runtime, &origin).await.is_err());
    let older = upgrade_check();
    assert!(!older.status.success());
    assert!(older.stdout.is_empty());
    assert_eq!(
        String::from_utf8(older.stderr).unwrap().trim(),
        "relay-hosted: database schema is newer than this binary"
    );
    drop(server);
    assert!(
        !c.cli(RUNTIME, &cfg)
            .arg("serve")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!migrate().wait().unwrap().success());
    // A running reader must also fail closed on actual loss of its database connection.
    sqlx::query("DELETE FROM relay_hosted._sqlx_migrations WHERE version=9999")
        .execute(&bootstrap)
        .await
        .unwrap();
    let mut server = serve();
    support::ready(&mut server, &http, &url).await;
    runtime.close().await;
    bootstrap.close().await;
    c.stop();
    let r = unavailable().await.unwrap();
    assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["retryable"], true);
    assert_eq!(
        http.get(format!("{origin}/api/hosted/v1/admin/overview"))
            .header("cookie", format!("__Host-relay_admin={}", "ab".repeat(32)))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
}

async fn verify_upgrade_plan(db: &PgPool, origin: &str) -> i64 {
    let plan = db::upgrade_plan(db, origin).await.unwrap();
    assert_eq!(plan.installed, 35);
    assert!(plan.target > plan.installed);
    assert_eq!(plan.pending, (36..=plan.target).collect::<Vec<_>>());
    assert_eq!(
        db::upgrade_plan(db, "https://other.example").await,
        Err("incompatible installation or public origin")
    );
    let version: i64 = sqlx::query_scalar("SELECT max(version) FROM relay_hosted._sqlx_migrations")
        .fetch_one(db)
        .await
        .unwrap();
    assert_eq!(version, plan.installed);

    for (corrupt, expected, restore) in [
        (
            "UPDATE relay_hosted._sqlx_migrations SET success=false WHERE version=35",
            "migration history contains a failed migration",
            "UPDATE relay_hosted._sqlx_migrations SET success=true WHERE version=35",
        ),
        (
            "UPDATE relay_hosted._sqlx_migrations SET version=36 WHERE version=34",
            "migration history does not match this binary",
            "UPDATE relay_hosted._sqlx_migrations SET version=34 WHERE version=36",
        ),
    ] {
        assert_eq!(
            sqlx::query(corrupt)
                .execute(db)
                .await
                .unwrap()
                .rows_affected(),
            1
        );
        assert_eq!(db::upgrade_plan(db, origin).await, Err(expected));
        assert_eq!(
            sqlx::query(restore)
                .execute(db)
                .await
                .unwrap()
                .rows_affected(),
            1
        );
    }
    let checksum: Vec<u8> =
        sqlx::query_scalar("SELECT checksum FROM relay_hosted._sqlx_migrations WHERE version=35")
            .fetch_one(db)
            .await
            .unwrap();
    sqlx::query("UPDATE relay_hosted._sqlx_migrations SET checksum='\\x01' WHERE version=35")
        .execute(db)
        .await
        .unwrap();
    assert_eq!(
        db::upgrade_plan(db, origin).await,
        Err("migration history does not match this binary")
    );
    sqlx::query("UPDATE relay_hosted._sqlx_migrations SET checksum=$1 WHERE version=35")
        .bind(checksum)
        .execute(db)
        .await
        .unwrap();
    assert_eq!(db::upgrade_plan(db, origin).await.unwrap(), plan);
    plan.target
}

async fn legacy_wallet(db: &PgPool, workspace: Uuid) -> (i64, i64, i64, Uuid) {
    sqlx::query_as(
        "SELECT w.available_micro_usd,w.reserved_micro_usd,w.revision,j.id
         FROM relay_hosted.wallets w JOIN relay_hosted.journal_transactions j USING(workspace_id)
         WHERE w.workspace_id=$1 AND j.operation_kind='payment_credit'",
    )
    .bind(workspace)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn legacy_text_listing(db: &PgPool) -> (Uuid, Uuid, Value) {
    sqlx::raw_sql(
        "WITH actor AS (INSERT INTO relay_hosted.users(id) VALUES(gen_random_uuid()) RETURNING id),
         owner AS (INSERT INTO relay_hosted.admin_assignments(user_id,role) SELECT id,'owner' FROM actor RETURNING user_id),
         issuer AS (INSERT INTO relay_hosted.identity_providers VALUES
             (gen_random_uuid(),'https://identity.example','fixture',decode(repeat('01',32),'hex')) RETURNING id),
         scope AS (INSERT INTO relay_hosted.configuration_revisions SELECT gen_random_uuid(),2,1,'qualification',
             'verify_customer_access',owner.user_id,issuer.id,ARRAY[gen_random_uuid()],now(),now()+interval '1 hour'
             FROM owner,issuer RETURNING id)
         UPDATE relay_hosted.service_control SET mode='qualification',revision=2,qualification_scope_ref=(SELECT id FROM scope);
         INSERT INTO relay_hosted.vault_bindings SELECT id,1,decode(repeat('02',32),'hex') FROM relay_hosted.installations;
         INSERT INTO relay_hosted.connection_secret_slots SELECT installation_id,gen_random_uuid(),'provider',1,
             gen_random_uuid(),decode(repeat('03',32),'hex') FROM relay_hosted.vault_bindings;
         INSERT INTO relay_hosted.text_inventory(id,scope_id,source_id,provider_revision,proxy_revision,uses_proxy,
             base_url,model,source_label,max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,close_after_s,valid_until)
         SELECT gen_random_uuid(),s.qualification_scope_ref,v.connection_id,1,0,false,'http://127.0.0.1:38888/v1',
             'fixture-legacy','Fixture',4000,64,1,0,65,now()+interval '30 minutes'
         FROM relay_hosted.connection_secret_slots v JOIN relay_hosted.service_control s USING(installation_id);
         INSERT INTO relay_hosted.text_offers(id,inventory_id,scope_id,source_id,provider_revision,base_url,model,revision,
             max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,input_rate,cached_rate,output_rate,close_after_s)
         SELECT gen_random_uuid(),id,scope_id,source_id,provider_revision,base_url,model,1,max_input_bytes,max_output_tokens,
             tokens_per_byte,overhead_tokens,0,0,0,close_after_s FROM relay_hosted.text_inventory;
         INSERT INTO relay_hosted.text_model_rules(id,scope_id,model,revision,admitted,draft,last_offer,enabled,first_live_revision)
         SELECT inventory_id,scope_id,model,3,true,relay_hosted.text_rates(o),id,true,revision FROM relay_hosted.text_offers o;",
    )
    .execute(db)
    .await
    .unwrap();
    sqlx::query_as("SELECT l.item,l.id,relay_hosted.text_rates(o) FROM relay_hosted.active_text_listings l JOIN relay_hosted.text_offers o USING(id)")
        .fetch_one(db)
        .await
        .unwrap()
}
