use super::*;

pub(super) const OTHER_MODEL: &str = "fixture-other-text";
pub(super) const PRIOR_MODEL: &str = "fixture-prior-purchase";

async fn period(f: &Fixture, spent: &str, outstanding: &str, permits: i64) {
    let actual: (String, String, i64) = sqlx::query_as(
        "SELECT p.accounted_micro_usd::text,p.outstanding_micro_usd::text,
            (SELECT count(*) FROM relay_hosted.text_purchase_attempts a
             JOIN relay_hosted.text_inventory i ON i.id=a.item
             WHERE i.source_id=p.source_id AND a.month_utc=p.month_utc)
         FROM relay_hosted.text_purchase_periods p WHERE p.source_id=$1
            AND p.month_utc=date_trunc('month',clock_timestamp() AT TIME ZONE 'UTC')::date",
    )
    .bind(f.a.id)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert_eq!(actual, (spent.into(), outstanding.into(), permits));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn purchase_cap_counts_uncapped_spend_and_a_lost_permit_across_models_and_keys() {
    let mut f = Fixture::new().await;
    let prior = Uuid::new_v4();
    stock(
        &f.admin,
        f.scope,
        &f.a,
        prior,
        PRIOR_MODEL,
        1000,
        ["55000", "0", "0"],
    )
    .await;
    publish(&f.admin, prior, 1, None).await;
    f.verify_purchase(
        f.submit_model(&f.bearer, PRIOR_MODEL, "uncapped-spend", 1000, 100)
            .send()
            .await
            .unwrap(),
        prior,
        3500,
        55_000_000,
        "55000000",
    )
    .await;
    period(&f, "55000000", "0", 1).await;

    sqlx::query(
        "WITH rate AS (INSERT INTO relay_hosted.text_purchase_rates VALUES($1,2,3000,0,10000)
            RETURNING item,revision)
         UPDATE relay_hosted.text_model_rules r SET purchase_revision=rate.revision FROM rate WHERE r.id=rate.item",
    )
    .bind(f.a.item)
    .execute(&f.admin)
    .await
    .unwrap();
    let other = Uuid::new_v4();
    stock(
        &f.admin,
        f.scope,
        &f.a,
        other,
        OTHER_MODEL,
        1000,
        ["3000", "0", "10000"],
    )
    .await;
    publish(&f.admin, other, 1, None).await;
    limit(&f.admin, f.a.id, Some(60_000_000)).await;
    limit(&f.admin, f.b.id, Some(0)).await;

    let (key, bearer) = issue(&f.admin, f.scope, f.space).await;
    let a = Held::reserve(&f, f.key, &f.bearer).await;
    let b = Held::reserve_model(&f, key, &bearer, OTHER_MODEL).await;
    assert_eq!(a.quotes(&f.worker).await, [(f.a.item, 2, 4_000_000)]);
    assert_eq!(b.quotes(&f.worker).await, [(other, 1, 4_000_000)]);
    let mut lock = f.admin.begin().await.unwrap();
    lock_source(&mut *lock, f.a.id).await;
    // Both transactions reach the same physical-source lock before either gets permission.
    let (left, right, ()) = tokio::join!(
        a.claim_on(&f.worker, f.space, f.a.id, f.a.item, 2),
        b.claim_on(&f.worker, f.space, f.a.id, other, 1),
        async {
            wait_for_workers(&f.admin, "claim_text_candidate", Some("advisory"), 0.0, 2).await;
            lock.commit().await.unwrap();
        },
    );
    let (won, item, revision, waiting, waiting_item, waiting_revision) =
        match (left.as_str(), right.as_str()) {
            ("claimed", "busy") => (&a, f.a.item, 2, &b, other, 1),
            ("busy", "claimed") => (&b, other, 1, &a, f.a.item, 2),
            results => panic!("expected exactly one permit, got {results:?}"),
        };
    period(&f, "55000000", "4000000", 2).await;

    // Discard the winning permit before doing I/O: recovery must assume it might have been used.
    f.restart().await;
    period(&f, "55000000", "4000000", 2).await;
    assert!(!won.claim(&f, f.a.id, item, revision).await);
    assert_eq!(won.finish(&f.worker, "not_sent").await, "pending");
    for cap in [Some(55_000_000), None, Some(60_000_000)] {
        limit(&f.admin, f.a.id, cap).await;
        period(&f, "55000000", "4000000", 2).await;
    }
    let attempt = won.attempt();
    // A terminal without usage removes the busy slot, not the outstanding bound.
    assert!(attempt.capture(&f.worker, "completed", [None; 3]).await);
    assert_eq!(attempt.reconcile(&f.worker, "deadline").await, "pending");
    assert_eq!(
        waiting
            .claim_on(&f.worker, f.space, f.a.id, waiting_item, waiting_revision)
            .await,
        "denied"
    );
    assert_eq!(waiting.finish(&f.worker, "not_sent").await, "released");
    for _ in 0..2 {
        assert_eq!(
            attempt
                .finish(&f.admin, "completed", [Some(1000), Some(0), Some(0)])
                .await,
            "settled"
        );
    }
    period(&f, "58000000", "0", 2).await;
    verify_receipt(&f.admin, won.id, item, 3000, 4_000_000, "3000000").await;

    let result = f
        .submit_model(&bearer, MODEL, "capped", 1000, 100)
        .send()
        .await
        .unwrap();
    let id = request_id(&result);
    assert_eq!(result.status(), 503);
    assert_eq!(
        result.json::<Value>().await.unwrap()["error"]["code"],
        "relay_not_dispatched"
    );
    assert_eq!(
        sqlx::query_as::<_, (String, i64)>(
            "SELECT basis,charged FROM relay_hosted.text_requests WHERE id=$1"
        )
        .bind(id)
        .fetch_one(&f.admin)
        .await
        .unwrap(),
        ("not_sent".into(), 0)
    );
    payment::assert_wallet(&f.admin, f.space, CREDIT - 3500 - 3000, 9).await;
    assert_eq!(f.sends(), (1, 0));

    limit(&f.admin, f.b.id, None).await;
    f.verify_purchase(
        f.submit_model(&bearer, MODEL, "alternate", 1000, 100)
            .send()
            .await
            .unwrap(),
        f.b.item,
        3500,
        1600,
        "1600",
    )
    .await;
    assert_eq!(f.sends(), (1, 1));
    period(&f, "58000000", "0", 2).await;
}
