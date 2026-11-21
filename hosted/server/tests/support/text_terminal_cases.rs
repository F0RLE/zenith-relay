use super::{partial_text::Attempt, *};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn terminal_receipt_survives_wallet_lock_timeout_and_worker_restart() {
    let mut f = Fixture::new().await;
    let gate = Arc::new(ReplyGate::default());
    *f.b.provider.reply_gate.lock().unwrap() = Some(gate.clone());
    let response = tokio::spawn(f.submit("terminal-before-accounting", 1000, 100).send());
    tokio::time::timeout(Duration::from_secs(3), gate.entered.notified())
        .await
        .unwrap();
    let mut wallet = f.admin.begin().await.unwrap();
    sqlx::query("SELECT FROM relay_hosted.wallets WHERE workspace_id=$1 FOR NO KEY UPDATE")
        .bind(f.space)
        .execute(&mut *wallet)
        .await
        .unwrap();
    gate.release.notify_one();
    let pending = response.await.unwrap().unwrap();
    assert_eq!(pending.status(), 503);
    let id = request_id(&pending);
    assert_eq!(
        pending.json::<Value>().await.unwrap()["error"]["code"],
        "relay_outcome_pending"
    );
    let (offer, attempt, tokens, captured, charge): (Uuid, Uuid, i64, bool, Option<i64>) =
        sqlx::query_as(
            "SELECT r.offer_id,r.attempt_id,e.output_tokens,e.applied_at IS NULL,r.charged
             FROM relay_hosted.text_requests r JOIN relay_hosted.text_terminal_evidence e ON e.request_id=r.id
             WHERE r.id=$1",
        )
        .bind(id)
        .fetch_one(&mut *wallet)
        .await
        .unwrap();
    assert_eq!((tokens, captured, charge), (100, true, None));
    let held: i64 = sqlx::query_scalar(
        "SELECT reserved_micro_usd FROM relay_hosted.wallets WHERE workspace_id=$1",
    )
    .bind(f.space)
    .fetch_one(&mut *wallet)
    .await
    .unwrap();
    assert_eq!(held, 3500);
    wallet.rollback().await.unwrap();

    f.restart().await;
    let observed = Attempt {
        offer,
        request: id,
        id: attempt,
    };
    let mismatched = Attempt {
        id: Uuid::new_v4(),
        ..observed
    };
    let usage = [Some(1000), Some(0), Some(100)];
    assert!(!mismatched.capture(&f.worker, "completed", usage).await);
    assert!(observed.capture(&f.worker, "completed", usage).await);
    assert!(
        !observed
            .capture(&f.worker, "completed", [Some(99), Some(0), Some(1)])
            .await
    );
    assert_eq!(observed.reconcile(&f.worker, "completed").await, "invalid");
    let (first, second) = tokio::join!(
        inference::reconcile_due(&f.worker),
        inference::reconcile_due(&f.worker),
    );
    assert!((1..=2).contains(&(first.unwrap() + second.unwrap())));
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 0);
    verify_receipt(&f.admin, id, f.b.item, 3500, 1600, "1600").await;
    payment::assert_wallet(&f.admin, f.space, CREDIT - 3500, 3).await;

    let duplicate = f
        .submit("terminal-before-accounting", 1000, 100)
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), 409);
    assert_eq!(request_id(&duplicate), id);
    assert_eq!(f.sends(), (0, 1));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn accounting_after_the_deadline_uses_the_durable_observation_time() {
    let f = Fixture::new().await;
    let old = text_late_cases::expired(&f, "2024-02-29 23:59:30+00").await;
    // Seed the persisted cut point before the old deadline without sleeping through it.
    sqlx::query(
        "INSERT INTO relay_hosted.text_terminal_evidence(request_id,outcome,input_tokens,cached_tokens,output_tokens,observed_at)
         SELECT id,'incomplete',100,0,20,close_by-interval '1 second'
         FROM relay_hosted.text_requests WHERE id=$1",
    )
    .bind(old.request)
    .execute(&f.admin)
    .await
    .unwrap();
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 1);
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 0);
    verify_receipt(&f.admin, old.request, f.b.item, 400, 1600, "220").await;
    let closed: (String, String, bool) = sqlx::query_as(
        "SELECT r.basis,r.execution,e.applied_at>r.close_by FROM relay_hosted.text_requests r
         JOIN relay_hosted.text_terminal_evidence e ON e.request_id=r.id WHERE r.id=$1",
    )
    .bind(old.request)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert_eq!(closed, ("provider_final".into(), "incomplete".into(), true));
    payment::assert_wallet(&f.admin, f.space, CREDIT - 400, 3).await;
    assert_eq!(f.sends(), (0, 0));
}
