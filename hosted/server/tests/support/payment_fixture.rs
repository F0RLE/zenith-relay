use ring::{hmac, rand::SystemRandom};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;
use zenith_relay_hosted::payments::{PaidInvoice, PaidInvoiceVerifier, VerificationError};

pub struct Adapter {
    pub binding: Uuid,
    environment: String,
    key: hmac::Key,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire<'a> {
    environment: &'a str,
    kind: &'a str,
    event: &'a str,
    invoice: &'a str,
    currency: &'a str,
    amount: i64,
}

pub struct Delivery {
    body: Vec<u8>,
    signature: hmac::Tag,
}

impl Delivery {
    pub fn tamper(mut self) -> Self {
        self.body.push(b' ');
        self
    }
}

impl Adapter {
    pub fn signing_key() -> hmac::Key {
        hmac::Key::generate(hmac::HMAC_SHA256, &SystemRandom::new()).unwrap()
    }

    pub async fn new(db: &PgPool, environment: &str, key: hmac::Key) -> Self {
        let binding = sqlx::query_scalar(
            "INSERT INTO relay_hosted.merchant_bindings (provider_id,environment,merchant_identity)
             VALUES ('fixture',$1,'fixture-shop') RETURNING id",
        )
        .bind(environment)
        .fetch_one(db)
        .await
        .unwrap();
        Self {
            binding,
            environment: environment.into(),
            key,
        }
    }

    pub fn sign(
        &self,
        kind: &str,
        event: &str,
        invoice: &str,
        currency: &str,
        amount: i64,
    ) -> Delivery {
        let body = serde_json::to_vec(&Wire {
            environment: &self.environment,
            kind,
            event,
            invoice,
            currency,
            amount,
        })
        .unwrap();
        Delivery {
            signature: hmac::sign(&self.key, &body),
            body,
        }
    }

    pub fn rotate_key(&mut self) {
        self.key = Self::signing_key();
    }

    pub fn paid(&self, event: &str, invoice: &str) -> Delivery {
        self.sign("paid", event, invoice, "USD", 1050)
    }
}

impl PaidInvoiceVerifier for Adapter {
    type Delivery = Delivery;

    async fn verify_paid(&self, delivery: &Delivery) -> Result<PaidInvoice, VerificationError> {
        use VerificationError::*;
        hmac::verify(&self.key, &delivery.body, delivery.signature.as_ref())
            .map_err(|_| Rejected)?;
        let wire: Wire<'_> = serde_json::from_slice(&delivery.body).map_err(|_| Rejected)?;
        if wire.environment != self.environment {
            return Err(Rejected);
        }
        match wire.kind {
            "paid" => (),
            "lookup-unavailable" => return Err(Unavailable),
            _ => return Err(Rejected),
        }
        Ok(PaidInvoice {
            merchant_binding: self.binding,
            event_reference: wire.event.into(),
            invoice_reference: wire.invoice.into(),
            currency: wire.currency.into(),
            paid_minor: wire.amount,
        })
    }
}

/// Establish only the upstream checkout facts. Never seed the wallet or journal.
pub async fn invoice(
    db: &PgPool,
    workspace: Uuid,
    binding: Uuid,
    reference: &str,
    credit: i64,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO relay_hosted.payment_intents (workspace_id,operation_id,quote_id,merchant_binding_id,
         merchant_reference,quoted_currency,quoted_amount_minor,credit_micro_usd)
         VALUES ($1,gen_random_uuid(),gen_random_uuid(),$2,$3,'USD',1050,$4) RETURNING id",
    )
    .bind(workspace)
    .bind(binding)
    .bind(reference)
    .bind(credit)
    .fetch_one(db)
    .await
    .unwrap()
}

pub async fn assert_wallet(db: &PgPool, workspace: Uuid, credit: i64, receipts: i64) {
    assert_wallet_with_holds(db, workspace, credit, 0, receipts).await;
}

pub async fn assert_wallet_with_holds(
    db: &PgPool,
    workspace: Uuid,
    available: i64,
    held: i64,
    receipts: i64,
) {
    let state: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT w.available_micro_usd,w.reserved_micro_usd,w.revision,
            (SELECT count(*) FROM relay_hosted.journal_transactions WHERE workspace_id=w.workspace_id),
            (SELECT coalesce(sum(p.amount_micro_usd),0)::bigint FROM relay_hosted.ledger_accounts a
             JOIN relay_hosted.journal_postings p ON p.account_id=a.id WHERE a.workspace_id=w.workspace_id)
         FROM relay_hosted.wallets w WHERE w.workspace_id=$1",
    )
    .bind(workspace)
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(
        state,
        (available, held, 1 + receipts, receipts, available + held)
    );
}
