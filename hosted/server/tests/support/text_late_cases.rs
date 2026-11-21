use super::{
    partial_text::{Attempt, PartialText},
    *,
};

pub(super) async fn expired(f: &Fixture, claimed_at: &str) -> Attempt {
    PartialText {
        key: f.key,
        offer: f.offer,
        item: f.b.item,
        rate_revision: 1,
        claimed_at: claimed_at.into(),
        input_bound: 1000,
        output_bound: 100,
        observed_output: 100,
    }
    .seed(&f.admin)
    .await
}

pub(super) async fn at_status(f: &Fixture, request: Uuid) -> Value {
    let r = f
        .http
        .get(format!(
            "{}/api/hosted/v1/requests/{request}/status",
            f.server.url
        ))
        .bearer_auth(&f.bearer)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    r.json::<Value>().await.unwrap()["data"].clone()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn complete_counts_can_settle_a_partial_observation_before_close_without_a_credit() {
    let f = Fixture::new().await;
    let held = Held::reserve(&f, f.key, &f.bearer).await;
    assert!(held.claim(&f, f.b.id, f.b.item, 1).await);
    let attempt = held.attempt();
    assert!(
        attempt
            .capture(&f.worker, "incomplete", [Some(1000), None, Some(100)])
            .await
    );
    assert_eq!(attempt.reconcile(&f.worker, "deadline").await, "pending");
    assert_eq!(
        attempt
            .finish(&f.admin, "incomplete", [Some(1), Some(2), Some(100)])
            .await,
        "invalid"
    );
    let partial = at_status(&f, held.id).await;
    assert_eq!(partial["output_tokens"], "100");
    assert!(partial["charge_micro_usd"].is_null());

    assert_eq!(
        attempt
            .finish(&f.admin, "incomplete", [Some(1000), Some(1000), Some(100)])
            .await,
        "settled"
    );
    verify_receipt(&f.admin, held.id, f.b.item, 900, 1600, "850").await;
    let receipt = at_status(&f, held.id).await;
    assert_eq!(receipt["basis"], "provider_final");
    assert!(receipt["credit"].is_null());
    payment::assert_wallet(&f.admin, f.space, CREDIT - 900, 3).await;
}

async fn periods(f: &Fixture) -> Vec<(String, bool, String, String)> {
    sqlx::query_as(
        "SELECT month_utc::text,month_utc=date_trunc('month',clock_timestamp() AT TIME ZONE 'UTC')::date,
            accounted_micro_usd::text,outstanding_micro_usd::text
         FROM relay_hosted.text_purchase_periods WHERE source_id=$1 ORDER BY month_utc",
    )
    .bind(f.b.id)
    .fetch_all(&f.admin)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn expired_prior_month_usage_returns_one_credit_after_restart_and_keeps_the_current_cap() {
    let mut f = Fixture::new().await;
    let attempt = expired(&f, "2024-02-29 23:59:30+00").await;
    limit(&f.admin, f.a.id, Some(0)).await;
    limit(&f.admin, f.b.id, Some(1600)).await;
    f.restart().await;

    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 1);
    assert_eq!(
        periods(&f).await,
        [("2024-02-01".into(), false, "0".into(), "1600".into())]
    );
    let mut original = at_status(&f, attempt.request).await;
    assert_eq!(original["basis"], "provider_partial");
    assert_eq!(original["charge_micro_usd"], "500");
    assert!(original["credit"].is_null());

    // Old exposure remains counted in its original month, not under today's cap.
    f.verify_purchase(
        f.submit("new-month", 1000, 100).send().await.unwrap(),
        f.b.item,
        3500,
        1600,
        "1600",
    )
    .await;
    publish(&f.admin, f.a.item, 2, Some("2")).await;
    let evidence = [Some(100), Some(0), Some(20)];
    let (first, repeat) = tokio::join!(
        attempt.capture(&f.worker, "incomplete", evidence),
        attempt.capture(&f.worker, "incomplete", evidence)
    );
    assert!(first && repeat);
    let (first, repeat) = tokio::join!(
        attempt.reconcile(&f.worker, "deadline"),
        attempt.reconcile(&f.worker, "deadline")
    );
    assert_eq!((first.as_str(), repeat.as_str()), ("settled", "settled"));
    assert!(
        !attempt
            .capture(&f.worker, "incomplete", [Some(0), None, Some(0)])
            .await
    );

    let mut updated = at_status(&f, attempt.request).await;
    let credit = updated.as_object_mut().unwrap().remove("credit").unwrap();
    assert_eq!(
        credit["amount_micro_usd"], "100",
        "the original tariff applies"
    );
    original.as_object_mut().unwrap().remove("credit");
    assert_eq!(updated, original);
    let journal: Uuid = credit["wallet_receipt_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let postings: Vec<(String, Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT a.account_kind,a.workspace_id,p.amount_micro_usd FROM relay_hosted.journal_postings p
         JOIN relay_hosted.ledger_accounts a ON a.id=p.account_id WHERE p.transaction_id=$1 ORDER BY p.sequence",
    )
    .bind(journal)
    .fetch_all(&f.admin)
    .await
    .unwrap();
    assert_eq!(
        postings,
        [
            ("customer_available".into(), Some(f.space), 100),
            ("inference_revenue".into(), None, -100)
        ]
    );
    let months = periods(&f).await;
    assert_eq!(months.len(), 2);
    assert_eq!(
        months[0],
        ("2024-02-01".into(), false, "220".into(), "0".into())
    );
    assert_eq!(
        (months[1].1, months[1].2.as_str(), months[1].3.as_str()),
        (true, "1600", "0")
    );
    payment::assert_wallet(&f.admin, f.space, CREDIT - 3500 - 400, 6).await;
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 0);
    let unavailable = f.submit("cap-remains", 1000, 100).send().await.unwrap();
    assert_eq!(unavailable.status(), 503);
    assert_eq!(
        unavailable.json::<Value>().await.unwrap()["error"]["code"],
        "relay_not_dispatched"
    );
    assert_eq!(f.sends(), (0, 1));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn first_complete_evidence_after_cutoff_cannot_raise_the_customer_debit() {
    let f = Fixture::new().await;
    let attempt = expired(&f, "2024-02-29 23:59:30+00").await;
    // Late evidence can exceed i64 as an expense. It is not a second debit.
    assert!(
        attempt
            .capture(
                &f.worker,
                "incomplete",
                [Some(i64::MAX), Some(0), Some(i64::MAX)]
            )
            .await
    );
    assert_eq!(attempt.reconcile(&f.worker, "deadline").await, "settled");
    assert!(
        !attempt
            .capture(&f.worker, "incomplete", [Some(0), None, Some(0)])
            .await
    );
    let receipt = at_status(&f, attempt.request).await;
    assert_eq!(receipt["charge_micro_usd"], "500");
    assert_eq!(receipt["basis"], "provider_partial");
    assert!(receipt["credit"].is_null());
    let cost: String = sqlx::query_scalar(
        "SELECT valued_micro_usd::text FROM relay_hosted.text_purchase_results WHERE attempt_id=$1",
    )
    .bind(attempt.id)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert_eq!(cost, (i128::from(i64::MAX) * 7).to_string());
    payment::assert_wallet(&f.admin, f.space, CREDIT - 500, 3).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn full_wallet_defers_the_credit_without_losing_late_evidence() {
    let f = Fixture::new().await;
    let attempt = expired(&f, "2024-02-29 23:59:30+00").await;
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 1);
    payment::invoice(
        &f.admin,
        f.space,
        f.merchant.binding,
        "fill-wallet",
        i64::MAX - (CREDIT - 500),
    )
    .await;
    reconcile_paid(
        &f.pay,
        &f.merchant,
        &f.merchant.paid("wallet-filled", "fill-wallet"),
    )
    .await
    .unwrap();

    // Known zero input needs no cache breakdown, even though cached prices differ.
    assert!(
        attempt
            .capture(&f.worker, "incomplete", [Some(0), None, Some(80)])
            .await
    );
    assert_eq!(attempt.reconcile(&f.worker, "deadline").await, "settled");
    assert!(at_status(&f, attempt.request).await["credit"].is_null());
    assert_eq!(
        periods(&f).await,
        [("2024-02-01".into(), false, "480".into(), "0".into())]
    );
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 0);
    payment::assert_wallet(&f.admin, f.space, i64::MAX, 4).await;

    f.verify_purchase(
        f.submit("make-room", 1000, 100).send().await.unwrap(),
        f.b.item,
        3500,
        1600,
        "1600",
    )
    .await;
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 1);
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 0);
    assert_eq!(
        at_status(&f, attempt.request).await["credit"]["amount_micro_usd"],
        "100"
    );
    payment::assert_wallet(&f.admin, f.space, i64::MAX - 3500 + 100, 7).await;
}
