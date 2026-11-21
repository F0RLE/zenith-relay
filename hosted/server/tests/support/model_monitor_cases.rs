use super::customer_reader::{Reader, add_workspace, setup_reader};
use super::*;

#[path = "model_monitor_collector.rs"]
mod collector;

struct Monitor {
    pool: PgPool,
    publication: Uuid,
    last_slot: i64,
}

impl Monitor {
    async fn new(f: &Fixture) -> Self {
        f._cluster.create_logins(&f.admin, &["model_monitor"]).await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(
            include_str!("../../model-monitor-grants.sql")
                .replace(":\"model_monitor_role\"", "model_monitor"),
        ))
        .execute(&f.admin)
        .await
        .unwrap();
        sqlx::query("UPDATE relay_hosted.configuration_revisions SET created_at=now()-interval '8 days' WHERE id=$1")
            .bind(f.scope).execute(&f.admin).await.unwrap();
        let last_slot = sqlx::query_scalar(
            "SELECT extract(epoch FROM date_bin(interval '30 minutes',statement_timestamp(),timestamptz 'epoch'))::bigint-1800",
        )
        .fetch_one(&f.admin)
        .await
        .unwrap();
        // Seed an already published test model without a clock override on the writer.
        let publication =
            publish_at(&f.admin, f.a.item, 2, None, 2, Some(last_slot - 8 * 86400)).await;
        Self {
            pool: db::connect(&fixture_config(), &f._cluster.dsn("model_monitor"))
                .await
                .unwrap(),
            publication,
            last_slot,
        }
    }

    async fn targets(&self, origin: &str) -> Value {
        sqlx::query_scalar("SELECT relay_hosted.model_monitor_targets($1)")
            .bind(origin)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn attest(&self, origin: &str, pub_id: Uuid, slot: i64, state: &str) -> String {
        sqlx::query_scalar(
            "SELECT relay_hosted.attest_model_status($1,$2,to_timestamp($3::bigint::double precision),$4)",
        )
        .bind(origin)
        .bind(pub_id)
        .bind(slot)
        .bind(state)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn model_monitor_accepts_exact_completed_slots_without_sends_or_customer_data() {
    let f = Fixture::new().await;
    setup_reader(&f).await;
    let a = Reader::new(&f, f.space, 1).await;
    let (space, _) = add_workspace(&f).await;
    let b = Reader::new(&f, space, 2).await;
    let m = Monitor::new(&f).await;
    let stamp: i64 = sqlx::query_scalar(
        "SELECT ceil(extract(epoch FROM created_at)*1000)::bigint FROM relay_hosted.text_offers WHERE id=$1",
    )
    .bind(m.publication)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    let empty = json!({"items":[]});
    let target =
        json!({"items":[{"publication_id":m.publication,"model":MODEL,"published_at_ms":stamp}]});
    assert_eq!(m.targets(ORIGIN).await, target);
    assert_eq!(m.targets("https://outside.invalid").await, empty);
    let attest = |id, slot, state| m.attest(ORIGIN, id, slot, state);
    for (id, slot, state, expected) in [
        (f.offer, m.last_slot, "operational", "inactive"),
        (m.publication, m.last_slot + 3600, "operational", "invalid"),
        (m.publication, m.last_slot - 1, "operational", "invalid"),
        (m.publication, m.last_slot - 7200, "operational", "invalid"),
        (m.publication, m.last_slot, "unknown", "invalid"),
    ] {
        assert_eq!(attest(id, slot, state).await, expected);
    }
    assert_eq!(
        m.attest(
            "https://outside.invalid",
            m.publication,
            m.last_slot,
            "operational"
        )
        .await,
        "inactive"
    );
    assert_eq!(
        a.read_status(24).await.unwrap()["summary"]["monitored_ms"],
        0
    );
    let (first, replay) = tokio::join!(
        attest(m.publication, m.last_slot, "operational"),
        attest(m.publication, m.last_slot, "operational"),
    );
    assert_eq!((first.as_str(), replay.as_str()), ("recorded", "recorded"));
    assert_eq!(
        attest(m.publication, m.last_slot, "unavailable").await,
        "conflict"
    );
    for (reader, hours) in [(&a, 24), (&b, 168)] {
        let status = reader.read_status(hours).await.unwrap();
        assert_eq!(status["summary"]["monitored_ms"], 1_800_000);
        assert_eq!(status["summary"]["operational_ms"], 1_800_000);
        assert_eq!(status["summary"]["requests"], 0);
        assert_eq!(status["items"][0]["model"], MODEL);
        assert!(!status.to_string().contains(&m.publication.to_string()));
    }
    let actions = [
        "SELECT relay_hosted.model_monitor_targets('http://localhost:1443')",
        "SELECT relay_hosted.attest_model_status('http://localhost:1443',gen_random_uuid(),now(),'unavailable')",
    ];
    denied_sql(&a.pool, actions).await;
    denied_sql(&f.worker, actions).await;
    denied_sql(
        &m.pool,
        [
            "SELECT * FROM relay_hosted.model_status_observations",
            "UPDATE relay_hosted.model_status_observations SET state='degraded'",
            "SELECT * FROM relay_hosted.customer_keys",
            "SELECT relay_hosted.prune_model_status_observations()",
        ],
    )
    .await;

    // Pricing does not reset monitoring; selling or probing is not a monitor grant.
    publish(&f.admin, f.a.item, 3, Some("2")).await;
    let set_epoch = "UPDATE relay_hosted.text_model_rules SET first_live_revision=$2 WHERE id=$1";
    sqlx::query(set_epoch)
        .bind(f.a.item)
        .bind(2_i64)
        .execute(&f.admin)
        .await
        .unwrap();
    assert_eq!(m.targets(ORIGIN).await, target);
    assert_eq!(
        a.read_status(24).await.unwrap()["summary"]["monitored_ms"],
        1_800_000
    );
    let mut withdraw = f.admin.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar(
        "UPDATE relay_hosted.text_model_rules SET enabled=false WHERE id=$1 RETURNING pg_backend_pid()",
    )
    .bind(f.a.item)
    .fetch_one(&mut *withdraw)
    .await
    .unwrap();
    let (racing, ()) = tokio::join!(attest(m.publication, m.last_slot, "operational"), async {
        supply_confirmation_race::wait_for_block(
            &f.admin,
            pid,
            "relay_hosted.attest_model_status(",
        )
        .await;
        withdraw.commit().await.unwrap();
    });
    assert_eq!(racing, "inactive");
    assert_eq!(m.targets(ORIGIN).await, empty);
    assert_eq!(a.read_status(24).await.unwrap()["items"], json!([]));

    let fresh = publish(&f.admin, f.a.item, 4, None).await;
    sqlx::query(set_epoch)
        .bind(f.a.item)
        .bind(4_i64)
        .execute(&f.admin)
        .await
        .unwrap();
    assert_eq!(
        m.targets(ORIGIN).await["items"][0]["publication_id"],
        json!(fresh)
    );
    assert_eq!(
        attest(m.publication, m.last_slot, "operational").await,
        "inactive"
    );
    assert_eq!(attest(fresh, m.last_slot, "operational").await, "invalid");
    assert_eq!(
        a.read_status(24).await.unwrap()["summary"]["monitored_ms"],
        0
    );
    for source in [&f.a, &f.b] {
        f.vault
            .revoke(source.id, Purpose::Provider, 1)
            .await
            .unwrap();
    }
    assert_eq!(a.read_status(24).await.unwrap()["items"], json!([]));
    assert_eq!(
        m.targets(ORIGIN).await["items"][0]["publication_id"],
        json!(fresh)
    );
    sqlx::query(
        "UPDATE relay_hosted.service_control SET mode='maintenance',qualification_scope_ref=NULL",
    )
    .execute(&f.admin)
    .await
    .unwrap();
    assert_eq!(m.targets(ORIGIN).await, empty);
    assert_eq!(attest(fresh, m.last_slot, "unavailable").await, "inactive");
    assert_eq!(f.sends(), (0, 0));
    payment::assert_wallet(&f.admin, f.space, CREDIT, 1).await;
}

async fn denied_sql(pool: &PgPool, queries: impl IntoIterator<Item = &'static str>) {
    for query in queries {
        let error = sqlx::query(query).execute(pool).await.expect_err(query);
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("42501"),
            "{query}"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn model_monitor_cleanup_retains_overlapping_slot_and_rejects_backfill() {
    let f = Fixture::new().await;
    setup_reader(&f).await;
    let a = Reader::new(&f, f.space, 1).await;
    let m = Monitor::new(&f).await;
    let overlap = m.last_slot + 1800 - 168 * 3600;
    let expired = overlap - 1800;
    let older = m.last_slot - 48 * 3600;
    let seeded = [expired, overlap, overlap + 1800, older];
    sqlx::query(
        "INSERT INTO relay_hosted.model_status_observations(publication_id,slot_at,state)
         SELECT $1,to_timestamp(slot::double precision),'degraded' FROM unnest($2::bigint[]) AS slot",
    )
    .bind(m.publication)
    .bind(seeded.as_slice())
    .execute(&f.admin)
    .await
    .unwrap();
    let cutoff: i64 = sqlx::query_scalar(
        "SELECT floor(extract(epoch FROM statement_timestamp()-interval '168 hours 30 minutes'))::bigint
         FROM relay_hosted.prune_model_status_observations()",
    )
    .fetch_one(&a.pool)
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT extract(epoch FROM slot_at)::bigint FROM relay_hosted.model_status_observations ORDER BY slot_at",
        )
            .fetch_all(&f.admin).await.unwrap(),
        seeded.into_iter().filter(|slot| *slot > cutoff).collect::<Vec<_>>()
    );
    for (slot, state, expected) in [
        (expired, "degraded", "invalid"),
        (older, "operational", "conflict"),
        (older, "degraded", "recorded"),
        (m.last_slot - 3600, "degraded", "invalid"),
    ] {
        assert_eq!(m.attest(ORIGIN, m.publication, slot, state).await, expected);
    }
    assert_eq!(f.sends(), (0, 0));
}
