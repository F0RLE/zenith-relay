//! Hosted checkout and private settlement from verified merchant evidence.
pub mod checkout;
mod customer;
pub mod refunds;
pub mod registry;
pub(crate) use customer::routes;
use sqlx::PgPool;
use uuid::Uuid;

/// Normalized paid evidence, not deserialized by any public Hosted route.
pub struct PaidInvoice {
    /// Immutable registration used for verification, never a caller-selected wallet or binding.
    pub merchant_binding: Uuid,
    pub event_reference: String,
    pub invoice_reference: String,
    pub currency: String,
    /// Integer settlement minor units in the adapter's currency, not wallet microUSD.
    pub paid_minor: i64,
}

pub enum VerificationError {
    Rejected,
    Unavailable,
}

pub trait PaidInvoiceVerifier {
    type Delivery: ?Sized;

    /// Verify the final paid state for the configured merchant and environment.
    /// Any bounded merchant lookup finishes before accounting acquires a connection.
    fn verify_paid(
        &self,
        delivery: &Self::Delivery,
    ) -> impl Future<Output = Result<PaidInvoice, VerificationError>> + Send;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreditError {
    Unverified,
    InvalidEvidence,
    InvoiceMismatch,
    EventConflict,
    WalletCapacity,
    Busy,
    Unavailable,
}

fn storage(error: sqlx::Error) -> CreditError {
    match error.as_database_error().and_then(|e| e.code()).as_deref() {
        Some("RH001") => CreditError::EventConflict,
        Some("RH002") => CreditError::WalletCapacity,
        Some("40001" | "40P01" | "55P03") => CreditError::Busy,
        _ => CreditError::Unavailable,
    }
}

fn valid_reference(s: &str) -> bool {
    (1..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic())
}

/// Returns the same durable receipt on redelivery, including after a lost commit reply.
pub async fn reconcile_paid<V: PaidInvoiceVerifier>(
    pool: &PgPool,
    verifier: &V,
    delivery: &V::Delivery,
) -> Result<Uuid, CreditError> {
    let notice = verifier.verify_paid(delivery).await.map_err(|e| match e {
        VerificationError::Rejected => CreditError::Unverified,
        VerificationError::Unavailable => CreditError::Unavailable,
    })?;
    if notice.merchant_binding.is_nil()
        || !valid_reference(&notice.event_reference)
        || !valid_reference(&notice.invoice_reference)
        || notice.currency.len() != 3
        || !notice.currency.bytes().all(|b| b.is_ascii_uppercase())
        || notice.paid_minor <= 0
    {
        return Err(CreditError::InvalidEvidence);
    }
    // Retry only a known aborted transaction. An ambiguous commit goes back to
    // the worker for reconciliation against the original verified delivery.
    match post(pool, &notice).await {
        Err(CreditError::Busy) => post(pool, &notice).await,
        result => result,
    }
}

async fn post(pool: &PgPool, notice: &PaidInvoice) -> Result<Uuid, CreditError> {
    let mut tx = pool
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
        .await
        .map_err(storage)?;
    let receipt = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT relay_hosted.post_verified_payment($1,$2,$3,$4,$5)",
    )
    .bind(notice.merchant_binding)
    .bind(&notice.event_reference)
    .bind(&notice.invoice_reference)
    .bind(&notice.currency)
    .bind(notice.paid_minor)
    .fetch_one(&mut *tx)
    .await
    .map_err(storage)?
    .ok_or(CreditError::InvoiceMismatch)?;
    tx.commit().await.map_err(storage)?;
    Ok(receipt)
}
