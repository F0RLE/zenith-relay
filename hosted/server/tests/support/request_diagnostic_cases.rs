use super::{
    customer_reader::{Reader, add_workspace},
    owner_usage_cases::{diagnostic_filter, owner, request_detail},
    *,
};

const STATUS: &str = "upstream_status";
const DELIVERY: &str = "hosted_delivery_closed";

#[derive(Clone, Copy)]
struct RequestIds {
    request: Uuid,
    attempt: Uuid,
    offer: Uuid,
}

impl RequestIds {
    async fn seed(f: &Fixture, claimed: bool, age_hours: i32) -> Self {
        let (request, attempt) = sqlx::query_as(
            "WITH stamp AS (SELECT now()-make_interval(hours=>$3) AS started)
             INSERT INTO relay_hosted.text_requests(workspace_id,key_id,offer_id,authority_epoch,input_tag,send_tag,
                input_bound,output_bound,held,started_at,claim_by,close_by,claimed_at,execution,accounting)
             SELECT k.workspace_id,k.id,$2,1,k.verifier,k.verifier,1,1,0,s.started,s.started+interval '30 seconds',
                s.started+interval '65 seconds',CASE WHEN $4 THEN s.started END,
                CASE WHEN $4 THEN 'uncertain' ELSE 'reserved' END,CASE WHEN $4 THEN 'pending' ELSE 'held' END
             FROM relay_hosted.customer_keys k CROSS JOIN stamp s WHERE k.id=$1 RETURNING id,attempt_id",
        )
        .bind(f.key)
        .bind(f.offer)
        .bind(age_hours)
        .bind(claimed)
        .fetch_one(&f.admin)
        .await
        .unwrap();
        Self {
            request,
            attempt,
            offer: f.offer,
        }
    }

    async fn record(
        self,
        pool: &PgPool,
        code: &str,
        status: Option<i16>,
        occurred_ms: Option<i64>,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar("SELECT relay_hosted.record_request_diagnostic($1,$2,$3,$4,$5,$6)")
            .bind(self.offer)
            .bind(self.request)
            .bind(self.attempt)
            .bind(code)
            .bind(status)
            .bind(occurred_ms)
            .fetch_one(pool)
            .await
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn scoped_diagnostics_are_deduplicated_and_projected_by_audience() {
    let f = Fixture::new().await;
    let runtime = owner(&f).await;
    let a = Reader::new(&f, f.space, 1).await;
    let (other_space, _) = add_workspace(&f).await;
    let b = Reader::new(&f, other_space, 2).await;
    let ids = RequestIds::seed(&f, true, 1).await;
    for incorrect in [
        RequestIds {
            offer: Uuid::new_v4(),
            ..ids
        },
        RequestIds {
            request: Uuid::new_v4(),
            ..ids
        },
        RequestIds {
            attempt: Uuid::new_v4(),
            ..ids
        },
        RequestIds::seed(&f, false, 0).await,
        RequestIds::seed(&f, true, 721).await,
    ] {
        assert!(
            !incorrect
                .record(&f.worker, STATUS, Some(429), None)
                .await
                .unwrap()
        );
    }
    for (code, status) in [
        ("fixture_extra_code", None),
        (STATUS, Some(600)),
        (DELIVERY, Some(502)),
    ] {
        assert!(!ids.record(&f.worker, code, status, None).await.unwrap());
    }
    let denied = ids
        .record(&runtime, STATUS, Some(429), None)
        .await
        .unwrap_err();
    assert_eq!(
        denied.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    assert!(
        sqlx::query("SELECT * FROM relay_hosted.request_diagnostics")
            .execute(&f.worker)
            .await
            .is_err()
    );
    assert_eq!(
        request_detail(&runtime, ids.request).await["diagnostic_coverage"],
        "unavailable"
    );
    assert_eq!(
        a.current_receipt(ids.request).await["diagnostic"],
        json!({"diagnostic_version":1,"coverage":"unavailable","code":"relay_outcome_pending",
            "summary":"unavailable","next_action":"check_status"})
    );

    assert!(
        ids.record(&f.worker, STATUS, Some(429), Some(i64::MAX))
            .await
            .unwrap()
    );
    let first = request_detail(&runtime, ids.request).await["diagnostics"][0].clone();
    assert_eq!(first["cause_status"], "unknown");
    assert!(first["origin"].is_null() && first["occurred_at_ms"].is_null());
    assert!(
        ids.record(&f.worker, STATUS, Some(500), None)
            .await
            .unwrap()
    );
    assert!(ids.record(&f.worker, DELIVERY, None, None).await.unwrap());
    let detail = request_detail(&runtime, ids.request).await;
    assert_eq!(detail["diagnostic_coverage"], "partial");
    assert_eq!(detail["diagnostics"].as_array().unwrap().len(), 2);
    assert_eq!(detail["diagnostics"][0], first);
    assert_eq!(first["evidence"], json!({"http_status":429}));
    assert_eq!(first["attempt_id"], ids.attempt.to_string());
    assert_eq!(first["code"], STATUS);
    let latest = diagnostic_filter(&runtime, None, None, None).await;
    assert_eq!(latest["items"].as_array().unwrap().len(), 1);
    assert_eq!(latest["items"][0]["diagnostic"]["code"], DELIVERY);
    let status = diagnostic_filter(
        &runtime,
        Some("upstream_read"),
        Some("unknown"),
        Some(STATUS),
    )
    .await;
    assert_eq!(status["items"].as_array().unwrap().len(), 1);
    assert_eq!(status["items"][0]["diagnostic"], first);
    assert_eq!(
        diagnostic_filter(&runtime, Some("upstream_read"), Some("cancellation"), None).await["items"],
        json!([]),
        "all diagnostic filters must match the same observation"
    );
    assert_eq!(
        diagnostic_filter(&runtime, Some("preview"), None, None).await["error"],
        "invalid_filter"
    );

    let own = a.current_receipt(ids.request).await;
    assert_eq!(own["items"][0]["request_id"], ids.request.to_string());
    assert_eq!(
        own["diagnostic"],
        json!({"diagnostic_version":1,"coverage":"partial","code":"relay_outcome_pending",
            "summary":"connection_closed","next_action":"check_status"})
    );
    assert_eq!(
        b.current_receipt(ids.request).await,
        json!({"items":[],"diagnostic":null})
    );
    assert_eq!(f.sends(), (0, 0));
    payment::assert_wallet(&f.admin, f.space, CREDIT, 1).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn diagnostic_capacity_collision_drops_new_data_and_expiry_hides_old_data() {
    let f = Fixture::new().await;
    let runtime = owner(&f).await;
    let a = Reader::new(&f, f.space, 1).await;
    let ids = RequestIds::seed(&f, true, 1).await;
    assert!(
        ids.record(&f.worker, STATUS, Some(503), None)
            .await
            .unwrap()
    );
    let expiry_hours: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM d.expires_at-r.started_at)::double precision/3600
         FROM relay_hosted.request_diagnostics d JOIN relay_hosted.text_requests r ON r.id=d.request_id",
    )
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert_eq!(expiry_hours, 720.0);
    // Jump directly to a collision rather than populating the whole history.
    sqlx::query(
        "SELECT setval('relay_hosted.request_diagnostic_sequence',sequence+262144,false)
         FROM relay_hosted.request_diagnostics",
    )
    .execute(&f.admin)
    .await
    .unwrap();
    assert!(!ids.record(&f.worker, DELIVERY, None, None).await.unwrap());
    assert_eq!(
        request_detail(&runtime, ids.request).await["diagnostics"][0]["code"],
        STATUS
    );
    sqlx::query(
        "UPDATE relay_hosted.request_diagnostics SET expires_at=now()-interval '1 millisecond'",
    )
    .execute(&f.admin)
    .await
    .unwrap();
    assert_eq!(
        request_detail(&runtime, ids.request).await["diagnostic_coverage"],
        "unavailable"
    );
    assert_eq!(
        diagnostic_filter(&runtime, None, None, Some(STATUS)).await["items"],
        json!([])
    );
    assert_eq!(
        a.current_receipt(ids.request).await["diagnostic"]["coverage"],
        "unavailable"
    );
    db::prune_request_diagnostics(&runtime).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM relay_hosted.request_diagnostics")
            .fetch_one(&f.admin)
            .await
            .unwrap(),
        0
    );
    assert!(ids.record(&f.worker, DELIVERY, None, None).await.unwrap());
    assert_eq!(
        request_detail(&runtime, ids.request).await["diagnostics"][0]["code"],
        DELIVERY
    );
    assert_eq!(f.sends(), (0, 0));
    payment::assert_wallet(&f.admin, f.space, CREDIT, 1).await;
}
