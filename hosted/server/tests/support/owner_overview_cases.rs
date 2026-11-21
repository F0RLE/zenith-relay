use super::{
    customer_reader::{Reader, add_workspace, setup_reader},
    purchase_limit_race::OTHER_MODEL,
    *,
};

async fn overview(f: &Fixture, hours: i32, model: Option<&str>, anchor: &str) -> Value {
    sqlx::query_scalar("SELECT relay_hosted.platform_usage_overview($1,$2,$3::text::timestamptz)")
        .bind(hours)
        .bind(model)
        .bind(anchor)
        .fetch_one(&f.admin)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn periods_and_exact_model_filter_cover_all_workspaces() {
    const ANCHOR: &str = "2024-03-01 00:30:00.123+00";
    const AS_OF: i64 = 1_709_253_000_123;
    const HOUR: i64 = 3_600_000;
    let f = Fixture::new().await;
    let empty = overview(&f, 0, None, ANCHOR).await;
    assert_eq!(empty["from_ms"], AS_OF - 24 * HOUR);
    assert_eq!(empty["totals"]["requests"], "0");
    assert_eq!(empty["totals"]["total_tokens"], "0");
    assert_eq!(empty["totals"]["spend_micro_usd"], "0");
    assert_eq!(empty["models"], json!([]));

    let (_, other_key) = add_workspace(&f).await;
    let item = Uuid::new_v4();
    stock(&f.admin, f.scope, &f.a, item, OTHER_MODEL, 100, ["1"; 3]).await;
    let other_offer = publish(&f.admin, item, 1, None).await;
    sqlx::query(
        "INSERT INTO relay_hosted.text_requests(workspace_id,key_id,offer_id,authority_epoch,input_tag,send_tag,
             input_bound,output_bound,held,started_at,claim_by,close_by,input_tokens,cached_tokens,output_tokens)
         SELECT k.workspace_id,k.id,v.offer,1,k.verifier,k.verifier,1,1,0,stamp,stamp+interval '1 hour',
             stamp+interval '2 hours',v.inputs,v.cached,v.outputs
         FROM (VALUES ($2::uuid,$3::uuid,'-720 hours -0.000001 seconds',0,0,0),($2,$3,'-720 hours',0,0,0),
             ($2,$3,'-168 hours -0.000001 seconds',0,0,0),($2,$3,'-168 hours',0,0,0),
             ($2,$3,'-24 hours -0.000001 seconds',0,0,0),($2,$3,'-24 hours',10,6,5),
             ($2,$3,'-23 hours -0.000001 seconds',0,0,0),($2,$3,'-23 hours',NULL,NULL,0),
             ($2,$3,'-0.000001 seconds',NULL,NULL,NULL),($2,$3,'0 seconds',10,0,1),
             ($2,$3,'0.000001 seconds',10,0,1),($4::uuid,$5::uuid,'-2 hours',20,10,9))
             v(key,offer,offset_time,inputs,cached,outputs)
         JOIN relay_hosted.customer_keys k ON k.id=v.key
         CROSS JOIN LATERAL (SELECT $1::text::timestamptz+v.offset_time::interval AS stamp) m",
    )
    .bind(ANCHOR)
    .bind(f.key)
    .bind(f.offer)
    .bind(other_key)
    .bind(other_offer)
    .execute(&f.admin)
    .await
    .unwrap();

    for (hours, count, buckets) in [(24, "5", 24), (168, "7", 7), (720, "9", 30), (0, "10", 30)] {
        let chart = overview(&f, hours, None, ANCHOR).await;
        assert_eq!(chart["hours"], hours);
        assert_eq!(chart["as_of_ms"], AS_OF);
        let width = i64::from(if hours == 0 { 720 } else { hours }) * HOUR;
        assert_eq!(chart["from_ms"], AS_OF - width - i64::from(hours == 0));
        assert_eq!(chart["buckets"].as_array().unwrap().len(), buckets);
        assert_eq!(chart["totals"]["requests"], count);
        assert_eq!(chart["totals"]["unsettled"], count);
        assert_eq!(chart["totals"]["total_tokens"], "44"); // Cached input is already included in input.
        assert_eq!(chart["totals"]["spend_micro_usd"], "0");
        assert_eq!(chart["models"], json!([OTHER_MODEL, MODEL]));
        assert_eq!(chart["more_models"], false);
        let wire = chart.to_string();
        for id in [
            f.space,
            f.key,
            other_key,
            f.offer,
            other_offer,
            f.a.id,
            f.b.id,
        ] {
            assert!(!wire.contains(&id.to_string()));
        }
        if hours == 24 {
            assert_eq!(chart["buckets"][0]["requests"], "2");
            assert_eq!(chart["buckets"][0]["total_tokens"], "15");
            assert_eq!(chart["buckets"][1]["requests"], "1");
            assert_eq!(chart["buckets"][1]["partial_tokens"], "1");
            assert!(chart["buckets"][1]["total_tokens"].is_null());
            assert_eq!(chart["buckets"][23]["requests"], "1");
            assert_eq!(chart["buckets"][23]["partial_tokens"], "0");
            assert!(chart["buckets"][23]["total_tokens"].is_null());
            assert_eq!(chart["buckets"][22]["total_tokens"], "29");
            assert_eq!(chart["buckets"][21], empty["totals"]);
        }
    }
    let selected = overview(&f, 24, Some(OTHER_MODEL), ANCHOR).await;
    assert_eq!(selected["totals"]["requests"], "1");
    assert_eq!(selected["totals"]["total_tokens"], "29");
    assert_eq!(selected["models"], json!([OTHER_MODEL, MODEL]));
    let wrong_case = overview(&f, 24, Some("Fixture-text"), ANCHOR).await;
    assert_eq!(wrong_case["totals"], empty["totals"]);
    setup_reader(&f).await;
    let customer = Reader::new(&f, f.space, 1).await;
    let denied: Option<Value> =
        sqlx::query_scalar("SELECT relay_hosted.owner_usage_overview($1,$2,24,NULL)")
            .bind(ORIGIN)
            .bind(customer.tag.as_slice())
            .fetch_one(&customer.pool)
            .await
            .unwrap();
    assert!(denied.is_none());
    assert!(
        sqlx::query("SELECT relay_hosted.platform_usage_overview(24,NULL,statement_timestamp())")
            .execute(&customer.pool)
            .await
            .is_err()
    );
    assert_eq!(f.sends(), (0, 0));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn posted_charge_changes_only_when_the_journal_credit_posts() {
    const STAMP: &str = "2024-02-29 00:30:00.123+00";
    const START_DAY: &str = "2024-02-29 01:00:00+00";
    let f = Fixture::new().await;
    let attempt = text_late_cases::expired(&f, STAMP).await;
    let before = overview(&f, 24, None, START_DAY).await;
    assert_eq!(before["totals"]["unsettled"], "1");
    assert_eq!(before["totals"]["spend_micro_usd"], "0"); // The hold and top-up are not charges.
    assert!(before["totals"]["total_tokens"].is_null());
    assert_eq!(attempt.reconcile(&f.worker, "deadline").await, "settled");
    let closed = overview(&f, 24, None, START_DAY).await;
    assert_eq!(closed["totals"]["spend_micro_usd"], "500");
    assert_eq!(closed["totals"]["unsettled"], "0");

    // Hold the wallet at capacity so that the late evidence can be recorded
    // while the correcting debit reversal cannot post yet.
    payment::invoice(
        &f.admin,
        f.space,
        f.merchant.binding,
        "fill-overview-wallet",
        i64::MAX - (CREDIT - 500),
    )
    .await;
    reconcile_paid(
        &f.pay,
        &f.merchant,
        &f.merchant.paid("fill-overview", "fill-overview-wallet"),
    )
    .await
    .unwrap();
    assert!(
        attempt
            .capture(&f.worker, "incomplete", [Some(0), None, Some(80)])
            .await
    );
    assert_eq!(attempt.reconcile(&f.worker, "deadline").await, "settled");
    let measured = overview(&f, 24, None, START_DAY).await;
    assert_eq!(measured["totals"]["total_tokens"], "80");
    assert_eq!(measured["totals"]["requests"], "1");
    assert_eq!(measured["totals"]["partial_tokens"], "0");
    assert_eq!(measured["totals"]["spend_micro_usd"], "500"); // Purchase expense and an unposted correction do not count.

    f.verify_purchase(
        f.submit("make-room-overview", 1000, 100)
            .send()
            .await
            .unwrap(),
        f.b.item,
        3500,
        1600,
        "1600",
    )
    .await;
    let sends = f.sends();
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 1);
    let corrected = overview(&f, 24, None, START_DAY).await;
    assert_eq!(corrected["totals"]["spend_micro_usd"], "400");
    assert_eq!(corrected["buckets"][23], corrected["totals"]);
    assert_eq!(corrected["totals"]["total_tokens"], "80");
    assert_eq!(corrected["totals"]["requests"], "1");
    assert_eq!(f.sends(), sends);
}
