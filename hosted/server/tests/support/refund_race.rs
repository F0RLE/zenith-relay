#[path = "refund_fixture.rs"]
pub(super) mod fixture;

use super::*;
use customer_reader::{Reader, setup_reader};
use fixture::{Merchant, balances};
use zenith_relay_hosted::payments::refunds::{pending_lookups, reconcile, submit_next};

pub(super) async fn checkout_invoice(
    db: &PgPool,
    space: Uuid,
    binding: Uuid,
    invoice: &str,
    credit: i64,
) {
    sqlx::query(
        "WITH method AS (
            INSERT INTO relay_hosted.checkout_methods(merchant_binding_id,method_code,label,position,
                min_credit_minor,max_credit_minor,fee_minor)
            VALUES($2,'card','Fixture card',0,$4::bigint/10000,$4::bigint/10000,0) RETURNING id
         ) INSERT INTO relay_hosted.payment_intents(workspace_id,operation_id,quote_id,merchant_binding_id,
            merchant_reference,quoted_currency,quoted_amount_minor,credit_micro_usd,checkout_method_id,checkout_claimed_at,invoice_expires_at)
         SELECT $1,gen_random_uuid(),gen_random_uuid(),$2,$3,'USD',$4::bigint/10000,$4,id,now(),now()+interval '1 hour' FROM method",
    ).bind(space).bind(binding).bind(invoice).bind(credit).execute(db).await.unwrap();
}

pub(super) const OWNER: &[u8] = &[53; 32];

pub(super) async fn owner_session(db: &PgPool) {
    sqlx::raw_sql(
        "UPDATE relay_hosted.installations SET setup_state='claimed',
            owner_user_id=(SELECT user_id FROM relay_hosted.admin_assignments WHERE role='owner');
         INSERT INTO relay_hosted.admin_credentials(credential_id,user_id,passkey)
            SELECT '\\x42'::bytea,owner_user_id,jsonb_build_object('cred',jsonb_build_object('counter',0))
            FROM relay_hosted.installations;",
    )
    .execute(db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO relay_hosted.admin_sessions SELECT $1,user_id,credential_id,
        installation_epoch,user_epoch,now(),now(),now()+interval '8 hours' FROM relay_hosted.active_owner_credentials")
        .bind(OWNER).execute(db).await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn refund_and_real_inference_admission_cannot_spend_the_same_wallet_funds() {
    let mut f = Fixture::setup(true).await;
    setup_reader(&f).await;
    owner_session(&f.admin).await;
    let reader = Reader::new(&f, f.space, 24).await;
    let payment: Uuid =
        sqlx::query_scalar("SELECT id FROM relay_hosted.payment_intents WHERE workspace_id=$1")
            .bind(f.space)
            .fetch_one(&f.admin)
            .await
            .unwrap();
    let merchant = Merchant::new(f.merchant.binding, "fund-route-test".into());

    let first = request(&reader, payment, CREDIT).await;
    assert_eq!(balances(&f.admin, f.space).await, (CREDIT, 0, 0));
    let mut tx = reader.pool.begin().await.unwrap();
    let rolled_back = Uuid::new_v4();
    assert_eq!(
        review(&mut *tx, first, rolled_back, "approve", None).await,
        "fresh_required"
    );
    assert_eq!(
        review(&mut *tx, first, rolled_back, "approve", Some(1)).await,
        "applied"
    );
    tx.rollback().await.unwrap();
    assert_eq!(balances(&f.admin, f.space).await, (CREDIT, 0, 0));
    assert_eq!(submit_next(&f.pay, &merchant).await, Ok(None));
    reject_wrong_postings(&f.admin, f.space, first).await;

    // Keep the refund write uncommitted until a real HTTP admission waits on
    // the same wallet. It must see the new available balance after lock handoff.
    let mut tx = reader.pool.begin().await.unwrap();
    let op = Uuid::new_v4();
    assert_eq!(
        review(&mut *tx, first, op, "approve", Some(1)).await,
        "applied"
    );
    let parallel = tokio::spawn(f.submit("refund-got-wallet-first", 1000, 100).send());
    wait_for_worker(
        &f.admin,
        "reserve_published_text",
        Some("transactionid"),
        0.0,
    )
    .await;
    tx.commit().await.unwrap();
    assert_eq!(parallel.await.unwrap().unwrap().status(), 402);
    assert_eq!(f.sends(), (0, 0));
    assert_eq!(balances(&f.admin, f.space).await, (0, CREDIT, CREDIT));
    assert_eq!(
        review(&reader.pool, first, op, "approve", None).await,
        "applied"
    );
    merchant.refuse.store(true, SeqCst);
    assert_eq!(submit_next(&f.pay, &merchant).await, Ok(Some(first)));
    assert_eq!(balances(&f.admin, f.space).await, (CREDIT, 0, 0));

    let second = request(&reader, payment, CREDIT).await;
    assert_eq!(
        review(&reader.pool, second, Uuid::new_v4(), "decline", Some(1)).await,
        "passkey_changed"
    );
    let held = Held::reserve(&f, f.key, &f.bearer).await;
    assert_eq!(held.amount, 3500);
    let approve = Uuid::new_v4();
    assert_eq!(
        review(&reader.pool, second, approve, "approve", Some(2)).await,
        "balance"
    );
    assert_eq!(
        balances(&f.admin, f.space).await,
        (CREDIT - held.amount, held.amount, 0)
    );
    let was_audited: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT FROM relay_hosted.payment_refund_reviews WHERE operation_id=$1)",
    )
    .bind(approve)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert!(!was_audited);
    assert_eq!(
        review(&reader.pool, second, Uuid::new_v4(), "decline", Some(2)).await,
        "applied"
    );

    let part = 980_000;
    let third = request(&reader, payment, part).await;
    assert_eq!(
        review(&reader.pool, third, Uuid::new_v4(), "approve", Some(3)).await,
        "applied"
    );
    let mixed = (CREDIT - held.amount - part, held.amount + part, part);
    assert_eq!(balances(&f.admin, f.space).await, mixed);
    let published: (Uuid, i64, i64, i64) =
        sqlx::query_as("SELECT * FROM relay_hosted.customer_wallet($1,$2,$3,$4)")
            .bind(ORIGIN)
            .bind(reader.provider)
            .bind([17_u8; 32].as_slice())
            .bind(reader.tag.as_slice())
            .fetch_one(&reader.pool)
            .await
            .unwrap();
    assert_eq!(published, (f.space, mixed.0, mixed.1, mixed.2));
    merchant.refuse.store(false, SeqCst);
    merchant.lose_reply.store(true, SeqCst);
    assert_eq!(submit_next(&f.pay, &merchant).await, Ok(Some(third)));
    sqlx::query(
        "UPDATE relay_hosted.service_control SET mode='maintenance',qualification_scope_ref=NULL,
        revision=revision+1,admission_epoch=admission_epoch+1",
    )
    .execute(&f.admin)
    .await
    .unwrap();
    reader.pool.close().await;
    f.pay.close().await;
    f.restart().await;
    f.pay = db::connect(&fixture_config(), &f._cluster.dsn("route_payment"))
        .await
        .unwrap();
    assert_eq!(balances(&f.admin, f.space).await, mixed);
    assert_eq!(
        pending_lookups(&f.pay, merchant.binding, None)
            .await
            .unwrap(),
        [third]
    );
    assert_eq!(submit_next(&f.pay, &merchant).await, Ok(None));
    reconcile(&f.pay, &merchant, third).await.unwrap();
    assert_eq!(balances(&f.admin, f.space).await, (mixed.0, held.amount, 0));
    assert_eq!(merchant.submissions(), 2);
    let settled: String =
        sqlx::query_scalar("SELECT relay_hosted.reconcile_text_attempt($1,$2,$3,'not_sent')")
            .bind(held.offer)
            .bind(held.id)
            .bind(held.attempt)
            .fetch_one(&f.worker)
            .await
            .unwrap();
    assert_eq!(settled, "released");
    payment::assert_wallet(&f.admin, f.space, CREDIT - part, 7).await;
    let overview: Value = sqlx::query_scalar(
        "SELECT relay_hosted.platform_usage_overview(24,NULL,date_trunc('milliseconds',clock_timestamp()))",
    )
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert_eq!(overview["totals"]["requests"], "1");
    assert_eq!(overview["totals"]["spend_micro_usd"], "0"); // Neither the top-up nor the posted refund is inference spend.
}

pub(super) async fn request(reader: &Reader, payment: Uuid, amount: i64) -> Uuid {
    let (outcome, result): (String, Value) =
        sqlx::query_as("SELECT * FROM relay_hosted.request_payment_refund($1,$2,$3,$4,$5,$6,$7)")
            .bind(ORIGIN)
            .bind(reader.provider)
            .bind([17_u8; 32].as_slice())
            .bind(reader.tag.as_slice())
            .bind(payment)
            .bind(Uuid::new_v4())
            .bind(amount)
            .fetch_one(&reader.pool)
            .await
            .unwrap();
    assert_eq!(outcome, "accepted");
    assert_eq!(result["state"], "requested");
    result["id"].as_str().unwrap().parse().unwrap()
}

pub(super) async fn review<'e>(
    db: impl PgExecutor<'e>,
    refund: Uuid,
    operation: Uuid,
    action: &str,
    verified_revision: Option<i64>,
) -> String {
    // Synthetic verifier input tests the SQL boundary; the browser journey proves the actual assertion.
    sqlx::query_scalar(
        "SELECT relay_hosted.review_payment_refund($1,$2,$3,$4,'requested',$5,$6,$7,0,false,false,
            floor(extract(epoch FROM clock_timestamp())*1000)::bigint+30000)",
    )
    .bind(ORIGIN)
    .bind(OWNER)
    .bind(refund)
    .bind(operation)
    .bind(action)
    .bind(verified_revision.map(|_| b"B".as_slice()))
    .bind(verified_revision)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn reject_wrong_postings(db: &PgPool, workspace: Uuid, refund: Uuid) {
    let mut tx = db.begin().await.unwrap();
    let receipt: Uuid = sqlx::query_scalar(
        "WITH allowed AS (UPDATE relay_hosted.payment_refunds SET state='queued' WHERE id=$2 RETURNING id)
         INSERT INTO relay_hosted.journal_transactions(workspace_id,refund_id,operation_kind)
            SELECT $1,id,'refund_hold' FROM allowed RETURNING id",
    ).bind(workspace).bind(refund).fetch_one(&mut *tx).await.unwrap();
    sqlx::query(
        "INSERT INTO relay_hosted.journal_postings(transaction_id,sequence,account_id,amount_micro_usd)
         SELECT $2::uuid,1,id,$3::bigint FROM relay_hosted.ledger_accounts WHERE workspace_id=$1 AND account_kind='customer_reserved'
         UNION ALL
         SELECT $2,2,id,-$3 FROM relay_hosted.ledger_accounts WHERE workspace_id=$1 AND account_kind='customer_available'",
    ).bind(workspace).bind(receipt).bind(CREDIT).execute(&mut *tx).await.unwrap();
    let error = tx.commit().await.unwrap_err();
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
}
