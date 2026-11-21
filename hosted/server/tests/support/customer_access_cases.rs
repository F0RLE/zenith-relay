use super::*;
use customer_reader::{Reader, add_workspace, setup_reader};

pub(super) async fn close_account<'e>(
    db: impl PgExecutor<'e>,
    space: Uuid,
    op: Uuid,
    proof: Option<i64>,
) -> String {
    sqlx::query_scalar(
        "SELECT relay_hosted.change_customer_access($1,$2,$3,$4,1,'close','verified_customer_request',
            $5,$6,$6,false,false,floor(extract(epoch FROM clock_timestamp())*1000)::bigint+120000)",
    )
    .bind(ORIGIN)
    .bind(refund_race::OWNER)
    .bind(space)
    .bind(op)
    .bind(proof.map(|_| &[0x42_u8][..]))
    .bind(proof)
    .fetch_one(db)
    .await
    .unwrap()
}

pub(super) async fn access_state(admin: &PgPool, space: Uuid) -> (String, i64, Option<String>) {
    sqlx::query_as(
        "SELECT relay_hosted.customer_access_state(u),u.authorization_epoch,u.closed_at::text
         FROM relay_hosted.users u JOIN relay_hosted.workspaces w ON w.owner_user_id=u.id WHERE w.id=$1",
    )
    .bind(space)
    .fetch_one(admin)
    .await
    .unwrap()
}

async fn quote(reader: &Reader, credit: i64, method: Uuid) -> (String, Option<Uuid>) {
    sqlx::query_as(
        "SELECT outcome,(payload->>'id')::uuid FROM relay_hosted.quote_checkout($1,$2,$3,$4,$5,$6)",
    )
    .bind(ORIGIN)
    .bind(reader.provider)
    .bind(&[17_u8; 32][..])
    .bind(reader.tag.as_slice())
    .bind(method)
    .bind(credit)
    .fetch_one(&reader.pool)
    .await
    .unwrap()
}

async fn confirm(reader: &Reader, quote: Uuid) -> (String, Option<Uuid>) {
    sqlx::query_as(
        "SELECT outcome,(payload->>'id')::uuid FROM relay_hosted.confirm_checkout($1,$2,$3,$4,$5,$6)",
    )
    .bind(ORIGIN)
    .bind(reader.provider)
    .bind(&[17_u8; 32][..])
    .bind(reader.tag.as_slice())
    .bind(quote)
    .bind(Uuid::new_v4())
    .fetch_one(&reader.pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn closure_fences_waiting_admission_and_preserves_accepted_obligations() {
    let f = Fixture::setup(true).await;
    setup_reader(&f).await;
    refund_race::owner_session(&f.admin).await;
    let a = Reader::new(&f, f.space, 44).await;
    let (other, _) = add_workspace(&f).await;
    let (other_key, other_bearer) = issue(&f.admin, f.scope, other).await;
    let method: Uuid = sqlx::query_scalar("SELECT id FROM relay_hosted.checkout_methods")
        .fetch_one(&f.admin)
        .await
        .unwrap();
    let (outcome, accepted_quote) = quote(&a, CREDIT, method).await;
    assert_eq!(outcome, "ready");
    let (outcome, accepted) = confirm(&a, accepted_quote.unwrap()).await;
    assert_eq!(outcome, "accepted");
    let accepted = accepted.unwrap();
    let (_, waiting_quote) = quote(&a, CREDIT, method).await;
    let waiting_quote = waiting_quote.unwrap();

    let gate = Arc::new(ReplyGate::default());
    *f.b.provider.reply_gate.lock().unwrap() = Some(gate.clone());
    let sent = tokio::spawn(f.submit("sent-before-close", 1000, 100).send());
    tokio::time::timeout(Duration::from_secs(3), gate.entered.notified())
        .await
        .unwrap();
    let queued = Held::reserve(&f, f.key, &f.bearer).await;
    let op = Uuid::new_v4();
    assert_eq!(
        close_account(&a.pool, f.space, op, None).await,
        "fresh_required"
    );
    let mut tx = a.pool.begin().await.unwrap();
    assert_eq!(
        close_account(&mut *tx, f.space, op, Some(1)).await,
        "applied"
    );
    let waiting = tokio::spawn(f.submit("admission-waiting-on-close", 1000, 100).send());
    wait_for_worker(
        &f.admin,
        "reserve_published_text",
        Some("transactionid"),
        0.0,
    )
    .await;
    tx.commit().await.unwrap();
    assert_eq!(waiting.await.unwrap().unwrap().status(), 401);
    assert_eq!(close_account(&a.pool, f.space, op, None).await, "applied");
    assert_eq!(close_account(&a.pool, other, op, None).await, "conflict");
    assert_eq!(access_state(&f.admin, f.space).await.0, "closed");
    assert_eq!(
        access_state(&f.admin, other).await,
        ("active".into(), 1, None)
    );
    payment::assert_wallet_with_holds(
        &f.admin,
        f.space,
        CREDIT - 2 * queued.amount,
        2 * queued.amount,
        3,
    )
    .await;
    assert_eq!(confirm(&a, waiting_quote).await, ("denied".into(), None));
    assert_eq!(quote(&a, CREDIT, method).await, ("denied".into(), None));
    assert_eq!(
        queued
            .claim_on(&f.worker, f.space, f.a.id, f.a.item, 1)
            .await,
        "denied"
    );
    assert_eq!(queued.finish(&f.worker, "not_sent").await, "released");

    // No new funding intent, but the accepted intent retained its merchant binding.
    let claimed: Uuid =
        sqlx::query_scalar("SELECT id FROM relay_hosted.claim_checkout_invoice($1)")
            .bind(f.merchant.binding)
            .fetch_one(&f.pay)
            .await
            .unwrap();
    assert_eq!(claimed, accepted);
    let expiry: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint+600")
            .fetch_one(&f.admin)
            .await
            .unwrap();
    let observed: bool = sqlx::query_scalar(
        "SELECT relay_hosted.observe_checkout_invoice($1,$2,'accepted-before-close','USD',100,$3,false,false)",
    ).bind(f.merchant.binding).bind(accepted).bind(expiry).fetch_one(&f.pay).await.unwrap();
    assert!(observed);
    let callback = f
        .merchant
        .sign("paid", "late-payment", "accepted-before-close", "USD", 100);
    let receipt = reconcile_paid(&f.pay, &f.merchant, &callback)
        .await
        .unwrap();
    assert_eq!(
        reconcile_paid(&f.pay, &f.merchant, &callback).await,
        Ok(receipt)
    );

    gate.release.notify_one();
    f.verify_purchase(sent.await.unwrap().unwrap(), f.b.item, 3500, 1600, "1600")
        .await;
    assert_eq!(f.sends(), (0, 1));
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 0);
    payment::assert_wallet(&f.admin, f.space, 2 * CREDIT - 3500, 6).await;
    let balances: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM relay_hosted.payment_intents WHERE workspace_id=$1),
                (SELECT count(*) FROM relay_hosted.payment_refunds WHERE workspace_id=$1)",
    )
    .bind(f.space)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert_eq!(balances, (2, 0));
    assert_eq!(
        zenith_relay_hosted::access::qualified_workspace(&f.worker, ORIGIN, &other_bearer)
            .await
            .unwrap(),
        Some(other)
    );
    assert_ne!(other_key, f.key);
    payment::assert_wallet(&f.admin, other, 0, 0).await;
}
