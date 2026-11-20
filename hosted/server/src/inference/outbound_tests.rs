#[path = "../../tests/support/cluster.rs"]
mod cluster;

use super::*;
use crate::config::Config;
use std::fs;

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL cluster; set RELAY_TEST_PG_BIN"]
async fn outbound_observation_requires_the_same_pin_after_worker_restart() {
    const BASE: &str = include_str!("../../config.example.toml");
    const ENDPOINT: &str = "https://fixture.example/v1";
    // Only config and database data: no external IP is contacted.
    const PIN: &str = "1.1.1.1";
    const CHANGED: &str = "1.0.0.1";
    let c = cluster::Cluster::new();
    let source = Uuid::new_v4();
    let config_for_pin = |pin| {
        Config::parse(&format!(
            "{BASE}\n[[network.outbound_policy.text_sources]]\nsource = '{source}'\nendpoint = '{ENDPOINT}'\nipv4 = '{pin}'\n"
        ))
        .unwrap()
    };
    let config = config_for_pin(PIN);
    let origin = &config.server.public_origin;
    let admin = PgPool::connect(&c.dsn("fixture_boot")).await.unwrap();
    db::migrate(&admin, origin).await.unwrap();
    c.create_logins(&admin, &["test_vault", "test_qualifier", "test_inference"])
        .await;
    for (sql, var, role) in [
        (
            include_str!("../../vault-grants.sql"),
            ":\"vault_role\"",
            "test_vault",
        ),
        (
            include_str!("../../qualification-grants.sql"),
            ":\"qualification_role\"",
            "test_qualifier",
        ),
        (
            include_str!("../../inference-grants.sql"),
            ":\"inference_role\"",
            "test_inference",
        ),
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.replace(var, role)))
            .execute(&admin)
            .await
            .unwrap();
    }
    let scope = Uuid::new_v4();
    sqlx::query(
        "WITH actor AS (INSERT INTO relay_hosted.users(id) VALUES(gen_random_uuid()) RETURNING id),
         owner AS (INSERT INTO relay_hosted.admin_assignments(user_id,role) SELECT id,'owner' FROM actor RETURNING user_id),
         issuer AS (INSERT INTO relay_hosted.identity_providers VALUES (gen_random_uuid(),'https://identity.example','fixture',$1) RETURNING id),
         scoped AS (INSERT INTO relay_hosted.configuration_revisions
            SELECT $2,2,1,'qualification','verify_customer_access',owner.user_id,issuer.id,ARRAY[gen_random_uuid()],
                now(),now()+interval '1 hour' FROM owner,issuer RETURNING id)
         UPDATE relay_hosted.service_control SET mode='qualification',revision=2,qualification_scope_ref=(SELECT id FROM scoped)",
    )
    .bind(&[42u8; 32][..])
    .bind(scope)
    .execute(&admin)
    .await
    .unwrap();
    let root = c.temp.path().join("vault");
    let key = c.temp.path().join("vault-key");
    fs::create_dir(&root).unwrap();
    fs::write(&key, [91; 32]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let vp = db::connect(&config, &c.dsn("test_vault")).await.unwrap();
    let vault = Vault::open(&vp, origin, &root, &key).await.unwrap();
    vault
        .put(source, Purpose::Provider, 0, b"fixture-key")
        .await
        .unwrap();
    let qp = db::connect(&config, &c.dsn("test_qualifier"))
        .await
        .unwrap();
    assert_eq!(
        crate::qualification::scope(&qp, origin).await.unwrap().id(),
        scope
    );
    let item = Uuid::new_v4();
    let profile = json!({
        "id":item,"scope":scope,"source":source,"source_label":"Fixture","endpoint":ENDPOINT,
        "model":"fixture-text","provider_revision":1,"proxy_revision":0,"uses_proxy":false,
        "max_input_bytes":4000,"max_output_tokens":64,"tokens_per_byte":1,"overhead_tokens":0,
        "close_after_s":65,"valid_for_s":3600,"direct_ipv4":PIN
    });
    let step = |profile: &Value, action| {
        sqlx::query_scalar::<_, String>(
            "SELECT relay_hosted.text_check_step($1,$2,$3,$4,1,0,false,$5,$6)",
        )
        .bind(origin)
        .bind(scope)
        .bind(item)
        .bind(source)
        .bind(sqlx::types::Json(profile.clone()))
        .bind(action)
    };
    assert_eq!(
        step(&profile, "prepare").fetch_one(&qp).await.unwrap(),
        "prepared"
    );
    let mut different = profile.clone();
    different["direct_ipv4"] = json!(CHANGED);
    assert_eq!(
        step(&different, "claim").fetch_one(&qp).await.unwrap(),
        "conflict"
    );
    assert_eq!(
        step(&profile, "claim").fetch_one(&qp).await.unwrap(),
        "claimed"
    );
    let record = |ip: Option<&'static str>| {
        sqlx::query_scalar::<_, bool>("SELECT relay_hosted.record_text_check($1,$2,$3,$4,1,0,false,$5,'fixture-text','Fixture',4000,64,1,0,65,3600,$6::text::inet)")
            .bind(origin).bind(scope).bind(item).bind(source).bind(ENDPOINT).bind(ip)
    };
    assert!(!record(None).fetch_one(&qp).await.unwrap());
    assert!(!record(Some(CHANGED)).fetch_one(&qp).await.unwrap());
    assert!(record(Some(PIN)).fetch_one(&qp).await.unwrap());
    assert_eq!(
        step(&profile, "prepare").fetch_one(&qp).await.unwrap(),
        "done"
    );
    let offer = Uuid::new_v4();
    sqlx::query(
        "WITH purchase AS (
          INSERT INTO relay_hosted.text_purchase_rates(item,revision,input_rate,cached_rate,output_rate)
          VALUES($2,1,1,0,1) RETURNING revision
         ), published AS (
          INSERT INTO relay_hosted.text_offers (id,inventory_id,scope_id,source_id,provider_revision,base_url,model,revision,
              max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,input_rate,cached_rate,output_rate,close_after_s)
          SELECT $1,id,scope_id,source_id,provider_revision,base_url,model,1,max_input_bytes,max_output_tokens,
              tokens_per_byte,overhead_tokens,1,0,1,close_after_s FROM relay_hosted.text_inventory WHERE id=$2 RETURNING id)
         UPDATE relay_hosted.text_model_rules SET admitted=true,enabled=true,first_live_revision=1,
             last_offer=(SELECT id FROM published),purchase_revision=(SELECT revision FROM purchase) WHERE id=$2",
    )
    .bind(offer)
    .bind(item)
    .execute(&admin)
    .await
    .unwrap();

    qp.close().await;
    vp.close().await;
    admin.close().await;
    c.stop();
    c.start();
    let vp = db::connect(&config, &c.dsn("test_vault")).await.unwrap();
    let vault = Vault::open(&vp, origin, &root, &key).await.unwrap();
    let wp = db::connect(&config, &c.dsn("test_inference"))
        .await
        .unwrap();
    check_worker(&wp, origin).await.unwrap();
    let worker = |outbound_policy| Ingress {
        store: Arc::new(Store::new(
            wp.clone(),
            origin.clone(),
            hmac::Key::new(hmac::HMAC_SHA256, &[27; 32]),
            None,
        )),
        vault: vault.clone(),
        outbound_policy,
        runtimes: Default::default(),
        selection: Selection::default(),
        capacity: Arc::new(tokio::sync::Semaphore::new(1)),
    };
    let admin = PgPool::connect(&c.dsn("fixture_boot")).await.unwrap();
    let mut config: Value = sqlx::query_scalar("SELECT relay_hosted.text_offer_config($1,$2)")
        .bind(offer)
        .bind(origin)
        .fetch_one(&admin)
        .await
        .unwrap();
    config["cost"] = json!(8);
    config["rate_revision"] = json!(1);
    config["busy"] = json!(false);
    let approved: store::Route = serde_json::from_value(config).unwrap();
    let initial = worker(config_for_pin(PIN).network.outbound_policy);
    routing::runtime(&initial, &approved).await.unwrap();
    assert_eq!(approved.direct_ipv4.unwrap().to_string(), PIN);
    drop(initial);
    let changed = worker(config_for_pin(CHANGED).network.outbound_policy);
    assert_eq!(
        routing::runtime(&changed, &approved).await.err(),
        Some("outbound approval changed since qualification")
    );
    let removed = worker(OutboundPolicy::default());
    assert_eq!(
        routing::runtime(&removed, &approved).await.err(),
        Some("text source has no outbound approval")
    );
}
