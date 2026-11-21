use super::*;
#[path = "device_metric_cases.rs"]
mod metrics;
#[path = "device_pool_cases.rs"]
mod pool;

struct Device {
    installation: Uuid,
    id: Uuid,
}

const PROOF: &[u8] = &[73; 32];

impl Device {
    async fn save(f: &Fixture, scopes: &[&str]) -> Self {
        let (installation, id) = sqlx::query_as("INSERT INTO relay_hosted.management_devices
            (id,installation_id,owner_id,installation_epoch,owner_epoch,label,scope,
                revision,invited_at,approve_by,approved_at,expires_at,verifier)
            SELECT gen_random_uuid(),h.id,a.user_id,a.installation_epoch,a.user_epoch,'Fixture Desktop','connections.read',
                2,now(),now()+interval '5 minutes',now(),now()+interval '24 hours',$1
            FROM relay_hosted.active_owner_credentials a JOIN relay_hosted.installations h USING(public_origin)
            RETURNING installation_id,id")
            .bind(PROOF).fetch_one(&f.admin).await.unwrap();
        if !scopes.is_empty() {
            sqlx::query("UPDATE relay_hosted.management_devices SET read_scopes=$2 WHERE id=$1")
                .bind(id)
                .bind(scopes)
                .execute(&f.admin)
                .await
                .unwrap();
        }
        Self { installation, id }
    }

    async fn overview(&self, pool: &PgPool, origin: &str) -> Option<Value> {
        sqlx::query_scalar("SELECT relay_hosted.device_overview($1,$2,$3,$4,24)")
            .bind(origin)
            .bind(self.installation)
            .bind(self.id)
            .bind(PROOF)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn usage(
        &self,
        pool: &PgPool,
        filter: Option<&str>,
        anchor: Option<i64>,
        before: Option<Uuid>,
    ) -> Option<Value> {
        sqlx::query_scalar("SELECT relay_hosted.device_usage($1,$2,$3,$4,24,$5,'failed',$6,$7)")
            .bind(ORIGIN)
            .bind(self.installation)
            .bind(self.id)
            .bind(PROOF)
            .bind(filter)
            .bind(anchor)
            .bind(before)
            .fetch_one(pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn desktop_grants_are_independent_and_observations_project_only_safe_evidence() {
    let f = Fixture::new().await;
    let pool = owner_usage_cases::owner(&f).await;
    let legacy = Device::save(&f, &[]).await;
    let overview = Device::save(&f, &["overview.read"]).await;
    let usage = Device::save(&f, &["usage.read"]).await;
    let access = |d: &Device| {
        sqlx::query_scalar::<_, Value>("SELECT relay_hosted.device_access($1,$2,$3,$4)")
            .bind(ORIGIN)
            .bind(d.installation)
            .bind(d.id)
            .bind(PROOF)
    };
    assert_eq!(
        access(&legacy).fetch_one(&pool).await.unwrap()["result"],
        json!([])
    );
    assert!(legacy.overview(&pool, ORIGIN).await.is_none());
    assert!(legacy.usage(&pool, None, None, None).await.is_none());
    assert!(overview.usage(&pool, None, None, None).await.is_none());
    assert!(usage.overview(&pool, ORIGIN).await.is_none());
    assert!(
        overview
            .overview(&pool, "https://other.example")
            .await
            .is_none()
    );
    assert_eq!(
        overview.overview(&pool, ORIGIN).await.unwrap()["result"]["total_tokens"],
        "0"
    );
    assert!(
        sqlx::query("SELECT relay_hosted.device_read_context(NULL,NULL,NULL,NULL,NULL)")
            .execute(&pool)
            .await
            .is_err()
    );

    // Ties, a stale row outside the rolling window, one active queue entry and one in-flight attempt.
    let seed = |batch: i32, model: Uuid, execution: &str, at: &str| {
        sqlx::query("INSERT INTO relay_hosted.text_requests(workspace_id,key_id,offer_id,authority_epoch,input_tag,send_tag,
            input_bound,output_bound,held,started_at,claim_by,close_by,claimed_at,execution,accounting,input_tokens,output_tokens)
            SELECT k.workspace_id,k.id,$2,1,k.verifier,k.verifier,1,1,0,stamp,stamp+interval '1 hour',
                stamp+interval '2 hours',CASE WHEN $4='dispatched' THEN stamp END,$4,'held',
                CASE WHEN $4='failed' THEN 0 END,CASE WHEN $4='failed' THEN 1 END
            FROM relay_hosted.customer_keys k CROSS JOIN generate_series(1,$3)
            CROSS JOIN (SELECT date_trunc('milliseconds',statement_timestamp())+$5::text::interval AS stamp) s WHERE k.id=$1")
            .bind(f.key).bind(model).bind(batch).bind(execution.to_owned()).bind(at.to_owned())
    };
    seed(35, f.offer, "failed", "-2 minutes")
        .execute(&f.admin)
        .await
        .unwrap();
    seed(1, f.offer, "failed", "-25 hours")
        .execute(&f.admin)
        .await
        .unwrap();
    seed(1, f.offer, "reserved", "-2 minutes")
        .execute(&f.admin)
        .await
        .unwrap();
    seed(1, f.offer, "dispatched", "-2 minutes")
        .execute(&f.admin)
        .await
        .unwrap();
    sqlx::query("SELECT relay_hosted.record_request_diagnostic(r.offer_id,r.id,r.attempt_id,
        'hosted_dispatch_unavailable',NULL,NULL) FROM relay_hosted.text_requests r WHERE r.execution='failed'")
        .execute(&f.admin).await.unwrap();

    let stats = overview.overview(&pool, ORIGIN).await.unwrap();
    let totals = &stats["result"];
    assert_eq!(totals["requests"], "37");
    assert_eq!(totals["errors"], "35");
    assert_eq!(totals["token_samples"], "35");
    assert_eq!(totals["partial_tokens"], "35");
    assert_eq!(totals["total_tokens"], "35");
    assert_eq!(totals["active_requests"], "2");
    assert_eq!(totals["active_attempts"], "1");
    assert!(totals.get("spend_micro_usd").is_none());

    let first = usage.usage(&pool, Some(MODEL), None, None).await.unwrap();
    let result = &first["result"];
    let at = result["as_of_ms"].as_i64().unwrap();
    let rows = result["items"].as_array().unwrap();
    assert_eq!(rows.len(), 32);
    let before = serde_json::from_value(result["next_before"].clone()).unwrap();
    assert_eq!(rows[0]["diagnostic_code"], "hosted_dispatch_unavailable");
    for row in rows {
        assert_eq!(row.as_object().unwrap().len(), 7);
        assert_eq!(row["tokens"], "1");
        assert_eq!(row["partial_tokens"], true);
    }
    let first_wire = first.to_string();
    for id in [f.space, f.key, f.offer, f.a.id, f.b.id] {
        assert!(!first_wire.contains(&id.to_string()));
    }
    // Work created after the first anchor does not enter the paged view.
    tokio::time::sleep(Duration::from_millis(5)).await;
    seed(1, f.offer, "failed", "0 seconds")
        .execute(&f.admin)
        .await
        .unwrap();
    let tail = usage
        .usage(&pool, Some(MODEL), Some(at), before)
        .await
        .unwrap();
    assert_eq!(tail["result"]["items"].as_array().unwrap().len(), 3);
    assert!(tail["result"]["next_before"].is_null());
    assert_eq!(
        usage.usage(&pool, Some("other"), None, None).await.unwrap()["result"]["items"],
        json!([])
    );
    assert_eq!(
        usage
            .usage(&pool, None, Some(at - 901_000), before)
            .await
            .unwrap()["error"],
        "invalid_filter"
    );

    let touched: i64 =
        sqlx::query_scalar("SELECT count(last_used_at) FROM relay_hosted.management_devices")
            .fetch_one(&f.admin)
            .await
            .unwrap();
    assert_eq!(touched, 0);
    assert_eq!(f.sends(), (0, 0));
    sqlx::query(
        "UPDATE relay_hosted.management_devices SET revoked_at=now(),revision=3 WHERE id=$1",
    )
    .bind(usage.id)
    .execute(&f.admin)
    .await
    .unwrap();
    assert!(usage.usage(&pool, None, Some(at), before).await.is_none());
    sqlx::query(
        "UPDATE relay_hosted.management_devices SET invited_at=invited_at-interval '25 hours',
        approve_by=approve_by-interval '25 hours',approved_at=approved_at-interval '25 hours',
        expires_at=expires_at-interval '25 hours' WHERE id=$1",
    )
    .bind(overview.id)
    .execute(&f.admin)
    .await
    .unwrap();
    assert!(overview.overview(&pool, ORIGIN).await.is_none());
}
