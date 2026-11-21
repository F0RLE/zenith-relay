use super::{age_current_passkey, checkpoint};
use crate::{payment, support::Process};
use sqlx::PgPool;
use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;
use uuid::Uuid;
use zenith_relay_hosted::payments::{
    CreditError,
    checkout::{
        AcceptedCheckout, CheckoutAdapter, CheckoutError, Invoice, Observation, create_next,
        reconcile_invoice, resume_next,
    },
    reconcile_paid,
    registry::{SandboxMethod, register_sandbox_profile},
};

const CREDIT: i64 = 10_000_000;
const DESTINATION: &str = "https://checkout.merchant.invalid";

struct Merchant<'a> {
    signer: &'a payment::Adapter,
    invoices: Mutex<HashMap<Uuid, i64>>,
    creates: AtomicUsize,
    reopens: AtomicUsize,
    lifetime_s: AtomicUsize,
    wrong_total: AtomicBool,
    block_reopen: AtomicBool,
    reopening: Notify,
    reopened: Notify,
}

fn now_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

impl Merchant<'_> {
    fn invoice(&self, intent: &AcceptedCheckout, expiry: i64) -> Invoice {
        Invoice {
            reference: intent.id.to_string(),
            currency: "USD".into(),
            payable_minor: intent.payable_minor
                + i64::from(self.wrong_total.load(Ordering::Relaxed)),
            expires_at_s: expiry,
            closed: expiry <= now_s(),
            can_resume: expiry > now_s(),
        }
    }
}

impl CheckoutAdapter for Merchant<'_> {
    fn binding(&self) -> Uuid {
        self.signer.binding
    }

    async fn create(&self, intent: &AcceptedCheckout) -> Observation {
        self.creates.fetch_add(1, Ordering::Relaxed);
        if intent.method_code == "bank" {
            return Observation::NotCreated;
        }
        assert_eq!(
            (
                &*intent.method_code,
                &*intent.currency,
                intent.payable_minor
            ),
            ("card", "USD", 1050)
        );
        let expiry = now_s() + self.lifetime_s.load(Ordering::Relaxed) as i64;
        assert!(
            self.invoices
                .lock()
                .unwrap()
                .insert(intent.id, expiry)
                .is_none()
        );
        Observation::Uncertain // merchant committed; response was lost
    }

    async fn lookup(&self, intent: &AcceptedCheckout) -> Observation {
        match self.invoices.lock().unwrap().get(&intent.id) {
            Some(expiry) => Observation::Invoice(self.invoice(intent, *expiry)),
            None => Observation::Uncertain,
        }
    }

    fn checkout_origin(&self) -> Option<&str> {
        Some(DESTINATION)
    }

    async fn reopen(
        &self,
        intent: &AcceptedCheckout,
        reference: &str,
    ) -> Option<(Invoice, String)> {
        self.reopens.fetch_add(1, Ordering::Relaxed);
        let expiry = *self.invoices.lock().unwrap().get(&intent.id)?;
        let invoice = self.invoice(intent, expiry);
        assert_eq!(invoice.reference, reference);
        if self.block_reopen.load(Ordering::Relaxed) {
            self.reopening.notify_one();
            self.reopened.notified().await;
        }
        (!invoice.closed).then(|| (invoice, format!("{DESTINATION}/invoice/{reference}")))
    }
}

pub(super) async fn run(
    admin: &PgPool,
    (worker, reconnect): (PgPool, impl AsyncFnOnce() -> PgPool),
    origin: &str,
    signer: &payment::Adapter,
    browser: &mut Process,
    exchange: &Path,
    (a, b): (Uuid, Uuid),
) {
    let merchant = Merchant {
        signer,
        invoices: Mutex::new(HashMap::new()),
        creates: AtomicUsize::new(0),
        reopens: AtomicUsize::new(0),
        lifetime_s: AtomicUsize::new(1200),
        wrong_total: AtomicBool::new(false),
        block_reopen: AtomicBool::new(false),
        reopening: Notify::new(),
        reopened: Notify::new(),
    };
    checkpoint(browser, &exchange.join("checkout-empty")).await;
    let methods = [
        SandboxMethod {
            code: "card",
            label: "Card",
            min_credit_minor: 1000,
            max_credit_minor: 2000,
            fee_minor: 50,
            custom_amount: true,
            merchant_pays: true,
            customer_pays: true,
        },
        SandboxMethod {
            code: "bank",
            label: "Bank transfer",
            min_credit_minor: 1000,
            max_credit_minor: 2000,
            fee_minor: 0,
            custom_amount: false,
            merchant_pays: true,
            customer_pays: false,
        },
        SandboxMethod {
            code: "voucher",
            label: "Voucher",
            min_credit_minor: 2500,
            max_credit_minor: 3000,
            fee_minor: 0,
            custom_amount: false,
            merchant_pays: true,
            customer_pays: false,
        },
    ];
    let install =
        |site, label| register_sandbox_profile(&worker, site, signer.binding, label, &methods);
    assert!(
        !install("https://relay-fixture.invalid", "Fixture payments")
            .await
            .unwrap()
    );
    assert!(install(origin, "Fixture payments").await.unwrap());
    assert!(install(origin, "Fixture payments").await.unwrap());
    assert!(!install(origin, "Unexpected replacement").await.unwrap());
    fs::write(exchange.join("checkout-ready"), []).unwrap();
    checkpoint(browser, &exchange.join("checkout-check-aged")).await;
    assert_eq!(create_next(&worker, &merchant).await, Ok(None));
    age_current_passkey(admin).await;
    sqlx::query("UPDATE relay_hosted.checkout_builder SET checked_until=now()-interval '1 second'")
        .execute(admin)
        .await
        .unwrap();
    fs::write(exchange.join("checkout-check-expired"), []).unwrap();

    let expiring = exchange.join("checkout-reviewed");
    checkpoint(browser, &expiring).await;
    assert_eq!(create_next(&worker, &merchant).await, Ok(None));
    assert_wallets(admin, a, b, 0).await;
    let quote: Uuid = fs::read_to_string(expiring).unwrap().parse().unwrap();
    let changed = sqlx::query(
        "UPDATE relay_hosted.checkout_quotes SET expires_at=now()-interval '1 second' WHERE id=$1",
    )
    .bind(quote)
    .execute(admin)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(changed, 1);
    fs::write(exchange.join("checkout-expired"), []).unwrap();

    let shorten = exchange.join("checkout-shorten-quote");
    checkpoint(browser, &shorten).await;
    let quote: Uuid = fs::read_to_string(shorten).unwrap().parse().unwrap();
    let expires: i64 = sqlx::query_scalar(
        "UPDATE relay_hosted.checkout_quotes SET expires_at=clock_timestamp()+interval '10 seconds' \
         WHERE id=$1 RETURNING ceil(extract(epoch FROM expires_at))::bigint",
    )
    .bind(quote)
    .fetch_one(admin)
    .await
    .unwrap();
    fs::write(exchange.join("checkout-short-quote"), expires.to_string()).unwrap();

    let confirmed = exchange.join("checkout-confirmed");
    checkpoint(browser, &confirmed).await;
    let id: Uuid = fs::read_to_string(confirmed).unwrap().parse().unwrap();
    let (first, parallel) = tokio::join!(
        create_next(&worker, &merchant),
        create_next(&worker, &merchant)
    );
    assert!(
        matches!((first, parallel), (Ok(Some(x)), Ok(None)) | (Ok(None), Ok(Some(x))) if x==id)
    );
    let reference = id.to_string();
    assert_eq!(
        reconcile_paid(&worker, signer, &signer.paid("early", &reference)).await,
        Err(CreditError::InvoiceMismatch)
    );
    assert_eq!(merchant.creates.load(Ordering::Relaxed), 1);
    fs::write(exchange.join("checkout-uncertain"), []).unwrap();

    let cap = exchange.join("checkout-cap");
    checkpoint(browser, &cap).await;
    let expiring: Uuid = fs::read_to_string(cap).unwrap().parse().unwrap();
    let count = |space| {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM relay_hosted.payment_intents WHERE workspace_id=$1",
        )
        .bind(space)
        .fetch_one(admin)
    };
    assert_eq!(count(a).await.unwrap(), 3);
    assert_eq!(count(b).await.unwrap(), 2);
    assert_wallets(admin, a, b, 0).await;
    let changes: (i64, i64, i64, i64, bool) = sqlx::query_as(
        "SELECT (SELECT open_limit::bigint FROM relay_hosted.checkout_policy),
            (SELECT count(*) FROM relay_hosted.checkout_builder_operations WHERE action='publish'),
            (SELECT count(*) FROM relay_hosted.checkout_builder_operations WHERE action='disable'),
            (SELECT count(*) FROM relay_hosted.checkout_methods
                WHERE disabled_at IS NULL AND merchant_binding_id=$1 AND method_code='card'),
            NOT EXISTS (SELECT FROM relay_hosted.checkout_builder_operations op
                WHERE op.confirmed_by_passkey_id IS DISTINCT FROM (
                    SELECT k.passkey_id FROM relay_hosted.admin_credentials k
                    WHERE k.user_id=op.actor_id AND op.action IN ('publish','disable')))",
    )
    .bind(signer.binding)
    .fetch_one(admin)
    .await
    .unwrap();
    assert_eq!(changes, (2, 3, 1, 0, true));
    worker.close().await;
    let worker = reconnect().await;
    merchant.wrong_total.store(true, Ordering::Relaxed);
    assert_eq!(
        reconcile_invoice(&worker, &merchant, id).await,
        Err(CheckoutError::Mismatch)
    );
    merchant.wrong_total.store(false, Ordering::Relaxed);
    reconcile_invoice(&worker, &merchant, id).await.unwrap();
    reconcile_invoice(&worker, &merchant, id).await.unwrap();
    assert_eq!(merchant.creates.load(Ordering::Relaxed), 1);
    fs::write(exchange.join("checkout-found"), []).unwrap();

    for _ in 0..2 {
        serve_resume(&worker, &merchant, browser).await;
    }
    checkpoint(browser, &exchange.join("checkout-resumed-twice")).await;
    assert!(now_s() >= expires);
    assert_eq!(count(a).await.unwrap(), 3);
    assert_eq!(count(b).await.unwrap(), 2);
    assert_eq!(merchant.creates.load(Ordering::Relaxed), 1);
    assert_wallets(admin, a, b, 0).await;

    merchant.wrong_total.store(true, Ordering::Relaxed);
    fs::write(exchange.join("checkout-mismatch-ready"), []).unwrap();
    serve_resume(&worker, &merchant, browser).await;
    checkpoint(browser, &exchange.join("checkout-mismatch-checked")).await;
    merchant.wrong_total.store(false, Ordering::Relaxed);
    reconcile_invoice(&worker, &merchant, id).await.unwrap();
    fs::write(exchange.join("checkout-mismatch-cleared"), []).unwrap();

    checkpoint(browser, &exchange.join("checkout-start-expiry")).await;
    merchant.lifetime_s.store(8, Ordering::Relaxed);
    assert_eq!(create_next(&worker, &merchant).await, Ok(Some(expiring)));
    reconcile_invoice(&worker, &merchant, expiring)
        .await
        .unwrap();
    fs::write(exchange.join("checkout-expiring"), []).unwrap();
    checkpoint(browser, &exchange.join("checkout-expiring-visible")).await;
    let deadline = merchant.invoices.lock().unwrap()[&expiring];
    while now_s() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    fs::write(exchange.join("checkout-invoice-expired"), []).unwrap();

    checkpoint(browser, &exchange.join("checkout-fake-return")).await;
    assert_wallets(admin, a, b, 0).await;
    assert_eq!(merchant.reopens.load(Ordering::Relaxed), 3);
    assert_eq!(resume_next(&worker, &merchant).await, Ok(false));
    assert_eq!(
        reconcile_paid(&worker, signer, &signer.paid("paid", &reference).tamper()).await,
        Err(CreditError::Unverified)
    );
    // Credit independently while the merchant lookup is returning its now stale payable result.
    merchant.block_reopen.store(true, Ordering::Relaxed);
    fs::write(exchange.join("checkout-pay-during-resume"), []).unwrap();
    let (_, receipt) = tokio::join!(serve_resume(&worker, &merchant, browser), async {
        tokio::time::timeout(Duration::from_secs(25), merchant.reopening.notified())
            .await
            .unwrap();
        let receipt = reconcile_paid(&worker, signer, &signer.paid("paid", &reference))
            .await
            .unwrap();
        merchant.reopened.notify_one();
        receipt
    });
    assert_eq!(merchant.reopens.load(Ordering::Relaxed), 4);
    for event in ["paid", "paid-alternate"] {
        assert_eq!(
            reconcile_paid(&worker, signer, &signer.paid(event, &reference)).await,
            Ok(receipt)
        );
    }
    let rejected = create_next(&worker, &merchant).await.unwrap().unwrap();
    assert_ne!(rejected, id);
    let closed: (Uuid, bool, bool) = sqlx::query_as(
        "SELECT workspace_id,invoice_closed_at IS NOT NULL,merchant_reference IS NULL FROM relay_hosted.payment_intents WHERE id=$1",
    ).bind(rejected).fetch_one(admin).await.unwrap();
    assert_eq!(closed, (a, true, true));
    assert_wallets(admin, a, b, CREDIT).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM relay_hosted.checkout_methods m JOIN relay_hosted.payment_intents i
                ON i.checkout_method_id=m.id WHERE i.id=$1 AND m.fee_payer='customer'
                    AND m.fee_minor=50 AND m.disabled_at IS NOT NULL",
        )
        .bind(id)
        .fetch_one(admin)
        .await
        .unwrap(),
        1
    );
}

async fn serve_resume(worker: &PgPool, merchant: &Merchant<'_>, browser: &mut Process) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            assert!(browser.0.try_wait().unwrap().is_none());
            if resume_next(worker, merchant).await.unwrap() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("resume worker did not receive a check");
}

async fn assert_wallets(admin: &PgPool, a: Uuid, b: Uuid, amount: i64) {
    payment::assert_wallet(admin, a, amount, i64::from(amount > 0)).await;
    payment::assert_wallet(admin, b, 0, 0).await;
}
