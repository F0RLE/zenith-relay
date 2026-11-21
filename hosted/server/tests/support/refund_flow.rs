#[path = "refund_fixture.rs"]
mod fixture;

use super::{age_current_passkey, checkpoint};
use crate::support::Process;
use fixture::{Merchant, balances};
use sqlx::PgPool;
use std::{fs, path::Path, sync::atomic::Ordering::SeqCst};
use uuid::Uuid;
use zenith_relay_hosted::payments::refunds::{
    RefundError, pending_lookups, reconcile, submit_next,
};

const TOTAL: i64 = 10_000_000;
const REFUND: i64 = 2_000_000;

pub(super) async fn run(
    admin: &PgPool,
    connect: impl AsyncFn() -> PgPool,
    binding: Uuid,
    browser: &mut Process,
    exchange: &Path,
    space: Uuid,
) {
    let payment: Uuid = fs::read_to_string(exchange.join("checkout-confirmed"))
        .unwrap()
        .parse()
        .unwrap();
    let merchant = Merchant::new(binding, payment.to_string());
    let worker = connect().await;
    let id = read_case(browser, exchange, "refund-requested").await;
    assert_eq!(balances(admin, space).await, (TOTAL, 0, 0));
    assert_eq!(submit_next(&worker, &merchant).await, Ok(None));
    assert!(
        pending_lookups(&worker, binding, None)
            .await
            .unwrap()
            .is_empty()
    );
    age_current_passkey(admin).await;
    fs::write(exchange.join("refund-passkey-aged"), []).unwrap();

    assert_eq!(read_case(browser, exchange, "refund-approved").await, id);
    assert_eq!(
        balances(admin, space).await,
        (TOTAL - REFUND, REFUND, REFUND)
    );
    let another_binding = Uuid::new_v4();
    assert!(
        pending_lookups(&worker, another_binding, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        reconcile(
            &worker,
            &Merchant::new(another_binding, merchant.invoice.clone()),
            id
        )
        .await,
        Err(RefundError::Mismatch)
    );
    let retired: bool = sqlx::query_scalar(
        "SELECT m.disabled_at IS NOT NULL FROM relay_hosted.checkout_methods m
         JOIN relay_hosted.payment_intents i ON i.checkout_method_id=m.id WHERE i.id=$1",
    )
    .bind(payment)
    .fetch_one(admin)
    .await
    .unwrap();
    assert!(retired);
    // The merchant made the external effect, but the worker lost its reply.
    merchant.lose_reply.store(true, SeqCst);
    let (one, two) = tokio::join!(
        submit_next(&worker, &merchant),
        submit_next(&worker, &merchant)
    );
    assert!(matches!((one, two), (Ok(Some(a)), Ok(None)) | (Ok(None), Ok(Some(a))) if a==id));
    assert_eq!(merchant.submissions(), 1);
    assert_eq!(
        balances(admin, space).await,
        (TOTAL - REFUND, REFUND, REFUND)
    );
    fs::write(exchange.join("refund-unknown"), []).unwrap();
    checkpoint(browser, &exchange.join("refund-read-unknown")).await;
    worker.close().await;
    let worker = connect().await;
    assert_eq!(pending_lookups(&worker, binding, None).await.unwrap(), [id]);
    assert!(
        pending_lookups(&worker, binding, Some(id))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(submit_next(&worker, &merchant).await, Ok(None));
    // A temporary no-record reply or a mismatched amount is not proof of failure.
    reconcile(
        &worker,
        &Merchant::new(binding, merchant.invoice.clone()),
        id,
    )
    .await
    .unwrap();
    merchant.wrong_total.store(true, SeqCst);
    assert_eq!(
        reconcile(&worker, &merchant, id).await,
        Err(RefundError::Mismatch)
    );
    assert_eq!(
        balances(admin, space).await,
        (TOTAL - REFUND, REFUND, REFUND)
    );
    merchant.wrong_total.store(false, SeqCst);
    let (one, two) = tokio::join!(
        reconcile(&worker, &merchant, id),
        reconcile(&worker, &merchant, id)
    );
    assert_eq!((one, two), (Ok(()), Ok(())));
    assert_eq!(balances(admin, space).await, (TOTAL - REFUND, 0, 0));
    assert_eq!(merchant.submissions(), 1);

    let observation = |value: bool, minor: i64| {
        sqlx::query_scalar::<_, bool>(
            "SELECT relay_hosted.observe_payment_refund($1,$2,$3,'USD',$4,$5,$6)",
        )
        .bind(binding)
        .bind(id)
        .bind(&merchant.invoice)
        .bind(minor)
        .bind(format!("refund-{id}"))
        .bind(value)
    };
    assert!(observation(true, 200).fetch_one(&worker).await.unwrap());
    assert!(!observation(true, 201).fetch_one(&worker).await.unwrap());
    assert!(!observation(false, 200).fetch_one(&worker).await.unwrap());
    let before = merchant.lookups.load(SeqCst);
    reconcile(&worker, &merchant, id).await.unwrap();
    assert_eq!(merchant.lookups.load(SeqCst), before);
    fs::write(exchange.join("refund-succeeded"), []).unwrap();

    let failed = read_case(browser, exchange, "refund-second-approved").await;
    assert_ne!(failed, id);
    assert_eq!(
        balances(admin, space).await,
        (TOTAL - REFUND - 1_000_000, 1_000_000, 1_000_000)
    );
    merchant.lose_reply.store(false, SeqCst);
    merchant.refuse.store(true, SeqCst);
    assert_eq!(submit_next(&worker, &merchant).await, Ok(Some(failed)));
    reconcile(&worker, &merchant, failed).await.unwrap();
    assert_eq!(merchant.submissions(), 2);
    assert_eq!(balances(admin, space).await, (TOTAL - REFUND, 0, 0));
    let closed: Vec<(Uuid, String, Vec<String>, Vec<i64>)> = sqlx::query_as(
        "SELECT j.refund_id,j.operation_kind,array_agg(a.account_kind ORDER BY p.sequence),
            array_agg(p.amount_micro_usd ORDER BY p.sequence)
         FROM relay_hosted.journal_transactions j JOIN relay_hosted.journal_postings p ON p.transaction_id=j.id
         JOIN relay_hosted.ledger_accounts a ON a.id=p.account_id WHERE j.workspace_id=$1 AND j.payment_id IS NULL
         GROUP BY j.id ORDER BY j.created_at",
    ).bind(space).fetch_all(admin).await.unwrap();
    let posting = |case, kind: &str, to: &str, n: i64| {
        (
            case,
            kind.to_owned(),
            vec!["customer_refund_reserved".to_owned(), to.to_owned()],
            vec![n, -n],
        )
    };
    assert_eq!(
        closed,
        [
            posting(id, "refund_hold", "customer_available", REFUND),
            posting(id, "refund_paid", "payment_clearing", -REFUND),
            posting(failed, "refund_hold", "customer_available", 1_000_000),
            posting(failed, "refund_release", "customer_available", -1_000_000),
        ]
    );
    fs::write(exchange.join("refund-failed"), []).unwrap();
    let declined = read_case(browser, exchange, "refund-declined").await;
    let audit: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT r.state,a.action,a.actor_id=i.owner_user_id AND a.actor_id=k.user_id
         FROM relay_hosted.payment_refunds r JOIN relay_hosted.payment_refund_reviews a ON a.refund_id=r.id
         JOIN relay_hosted.admin_credentials k ON k.passkey_id=a.confirmed_by_passkey_id
         CROSS JOIN relay_hosted.installations i
         WHERE r.payment_id=$1 ORDER BY r.requested_at",
    ).bind(payment).fetch_all(admin).await.unwrap();
    assert_eq!(
        audit,
        [
            ("succeeded".into(), "approve".into(), true),
            ("failed".into(), "approve".into(), true),
            ("declined".into(), "decline".into(), true),
        ]
    );
    assert_eq!(submit_next(&worker, &merchant).await, Ok(None));
    assert_eq!(balances(admin, space).await, (TOTAL - REFUND, 0, 0));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM relay_hosted.journal_transactions WHERE refund_id=$1"
        )
        .bind(declined)
        .fetch_one(admin)
        .await
        .unwrap(),
        0
    );
}

async fn read_case(browser: &mut Process, exchange: &Path, file: &str) -> Uuid {
    let path = exchange.join(file);
    checkpoint(browser, &path).await;
    fs::read_to_string(path).unwrap().parse().unwrap()
}
