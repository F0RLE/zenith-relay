use super::customer_reader::{Reader, add_workspace, setup_reader};
use super::*;

#[derive(Default)]
struct Selection<'a> {
    model: Option<&'a str>,
    bucket: Option<i32>,
    key: Option<Uuid>,
    execution: Option<&'a str>,
}

impl Reader {
    async fn start(&self, hours: i32) -> Value {
        sqlx::query_scalar("SELECT relay_hosted.start_usage_report($1,$2,$3,$4,$5)")
            .bind(ORIGIN)
            .bind(self.provider)
            .bind(&[0x11_u8; 32][..])
            .bind(&self.tag[..])
            .bind(hours)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn read(
        &self,
        id: Uuid,
        model: Option<&str>,
        bucket: Option<i32>,
        before: Option<Uuid>,
        overview: bool,
    ) -> Option<Value> {
        self.select(
            id,
            Selection {
                model,
                bucket,
                ..Selection::default()
            },
            before,
            if overview { "overview" } else { "page" },
        )
        .await
    }

    async fn select(
        &self,
        id: Uuid,
        selection: Selection<'_>,
        before: Option<Uuid>,
        view: &str,
    ) -> Option<Value> {
        sqlx::query_scalar(
            "SELECT relay_hosted.read_usage_report($1,$2,$3,$4,$5,
                ROW($6::text,$7::integer,$8::uuid,$9::text)::relay_hosted.usage_filter,$10,NULL,$11)",
        )
        .bind(ORIGIN)
        .bind(self.provider)
        .bind(&[0x11_u8; 32][..])
        .bind(&self.tag[..])
        .bind(id)
        .bind(selection.model)
        .bind(selection.bucket)
        .bind(selection.key)
        .bind(selection.execution)
        .bind(before)
        .bind(view)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

fn id(v: &Value) -> Uuid {
    v.as_str().unwrap().parse().unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn usage_bounds_are_utc_half_open_and_tenant_scoped() {
    let f = Fixture::new().await;
    setup_reader(&f).await;
    let a = Reader::new(&f, f.space, 1).await;
    let (space, b_key) = add_workspace(&f).await;
    let b = Reader::new(&f, space, 2).await;
    const ANCHOR: &str = "2024-03-01 00:30:00.123+00";
    sqlx::query(
        "INSERT INTO relay_hosted.text_requests(workspace_id,key_id,offer_id,authority_epoch,input_tag,send_tag,
             input_bound,output_bound,held,started_at,claim_by,close_by,input_tokens,cached_tokens,output_tokens)
         SELECT k.workspace_id,k.id,$1,1,k.verifier,k.verifier,1,1,0,stamp,stamp+interval '1 hour',
             stamp+interval '2 hours',t.tokens,t.tokens,t.tokens
         FROM (VALUES ($3::uuid,'-720 hours -0.000001 seconds',0),($3,'-720 hours',0),
             ($3,'-168 hours -0.000001 seconds',0),($3,'-168 hours',0),
             ($3,'-24 hours -0.000001 seconds',NULL),($3,'-24 hours',0),
             ($3,'-23 hours -0.000001 seconds',0),($3,'-23 hours',0),($3,'-0.000001 seconds',NULL),
             ($3,'0 seconds',0),($3,'0.000001 seconds',0),($4::uuid,'-2 hours',0)) t(key,offset_time,tokens)
         JOIN relay_hosted.customer_keys k ON k.id=t.key
         CROSS JOIN LATERAL (SELECT $2::text::timestamptz+t.offset_time::interval AS stamp) m",
    ).bind(f.offer).bind(ANCHOR).bind(f.key).bind(b_key).execute(&f.admin).await.unwrap();
    for (hours, slot, count, n) in [(24, 1i16, 4, 24), (168, 2, 6, 7), (720, 3, 8, 30)] {
        let snapshot: Uuid = sqlx::query_scalar(
            "SELECT relay_hosted.save_usage_report($1,$2,$3,$4::text::timestamptz)",
        )
        .bind(f.space)
        .bind(slot)
        .bind(hours)
        .bind(ANCHOR)
        .fetch_one(&f.admin)
        .await
        .unwrap();
        let chart = a.read(snapshot, None, None, None, true).await.unwrap();
        assert_eq!(chart["totals"]["requests"], count);
        assert_eq!(chart["buckets"].as_array().unwrap().len(), n);
        assert_eq!(chart["as_of_ms"], 1_709_253_000_123_i64);
        assert_eq!(
            chart["as_of_ms"].as_i64().unwrap() - chart["from_ms"].as_i64().unwrap(),
            i64::from(hours) * 3_600_000
        );
        assert_eq!(
            b.read(snapshot, None, None, None, true).await.unwrap()["error"],
            "report_not_found"
        );
        assert_eq!(chart["keys"].as_array().unwrap().len(), 1);
        assert_eq!(chart["keys"][0]["id"], json!(f.key));
        if hours == 24 {
            let rows = a
                .read(snapshot, Some(MODEL), Some(0), None, false)
                .await
                .unwrap();
            assert_eq!(rows["totals"], chart["buckets"][0]);
            assert_eq!(rows["items"].as_array().unwrap().len(), 2);
            assert_eq!(rows["items"][1]["started_at_ms"], rows["from_ms"]);
            assert_eq!(rows["totals"]["total_tokens"], "0");
            assert_eq!(chart["buckets"][1]["requests"], 1);
            assert_eq!(chart["buckets"][23]["requests"], 1);
            assert_eq!(chart["buckets"][23]["total_tokens"], Value::Null);
            assert_eq!(chart["buckets"][22]["total_tokens"], "0");
            let filtered = a
                .read(snapshot, Some("other-model"), None, None, true)
                .await
                .unwrap();
            assert_eq!(filtered["totals"]["requests"], 0);
            assert_eq!(
                a.read(snapshot, None, Some(24), None, false).await.unwrap()["error"],
                "invalid_filter"
            );
            let foreign_key = a
                .select(
                    snapshot,
                    Selection {
                        key: Some(b_key),
                        ..Selection::default()
                    },
                    None,
                    "export",
                )
                .await
                .unwrap();
            assert_eq!(foreign_key["totals"]["requests"], 0);
            let denied = b
                .select(snapshot, Selection::default(), None, "export")
                .await;
            assert_eq!(denied.unwrap()["error"], "report_not_found");
            let all = a
                .select(snapshot, Selection::default(), None, "export")
                .await
                .unwrap();
            assert_eq!(all["from_utc"], "2024-02-29T00:30:00.123Z");
            assert_eq!(all["as_of_utc"], "2024-03-01T00:30:00.123Z");
            assert_eq!(all["items"][3]["started_at_utc"], all["from_utc"]);
            sqlx::query(
                "UPDATE relay_hosted.usage_reports SET expires_at=clock_timestamp() WHERE id=$1",
            )
            .bind(snapshot)
            .execute(&f.admin)
            .await
            .unwrap();
            let expired = a
                .select(snapshot, Selection::default(), None, "export")
                .await;
            assert_eq!(expired.unwrap()["error"], "report_not_found");
            db::prune_usage_reports(&a.pool).await.unwrap();
            assert!(!sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT FROM relay_hosted.usage_report_receipts WHERE report_id=$1)")
                .bind(snapshot).fetch_one(&f.admin).await.unwrap());
        }
    }
    sqlx::query("DELETE FROM relay_hosted.customer_sessions WHERE verifier=$1")
        .bind(&a.tag[..])
        .execute(&f.admin)
        .await
        .unwrap();
    assert!(
        a.select(Uuid::new_v4(), Selection::default(), None, "export")
            .await
            .is_none()
    );
    assert_eq!(b.start(48).await["error"], "invalid_filter");
    let b_report = id(&b.start(24).await["report_id"]);
    for _ in 1..16 {
        assert!(b.start(24).await["report_id"].is_string());
    }
    assert_eq!(b.start(24).await["error"], "report_busy");
    assert_eq!(
        b.read(b_report, None, None, None, true).await.unwrap()["totals"]["requests"],
        0
    );
    for sql in [
        "SELECT * FROM relay_hosted.usage_report_receipts",
        "SELECT * FROM relay_hosted.usage_report_rows(NULL,NULL)",
        "SELECT relay_hosted.usage_report_metrics(NULL,NULL)",
        "SELECT relay_hosted.usage_report_keys(NULL)",
    ] {
        assert!(sqlx::query(sql).execute(&a.pool).await.is_err());
    }
    assert_eq!(f.sends(), (0, 0));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn a_credit_between_pages_does_not_change_the_saved_report() {
    let f = Fixture::new().await;
    setup_reader(&f).await;
    let a = Reader::new(&f, f.space, 1).await;
    assert_eq!(
        f.submit("one-logical-request", 5, 3)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        f.submit("one-logical-request", 5, 3)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let sends = f.sends();
    assert_eq!(sends.0 + sends.1, 1);
    let stamp: String = sqlx::query_scalar("SELECT (clock_timestamp()-interval '2 minutes')::text")
        .fetch_one(&f.admin)
        .await
        .unwrap();
    let attempt = text_late_cases::expired(&f, &stamp).await;
    assert_eq!(attempt.reconcile(&f.worker, "deadline").await, "settled");
    let (alternate, _) = issue(&f.admin, f.scope, f.space).await;
    let fill = |rows: i32| {
        sqlx::query(
            "INSERT INTO relay_hosted.text_requests(workspace_id,key_id,offer_id,authority_epoch,input_tag,send_tag,
                input_bound,output_bound,held,started_at,claim_by,close_by)
             SELECT k.workspace_id,k.id,$1,1,k.verifier,k.verifier,1,1,0,m.ts,m.ts+interval '1 hour',m.ts+interval '2 hours'
             FROM generate_series(1,$3) s(i) JOIN relay_hosted.customer_keys k
                 ON k.id=CASE WHEN s.i % 2=1 THEN $2 ELSE $4 END
             CROSS JOIN (SELECT clock_timestamp()-interval '10 seconds' ts) m",
        )
        .bind(f.offer)
        .bind(f.key)
        .bind(rows)
        .bind(alternate)
    };
    fill(34).execute(&f.admin).await.unwrap();
    let old = id(&a.start(24).await["report_id"]);
    let summary = a.read(old, Some(MODEL), None, None, true).await.unwrap();
    assert_eq!(summary["totals"]["requests"], 36);
    assert_eq!(summary["totals"]["token_samples"], 1);
    let first = a
        .read(old, Some(MODEL), Some(23), None, false)
        .await
        .unwrap();
    assert_eq!(first["totals"], summary["buckets"][23]);
    let before = id(&first["items"][31]["request_id"]);
    assert!(
        first["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["request_id"] != json!(attempt.request))
    );

    let filtered = |execution| Selection {
        key: Some(alternate),
        model: Some(MODEL),
        bucket: Some(23),
        execution: Some(execution),
    };
    let subset = a
        .select(old, filtered("reserved"), None, "export")
        .await
        .unwrap();
    assert_eq!(subset["totals"]["requests"], 17);
    assert_eq!(subset["items"].as_array().unwrap().len(), 17);
    assert_eq!(
        a.select(old, filtered("completed"), None, "page")
            .await
            .unwrap()["totals"]["requests"],
        0
    );
    assert!(
        subset["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| { r["key_id"] == json!(alternate) && r["execution"] == "reserved" })
    );
    sqlx::query("UPDATE relay_hosted.customer_keys SET revoked_at=clock_timestamp() WHERE id=$1")
        .bind(alternate)
        .execute(&f.admin)
        .await
        .unwrap();
    assert_eq!(
        a.read(old, Some(MODEL), None, None, true).await.unwrap()["keys"],
        summary["keys"]
    );

    // The late evidence can exist before its wallet credit posts.
    let staged = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM (SELECT relay_hosted.record_late_text_usage(r,100,0,20,400)
         FROM relay_hosted.text_requests r WHERE id=$1 OFFSET 0) a",
    )
    .bind(attempt.request)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert_eq!(staged, 1);
    let mid = id(&a.start(24).await["report_id"]);
    let unposted = a.read(mid, None, None, None, true).await.unwrap();
    assert_eq!(
        unposted["totals"]["spend_micro_usd"],
        summary["totals"]["spend_micro_usd"]
    );
    assert_eq!(unposted["totals"]["token_samples"], 2);
    sqlx::query(
        "SELECT relay_hosted.post_text_credit(r) FROM relay_hosted.text_requests r WHERE id=$1",
    )
    .bind(attempt.request)
    .execute(&f.admin)
    .await
    .unwrap();

    let frozen = a
        .read(old, Some(MODEL), Some(23), Some(before), false)
        .await
        .unwrap();
    assert_eq!(frozen["totals"], first["totals"]);
    assert_eq!(frozen["items"].as_array().unwrap().len(), 4);
    assert_eq!(frozen["items"][3]["request_id"], json!(attempt.request));
    assert_eq!(frozen["items"][3]["charge_micro_usd"], "500");
    assert!(frozen["items"][3]["credit"].is_null());
    assert_eq!(frozen["items"][3]["usage_quality"], "partial");
    let new = id(&a.start(24).await["report_id"]);
    let refreshed = a
        .read(new, None, Some(23), Some(before), false)
        .await
        .unwrap();
    assert_eq!(refreshed["items"][3]["credit"]["amount_micro_usd"], "100");
    assert_eq!(refreshed["items"][3]["total_tokens"], "120");
    assert_eq!(refreshed["totals"]["requests"], 36);
    let spend = |v: &Value| {
        v["totals"]["spend_micro_usd"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap()
    };
    assert_eq!(spend(&frozen) - spend(&refreshed), 100);
    let whole_old = a
        .select(old, Selection::default(), None, "export")
        .await
        .unwrap();
    assert_eq!(whole_old["totals"], summary["totals"]);
    assert_eq!(whole_old["items"].as_array().unwrap().len(), 36);
    assert!(whole_old["items"][35]["credit"].is_null());
    assert_eq!(f.sends(), sends);

    fill(8192).execute(&f.admin).await.unwrap();
    assert_eq!(a.start(24).await["error"], "report_too_large");
    assert_eq!(
        a.read(old, Some(MODEL), None, None, true).await.unwrap(),
        summary
    );
}
