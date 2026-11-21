use super::customer_reader::{Reader, add_workspace, setup_reader};
use super::*;

const SLOT: &str = "date_bin(interval '30 minutes',created_at,timestamptz '2000-01-01 00:00:00+00') + interval '30 minutes'";

impl Reader {
    pub(super) async fn read_status(&self, hours: i32) -> Option<Value> {
        sqlx::query_scalar("SELECT relay_hosted.customer_model_status($1,$2,$3,$4,$5)")
            .bind(ORIGIN)
            .bind(self.provider)
            .bind(&[0x11_u8; 32][..])
            .bind(&self.tag[..])
            .bind(hours)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn model_status_follows_publications_without_counting_other_workspaces_or_sends() {
    let f = Fixture::new().await;
    setup_reader(&f).await;
    let a = Reader::new(&f, f.space, 1).await;
    let (space, _) = add_workspace(&f).await;
    let b = Reader::new(&f, space, 2).await;
    assert!(a.read_status(720).await.is_none());
    db::check_runtime(&a.pool, ORIGIN).await.unwrap();
    let second = Uuid::new_v4();
    stock(
        &f.admin,
        f.scope,
        &f.a,
        second,
        purchase_limit_race::OTHER_MODEL,
        1000,
        ["2", ".50", "3"],
    )
    .await;
    publish(&f.admin, second, 1, None).await;
    let gate = Arc::new(ReplyGate::default());
    *f.a.provider.reply_gate.lock().unwrap() = Some(gate.clone());
    let wait = tokio::spawn(f.submit("model-status-logical-request", 5, 3).send());
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    let running = a.read_status(24).await.unwrap();
    assert!(running["summary"]["latency_ms"].is_null());
    assert_eq!(running["summary"]["latency_samples"], 0);
    tokio::time::sleep(Duration::from_millis(60)).await;
    gate.release.notify_one();
    let result = wait.await.unwrap().unwrap();
    assert_eq!(result.status(), 200);
    let rid = request_id(&result);
    let (aid, timing): (Uuid, i32) = sqlx::query_as(
        "SELECT r.attempt_id,t.elapsed_ms FROM relay_hosted.text_requests r
         JOIN relay_hosted.text_completion_timings t ON t.request_id=r.id WHERE r.id=$1",
    )
    .bind(rid)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert!((60..=90_000).contains(&timing));
    let attempt = partial_text::Attempt {
        offer: f.offer,
        request: rid,
        id: aid,
    };
    assert!(
        attempt
            .capture_timed(&f.worker, "completed", [Some(5), Some(0), Some(3)], Some(0))
            .await
    );
    let sends = f.sends();
    assert_eq!(sends.0 + sends.1, 1);
    for (hours, n) in [(24, 48), (168, 28)] {
        let snapshot = a.read_status(hours).await.unwrap();
        let foreign = b.read_status(hours).await.unwrap();
        let from = snapshot["intervals"][0]["from_ms"].as_i64().unwrap();
        let end = snapshot["as_of_ms"].as_i64().unwrap();
        assert_eq!(end - from, i64::from(hours) * 3_600_000);
        assert_eq!(snapshot["intervals"].as_array().unwrap().len(), n);
        let rows = snapshot["items"].as_array().unwrap();
        let catalog: Value =
            sqlx::query_scalar("SELECT relay_hosted.customer_text_models($1,$2,$3,$4)")
                .bind(ORIGIN)
                .bind(a.provider)
                .bind(&[0x11_u8; 32][..])
                .bind(&a.tag[..])
                .fetch_one(&a.pool)
                .await
                .unwrap();
        assert_eq!(
            rows.iter().map(|r| &r["model"]).collect::<Vec<_>>(),
            catalog
                .as_array()
                .unwrap()
                .iter()
                .map(|r| &r["model"])
                .collect::<Vec<_>>()
        );
        assert_eq!(rows.len(), 2);
        for row in rows {
            let history = row["history"].as_array().unwrap();
            assert_eq!(history.len(), n);
            assert!(history.iter().all(|i| i["state"] == "unknown"));
            assert!(history[0]["requests"].is_null());
            assert_eq!(history[0]["request_ms"], 0);
        }
        assert_eq!(snapshot["summary"]["requests"], 1);
        assert_eq!(snapshot["summary"]["cache_samples"], 1);
        assert_eq!(snapshot["summary"]["cache_bps"], 0);
        assert_eq!(snapshot["summary"]["latency_samples"], 1);
        assert_eq!(snapshot["summary"]["latency_ms"], timing);
        assert_eq!(rows[0]["history"][n - 1]["latency_ms"], timing);
        assert_eq!(foreign["summary"]["requests"], 0);
        assert_eq!(foreign["summary"]["cache_samples"], 0);
        assert!(foreign["summary"]["cache_bps"].is_null());
        assert!(foreign["summary"]["latency_ms"].is_null());
        assert_eq!(foreign["summary"]["latency_samples"], 0);
        assert_eq!(foreign["items"][0]["history"][n - 1]["requests"], 0);
        let safe = snapshot.to_string();
        for hidden in [
            f.a.id.to_string(),
            f.b.id.to_string(),
            f.key.to_string(),
            space.to_string(),
            f.offer.to_string(),
        ] {
            assert!(!safe.contains(&hidden));
        }
        assert!(!safe.contains("fixture-supplier") && !safe.contains("http"));
    }

    // A price revision during the same publication leaves request history intact.
    publish(&f.admin, f.a.item, 2, Some("2")).await;
    assert_eq!(a.read_status(24).await.unwrap()["summary"]["requests"], 1);
    assert_eq!(
        a.read_status(24).await.unwrap()["summary"]["latency_ms"],
        timing
    );
    sqlx::query("UPDATE relay_hosted.text_model_rules SET enabled=false WHERE id=$1")
        .bind(f.a.item)
        .execute(&f.admin)
        .await
        .unwrap();
    assert_eq!(
        a.read_status(24).await.unwrap()["items"][0]["model"],
        purchase_limit_race::OTHER_MODEL
    );
    let new = publish(&f.admin, f.a.item, 3, None).await;
    sqlx::query(
        "UPDATE relay_hosted.text_model_rules SET first_live_revision=3 WHERE last_offer=$1",
    )
    .bind(new)
    .execute(&f.admin)
    .await
    .unwrap();
    assert_eq!(a.read_status(24).await.unwrap()["summary"]["requests"], 0);
    assert!(a.read_status(24).await.unwrap()["summary"]["latency_ms"].is_null());
    sqlx::query("UPDATE relay_hosted.external_identities SET revoked_at=now() WHERE subject_digest=decode(repeat('22',32),'hex')")
        .execute(&f.admin).await.unwrap();
    assert!(a.read_status(24).await.is_none());
    assert_eq!(
        b.read_status(24).await.unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for sql in [
        "SELECT * FROM relay_hosted.model_status_observations",
        "DELETE FROM relay_hosted.model_status_observations",
        "SELECT * FROM relay_hosted.text_completion_timings",
        "SELECT relay_hosted.capture_text_terminal(gen_random_uuid(),gen_random_uuid(),gen_random_uuid(),'completed',1,0,1,100)",
        "SELECT relay_hosted.model_status_snapshot('http://localhost:1443',gen_random_uuid(),24,statement_timestamp())",
        "SELECT * FROM relay_hosted.customer_catalog_listings('http://localhost:1443',gen_random_uuid())",
    ] {
        let error = sqlx::query(sql).execute(&b.pool).await.unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("42501")
        );
    }
    assert_eq!(f.sends(), sends);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn model_status_cleanup_removes_only_expired_timings_without_rewriting_receipts() {
    let f = Fixture::new().await;
    setup_reader(&f).await;
    let a = Reader::new(&f, f.space, 1).await;
    let mut saved = Vec::new();
    for (kind, ms, measured) in [
        ("completed", 100, Some(100)),
        ("completed", 301, Some(301)),
        ("completed", 90_001, None),
        ("incomplete", 1, None),
    ] {
        let held = Held::reserve(&f, f.key, &f.bearer).await;
        assert!(held.claim(&f, f.a.id, f.a.item, 1).await);
        let attempt = held.attempt();
        assert!(
            attempt
                .capture_timed(&f.worker, kind, [Some(1000), Some(0), Some(20)], Some(ms))
                .await
        );
        assert_eq!(attempt.reconcile(&f.worker, "deadline").await, "settled");
        assert_eq!(
            sqlx::query_scalar::<_, i32>(
                "SELECT elapsed_ms FROM relay_hosted.text_completion_timings WHERE request_id=$1",
            )
            .bind(held.id)
            .fetch_optional(&f.admin)
            .await
            .unwrap(),
            measured
        );
        saved.push(held.id);
    }
    let before = a.read_status(24).await.unwrap();
    assert_eq!(before["summary"]["requests"], 4);
    assert_eq!(before["summary"]["latency_samples"], 2);
    assert_eq!(before["summary"]["latency_ms"], 201);
    let expired = saved[0];
    let receipt = text_late_cases::at_status(&f, expired).await;
    sqlx::query(
        "UPDATE relay_hosted.text_completion_timings
         SET observed_at=statement_timestamp()-interval '168 hours' WHERE request_id=$1",
    )
    .bind(expired)
    .execute(&f.admin)
    .await
    .unwrap();
    sqlx::query("SELECT relay_hosted.prune_model_status_completions()")
        .execute(&a.pool)
        .await
        .unwrap();
    let after = a.read_status(24).await.unwrap();
    assert_eq!(after["summary"]["requests"], 4);
    assert_eq!(after["summary"]["latency_samples"], 1);
    assert_eq!(after["summary"]["latency_ms"], 301);
    assert_eq!(text_late_cases::at_status(&f, expired).await, receipt);
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT applied_at IS NOT NULL FROM relay_hosted.text_terminal_evidence WHERE request_id=$1",
        )
        .bind(expired)
        .fetch_one(&f.admin)
        .await
        .unwrap()
    );
    assert_eq!(f.sends(), (0, 0));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn model_status_colors_only_covered_bins_and_never_treats_a_capped_scan_as_zero() {
    let f = Fixture::new().await;
    setup_reader(&f).await;
    let a = Reader::new(&f, f.space, 1).await;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO relay_hosted.model_status_observations
         SELECT p.id,{SLOT} + offset_minutes*interval '1 minute',state
         FROM relay_hosted.text_offers p CROSS JOIN (VALUES
           (-60,'operational'),(0,'operational'),(30,'operational'),(60,'unavailable'),(90,'unavailable'),
           (150,'degraded'),(180,'degraded'),(240,'operational')
         ) obs(offset_minutes,state) WHERE p.id=$1"
    )))
    .bind(f.offer)
    .execute(&f.admin)
    .await
    .unwrap();
    let snapshot: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT relay_hosted.model_status_snapshot($1,$2,24,{SLOT} + interval '255 minutes')
         FROM relay_hosted.text_offers WHERE id=$3",
    )))
    .bind(ORIGIN)
    .bind(f.space)
    .bind(f.offer)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    let h = snapshot["items"][0]["history"].as_array().unwrap();
    let state = |i: usize| h[i]["state"].as_str().unwrap();
    assert_eq!(h[39]["monitored_ms"], 900000);
    assert_eq!(state(39), "unknown");
    assert_eq!(state(40), "operational");
    assert_eq!(state(41), "degraded");
    assert_eq!(state(42), "unavailable");
    assert_eq!(h[43]["monitored_ms"], 900000);
    assert_eq!(state(43), "unknown");
    assert_eq!(state(45), "degraded");
    assert_eq!(h[47]["monitored_ms"], 0);
    assert_eq!(snapshot["summary"]["monitored_ms"], 6 * 1_800_000);
    assert_eq!(snapshot["summary"]["operational_ms"], 2 * 1_800_000);

    sqlx::query(
        "INSERT INTO relay_hosted.text_requests(workspace_id,key_id,offer_id,authority_epoch,input_tag,send_tag,
             input_bound,output_bound,held,started_at,claim_by,close_by,input_tokens,cached_tokens)
         SELECT k.workspace_id,k.id,$1,1,k.verifier,k.verifier,8,1,0,s.at,s.at+interval '30 seconds',
            s.at+interval '1 minute',8,4
         FROM relay_hosted.customer_keys k
         CROSS JOIN (SELECT statement_timestamp()-interval '1 millisecond' AS at) s
         CROSS JOIN generate_series(1,8193) WHERE k.id=$2",
    )
    .bind(f.offer)
    .bind(f.key)
    .execute(&f.admin)
    .await
    .unwrap();
    let capped = a.read_status(24).await.unwrap();
    assert_eq!(capped["items"][0]["model"], MODEL);
    assert!(capped["summary"]["requests"].is_null());
    assert_eq!(capped["summary"]["request_ms"], 0);
    assert!(capped["summary"]["latency_ms"].is_null());
    assert_eq!(capped["summary"]["latency_samples"], 0);
    assert!(capped["summary"]["cache_bps"].is_null());
    assert_eq!(f.sends(), (0, 0));
}
