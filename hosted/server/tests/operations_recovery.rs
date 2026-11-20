#[allow(dead_code)]
#[path = "support/cluster.rs"]
mod cluster;
mod fixture_files;

use serde_json::{Value, json};
use sqlx::PgPool;
use zenith_relay_hosted::{config::Config, db, inference, operations::Reconciliation};

const ORIGIN: &str = "http://localhost:1443";
const TAG: &[u8] = &[72; 32];

async fn status(pool: &PgPool, origin: &str, tag: &[u8]) -> Option<Value> {
    sqlx::query_scalar("SELECT relay_hosted.owner_operations_status($1,$2)")
        .bind(origin)
        .bind(tag)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn events(pool: &PgPool, before: Option<i64>) -> Value {
    sqlx::query_scalar("SELECT relay_hosted.owner_operations_events($1,$2,$3)")
        .bind(ORIGIN)
        .bind(TAG)
        .bind(before)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn job(pool: &PgPool, name: &str) -> Value {
    status(pool, ORIGIN, TAG).await.unwrap()["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["job"] == name)
        .unwrap()
        .clone()
}

async fn sql(pool: &PgPool, text: &'static str) {
    sqlx::raw_sql(text).execute(pool).await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn owner_observations_are_scoped_grouped_bounded_and_independent() {
    let c = cluster::Cluster::new();
    let admin = PgPool::connect(&c.dsn("fixture_boot")).await.unwrap();
    let config = Config::parse(include_str!("../config.example.toml")).unwrap();
    db::migrate(&admin, ORIGIN).await.unwrap();
    c.create_logins(&admin, &["fixture_runtime", "fixture_inference"])
        .await;
    sql(
        &admin,
        "GRANT USAGE ON SCHEMA relay_hosted TO fixture_runtime;
         GRANT SELECT ON relay_hosted.closed_installation,relay_hosted._sqlx_migrations TO fixture_runtime;
         WITH owner AS (INSERT INTO relay_hosted.users(id) VALUES(gen_random_uuid()) RETURNING id)
         INSERT INTO relay_hosted.admin_assignments(user_id,role) SELECT id,'owner' FROM owner;
         UPDATE relay_hosted.installations SET setup_state='claimed',
            owner_user_id=(SELECT user_id FROM relay_hosted.admin_assignments);
         INSERT INTO relay_hosted.admin_credentials(credential_id,user_id,passkey)
            SELECT '\\x01'::bytea,user_id,'{}' FROM relay_hosted.admin_assignments;",
    )
    .await;
    for (text, name, role) in [
        (
            include_str!("../runtime-grants.sql"),
            ":\"runtime_role\"",
            "fixture_runtime",
        ),
        (
            include_str!("../inference-grants.sql"),
            ":\"inference_role\"",
            "fixture_inference",
        ),
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(text.replace(name, role)))
            .execute(&admin)
            .await
            .unwrap();
    }
    let idle: String = sqlx::query_scalar(
        "INSERT INTO relay_hosted.admin_sessions SELECT $1,user_id,credential_id,installation_epoch,
            user_epoch,now(),now(),now()+interval '8 hours' FROM relay_hosted.active_owner_credentials
         RETURNING last_seen_at::text",
    )
    .bind(TAG)
    .fetch_one(&admin)
    .await
    .unwrap();
    let runtime = db::connect(&config, &c.dsn("fixture_runtime"))
        .await
        .unwrap();
    let worker = db::connect(&config, &c.dsn("fixture_inference"))
        .await
        .unwrap();
    db::check_runtime(&runtime, ORIGIN).await.unwrap();
    inference::check_worker(&worker, ORIGIN).await.unwrap();

    assert_eq!(status(&runtime, ORIGIN, &[41; 32]).await, None);
    assert_eq!(status(&runtime, "https://other.example", TAG).await, None);
    for (pool, statement) in [
        (&runtime, "SELECT * FROM relay_hosted.operation_events"),
        (
            &runtime,
            "SELECT relay_hosted.observe_reconciliation(true,false)",
        ),
        (
            &runtime,
            "SELECT relay_hosted.observe_operation('request_reconciliation',true,false)",
        ),
        (
            &worker,
            "SELECT relay_hosted.owner_operations_events('http://localhost:1443','\\x01',NULL)",
        ),
        (
            &worker,
            "SELECT relay_hosted.observe_projection_cleanup(true,true,true,true,true,false)",
        ),
    ] {
        let error = sqlx::raw_sql(statement).execute(pool).await.unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("42501")
        );
    }
    let initial = status(&runtime, ORIGIN, TAG).await.unwrap();
    let jobs = initial["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 6);
    assert!(
        jobs.iter()
            .all(|j| j["state"] == "unobserved" && j["observed_at_ms"].is_null())
    );
    assert_eq!(
        events(&runtime, None).await,
        json!({"version":1,"partial":true,"items":[],"next_before":null})
    );

    sql(
        &runtime,
        "SELECT relay_hosted.observe_projection_cleanup(true,true,false,true,true,false)",
    )
    .await;
    assert_eq!(
        job(&runtime, "checkout_quote_cleanup").await["state"],
        "failed"
    );
    assert_eq!(
        job(&runtime, "checkout_resume_cleanup").await["state"],
        "succeeded"
    );
    let mut reconcile = Reconciliation::default();
    assert_eq!(reconcile.run(&worker).await.unwrap(), 0);
    assert_eq!(
        job(&runtime, "request_reconciliation").await["state"],
        "succeeded"
    );
    assert_eq!(events(&runtime, None).await["partial"], false);

    sql(
        &admin,
        "REVOKE EXECUTE ON FUNCTION relay_hosted.due_text() FROM fixture_inference",
    )
    .await;
    for _ in 0..2 {
        let error = reconcile.run(&worker).await.unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("42501")
        );
    }
    let failed = events(&runtime, None).await;
    assert_eq!(failed["items"].as_array().unwrap().len(), 2);
    assert_eq!(failed["items"][0]["job"], "request_reconciliation");
    assert_eq!(failed["items"][0]["observations"], 2);
    assert_eq!(
        job(&runtime, "request_reconciliation").await["state"],
        "failed"
    );

    sql(
        &admin,
        "GRANT EXECUTE ON FUNCTION relay_hosted.due_text() TO fixture_inference;
         REVOKE EXECUTE ON FUNCTION relay_hosted.observe_reconciliation(boolean,boolean) FROM fixture_inference;",
    ).await;
    // A missing optional sink grant neither prevents startup nor changes a pass.
    inference::check_worker(&worker, ORIGIN).await.unwrap();
    assert_eq!(reconcile.run(&worker).await.unwrap(), 0);
    assert_eq!(events(&runtime, None).await, failed);
    sql(&admin, "GRANT EXECUTE ON FUNCTION relay_hosted.observe_reconciliation(boolean,boolean) TO fixture_inference").await;
    assert_eq!(reconcile.run(&worker).await.unwrap(), 0);
    let recovered = events(&runtime, None).await;
    assert_eq!(recovered["items"][0]["code"], "job_recovered");
    assert_eq!(recovered["items"][1], failed["items"][0]);
    assert_eq!(recovered["partial"], true);
    assert_eq!(
        job(&runtime, "request_reconciliation").await["state"],
        "succeeded"
    );
    sql(
        &admin,
        "UPDATE relay_hosted.operation_jobs SET observed_at=now()-interval '21 seconds'
            WHERE job='request_reconciliation';",
    )
    .await;
    assert_eq!(
        job(&runtime, "request_reconciliation").await["state"],
        "stale"
    );
    assert_eq!(events(&runtime, None).await["partial"], true);
    // Reconnect from a fresh pool/collector; saved events and coverage are durable.
    let reconnected = db::connect(&config, &c.dsn("fixture_inference"))
        .await
        .unwrap();
    assert_eq!(
        Reconciliation::default().run(&reconnected).await.unwrap(),
        0
    );
    assert_eq!(events(&runtime, None).await, recovered);

    sql(
        &worker,
        "DO $$ BEGIN FOR i IN 1..70 LOOP
            PERFORM relay_hosted.observe_reconciliation(false,false);
            PERFORM relay_hosted.observe_reconciliation(true,false);
        END LOOP; END $$;",
    )
    .await;
    let mut before = None;
    let mut ids = Vec::<i64>::new();
    loop {
        let page = events(&runtime, before).await;
        let items = page["items"].as_array().unwrap();
        assert!(items.len() <= 24);
        ids.extend(
            items
                .iter()
                .map(|r| r["id"].as_str().unwrap().parse::<i64>().unwrap()),
        );
        before = page["next_before"].as_str().map(|v| v.parse().unwrap());
        if before.is_none() {
            break;
        }
        assert_eq!(before, ids.last().copied());
        assert!(ids.len() < 129);
    }
    assert_eq!(ids.len(), 129); // 128 groups for reconciliation and one from quote cleanup.
    assert!(ids.windows(2).all(|pair| pair[0] > pair[1]));
    assert_eq!(
        events(&runtime, Some(2)).await["error"],
        "cursor_unavailable"
    );

    sql(&admin, "UPDATE relay_hosted.operation_events SET first_at=now()-interval '337 hours',last_at=now()-interval '337 hours'").await;
    assert_eq!(
        events(&runtime, Some(ids[23])).await["error"],
        "cursor_unavailable"
    );
    assert_eq!(events(&runtime, None).await["items"], json!([]));
    sql(
        &runtime,
        "SELECT relay_hosted.observe_projection_cleanup(true,true,false,true,true,false)",
    )
    .await;
    let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM relay_hosted.operation_events")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        retained, 1,
        "aged groups are pruned; continuing failure starts a new group"
    );
    assert_eq!(events(&runtime, None).await["items"][0]["observations"], 1);
    let last_seen: String =
        sqlx::query_scalar("SELECT last_seen_at::text FROM relay_hosted.admin_sessions")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(idle, last_seen);
    sql(
        &admin,
        "UPDATE relay_hosted.installations SET authorization_epoch=authorization_epoch+1",
    )
    .await;
    assert_eq!(status(&runtime, ORIGIN, TAG).await, None);
    let revoked: Option<Value> =
        sqlx::query_scalar("SELECT relay_hosted.owner_operations_events($1,$2,NULL)")
            .bind(ORIGIN)
            .bind(TAG)
            .fetch_one(&runtime)
            .await
            .unwrap();
    assert_eq!(revoked, None);
}
