#[path = "support/cluster.rs"]
mod cluster;
#[path = "support/payment_fixture.rs"]
mod fixture;

use cluster::Cluster;
use fixture::{Adapter, assert_wallet, invoice};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;
use zenith_relay_hosted::{
    config::Config,
    db,
    payments::{CreditError, reconcile_paid},
};

const WORKER: &str = "fixture_payments";
const TEN_USD: i64 = 10_000_000;
const POST: &str = "SELECT relay_hosted.post_verified_payment($1,$2,$3,'USD',1050)";

async fn workspace(db: &PgPool) -> Uuid {
    sqlx::query_scalar(
        "WITH person AS (INSERT INTO relay_hosted.users (id) VALUES (gen_random_uuid()) RETURNING id)
         INSERT INTO relay_hosted.workspaces (owner_user_id) SELECT id FROM person RETURNING id",
    )
    .fetch_one(db)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "creates a private temporary cluster; set RELAY_TEST_PG_BIN"]
async fn invoice_credit_is_isolated_balanced_and_recoverable() {
    let c = Cluster::new();
    let mut config = Config::parse(include_str!("../config.example.toml")).unwrap();
    config.database.lock_timeout_ms = 8000;
    config.database.statement_timeout_ms = 12000;
    let admin = PgPool::connect(&c.dsn("fixture_boot")).await.unwrap();
    db::migrate(&admin, &config.server.public_origin)
        .await
        .unwrap();
    c.create_logins(&admin, &[WORKER]).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(
        include_str!("../payment-grants.sql").replace(":\"payment_role\"", WORKER),
    ))
    .execute(&admin)
    .await
    .unwrap();
    let worker = db::connect(&config, &c.dsn(WORKER)).await.unwrap();
    assert_private_role(&worker).await;
    let (a, b) = (workspace(&admin).await, workspace(&admin).await);
    // Even shared signing material cannot move an invoice across environments.
    let key = Adapter::signing_key();
    let mut sandbox = Adapter::new(&admin, "sandbox", key.clone()).await;
    let other = Adapter::new(&admin, "live", key).await;
    let original = invoice(&admin, a, sandbox.binding, "one", TEN_USD).await;
    invoice(&admin, b, sandbox.binding, "two", TEN_USD).await;
    invoice(&admin, b, other.binding, "one", TEN_USD).await;
    assert_wallet(&admin, a, 0, 0).await;
    assert_wallet(&admin, b, 0, 0).await;

    check_rejections(&worker, &sandbox, &other).await;
    assert_eq!(event_count(&admin).await, 0);
    check_rollback(&worker, sandbox.binding).await;
    assert_eq!(event_count(&admin).await, 0);
    assert_wallet(&admin, a, 0, 0).await;
    reject_foreign_postings(&admin, a, b, original).await;
    let receipt = race_deliveries(&admin, &worker, &sandbox, a).await;
    assert_eq!(event_count(&admin).await, 2);
    assert_wallet(&admin, a, TEN_USD, 1).await;
    assert_eq!(
        reconcile_paid(&worker, &sandbox, &sandbox.paid("evt", "two")).await,
        Err(CreditError::EventConflict)
    );
    assert_wallet(&admin, b, 0, 0).await;
    let amounts: Vec<i64> = sqlx::query_scalar(
        "SELECT amount_micro_usd FROM relay_hosted.journal_postings WHERE transaction_id=$1 ORDER BY sequence",
    ).bind(receipt).fetch_all(&admin).await.unwrap();
    assert_eq!(amounts, [TEN_USD, -TEN_USD]);
    reject_rewrite(&admin, original, receipt).await;

    // Distinct binding: same external invoice and event IDs still fund B once.
    let b_receipt = reconcile_paid(&worker, &other, &other.paid("evt", "one"))
        .await
        .unwrap();
    assert_ne!(b_receipt, receipt);
    assert_wallet(&admin, b, TEN_USD, 1).await;
    invoice(&admin, a, sandbox.binding, "fresh", TEN_USD).await;
    let distinct = reconcile_paid(&worker, &sandbox, &sandbox.paid("evt-3", "fresh"))
        .await
        .unwrap();
    assert_ne!(distinct, receipt);
    assert_wallet(&admin, a, 2 * TEN_USD, 2).await;

    invoice(&admin, a, sandbox.binding, "too-large", i64::MAX).await;
    assert_eq!(
        reconcile_paid(&worker, &sandbox, &sandbox.paid("overflow", "too-large")).await,
        Err(CreditError::WalletCapacity)
    );
    assert_eq!(event_count(&admin).await, 4);
    worker.close().await;
    admin.close().await;
    c.stop();
    c.start();
    let worker = db::connect(&config, &c.dsn(WORKER)).await.unwrap();
    let admin = PgPool::connect(&c.dsn("fixture_boot")).await.unwrap();
    sandbox.rotate_key();
    assert_eq!(
        reconcile_paid(&worker, &sandbox, &sandbox.paid("evt", "one")).await,
        Ok(receipt)
    );
    assert_wallet(&admin, a, 2 * TEN_USD, 2).await;
    assert_wallet(&admin, b, TEN_USD, 1).await;
}

async fn event_count(db: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM relay_hosted.payment_events")
        .fetch_one(db)
        .await
        .unwrap()
}

fn assert_sqlstate(error: sqlx::Error, state: &str) {
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some(state)
    );
}

async fn assert_private_role(worker: &PgPool) {
    for command in [
        "SELECT * FROM relay_hosted.payment_intents",
        "UPDATE relay_hosted.wallets SET available_micro_usd=1",
        "DELETE FROM relay_hosted.journal_transactions",
        "DELETE FROM relay_hosted.journal_postings",
        "SELECT * FROM relay_hosted.admin_credentials",
    ] {
        assert_sqlstate(
            sqlx::query(sqlx::AssertSqlSafe(command))
                .execute(worker)
                .await
                .unwrap_err(),
            "42501",
        );
    }
}

async fn check_rejections(worker: &PgPool, sandbox: &Adapter, other: &Adapter) {
    use CreditError::*;
    let sign = |kind, currency, minor| sandbox.sign(kind, "evt", "one", currency, minor);
    for (delivery, expected) in [
        (other.paid("evt", "one"), Unverified),
        (sandbox.paid("evt", "one").tamper(), Unverified),
        (sign("pending", "USD", 1050), Unverified),
        (sign("lookup-unavailable", "USD", 1050), Unavailable),
        (sandbox.paid("evt", "unknown"), InvoiceMismatch),
        (sign("paid", "EUR", 1050), InvoiceMismatch),
        (sign("paid", "USD", 1000), InvoiceMismatch),
        (sign("paid", "USD", 0), InvalidEvidence),
        (sandbox.paid(&"x".repeat(129), "one"), InvalidEvidence),
    ] {
        assert_eq!(
            reconcile_paid(worker, sandbox, &delivery).await,
            Err(expected)
        );
    }
}

async fn check_rollback(worker: &PgPool, binding: Uuid) {
    let mut tx = worker.begin().await.unwrap();
    sqlx::query_scalar::<_, Uuid>(POST)
        .bind(binding)
        .bind("evt")
        .bind("one")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
}

async fn reject_foreign_postings(admin: &PgPool, a: Uuid, b: Uuid, original: Uuid) {
    let mut tx = admin.begin().await.unwrap();
    let incorrect: Uuid = sqlx::query_scalar(
        "INSERT INTO relay_hosted.journal_transactions (operation_kind,workspace_id,payment_id)
         VALUES ('payment_credit',$1,$2) RETURNING id",
    )
    .bind(a)
    .bind(original)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO relay_hosted.journal_postings (transaction_id,sequence,account_id,amount_micro_usd)
         SELECT $1,1,id,$3 FROM relay_hosted.ledger_accounts WHERE workspace_id=$2 AND account_kind='customer_available' UNION ALL
         SELECT $1,2,id,-$3::bigint FROM relay_hosted.ledger_accounts WHERE account_kind='payment_clearing'",
    ).bind(incorrect).bind(b).bind(TEN_USD).execute(&mut *tx).await.unwrap();
    assert_sqlstate(tx.commit().await.unwrap_err(), "23514");
}

async fn reject_rewrite(admin: &PgPool, invoice: Uuid, receipt: Uuid) {
    for (query, id) in [
        (
            "UPDATE relay_hosted.payment_intents SET credit_micro_usd=1 WHERE id=$1",
            invoice,
        ),
        (
            "UPDATE relay_hosted.payment_intents SET merchant_reference='another' WHERE id=$1",
            invoice,
        ),
        (
            "DELETE FROM relay_hosted.journal_postings WHERE transaction_id=$1",
            receipt,
        ),
    ] {
        let err = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(id)
            .execute(admin)
            .await
            .unwrap_err();
        assert_sqlstate(err, "23514");
    }
}

async fn race_deliveries(admin: &PgPool, worker: &PgPool, sandbox: &Adapter, a: Uuid) -> Uuid {
    // Prove actual overlap: two deliveries and one identical redelivery are
    // waiting in PostgreSQL before allowing any of them to commit.
    let mut gate = admin.begin().await.unwrap();
    sqlx::query("SELECT FROM relay_hosted.wallets WHERE workspace_id=$1 FOR NO KEY UPDATE")
        .bind(a)
        .execute(&mut *gate)
        .await
        .unwrap();
    let release = async {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let waiting = sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM pg_stat_activity WHERE usename='fixture_payments' AND wait_event_type='Lock'",
                ).fetch_one(admin).await.unwrap();
                if waiting == 3 { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("three independent credit transactions reached the lock");
        gate.commit().await.unwrap();
    };
    let (first, separate) = (sandbox.paid("evt", "one"), sandbox.paid("evt-2", "one"));
    let (r1, r2, r3, ()) = tokio::join!(
        reconcile_paid(worker, sandbox, &first),
        reconcile_paid(worker, sandbox, &first),
        reconcile_paid(worker, sandbox, &separate),
        release
    );
    let receipt = r1.unwrap();
    assert_eq!((r2, r3), (Ok(receipt), Ok(receipt)));
    receipt
}
