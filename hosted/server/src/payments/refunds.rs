//! Private voluntary refunds: issue after a durable claim; recover by lookup.
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
pub struct AuthorizedRefund {
    /// Merchant idempotency and lookup key, not the browser operation ID.
    pub id: Uuid,
    pub invoice_reference: String,
    pub currency: String,
    pub amount_minor: i64,
    terminal: bool,
}

pub struct RefundEvidence {
    pub merchant_binding: Uuid,
    pub id: Uuid,
    pub invoice_reference: String,
    pub currency: String,
    pub amount_minor: i64,
    pub reference: String,
}

pub enum RefundObservation {
    Succeeded(RefundEvidence),
    /// The merchant proves this operation has not refunded and cannot still refund.
    Failed(RefundEvidence),
    Uncertain,
}

pub trait RefundAdapter {
    fn binding(&self) -> Uuid;
    fn submit(&self, refund: &AuthorizedRefund) -> impl Future<Output = RefundObservation> + Send;
    /// Read the exact refund operation. A missing record alone is not a failure.
    fn lookup(&self, refund: &AuthorizedRefund) -> impl Future<Output = RefundObservation> + Send;
}

#[derive(Debug, PartialEq, Eq)]
pub enum RefundError {
    Storage,
    Mismatch,
}

const MERCHANT_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn submit_next<A: RefundAdapter>(
    worker: &PgPool,
    adapter: &A,
) -> Result<Option<Uuid>, RefundError> {
    let binding = adapter.binding();
    let mut tx = worker.begin().await.map_err(|_| RefundError::Storage)?;
    let refund = sqlx::query_as("SELECT * FROM relay_hosted.claim_payment_refund($1)")
        .bind(binding)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| RefundError::Storage)?;
    tx.commit().await.map_err(|_| RefundError::Storage)?;
    let Some(refund): Option<AuthorizedRefund> = refund else {
        return Ok(None);
    };
    let observation = tokio::time::timeout(MERCHANT_TIMEOUT, adapter.submit(&refund))
        .await
        .unwrap_or(RefundObservation::Uncertain);
    record(worker, binding, &refund, observation).await?;
    Ok(Some(refund.id))
}

/// Page from `None` on startup. Bound the work per pass and revisit uncertain
/// cases on later passes; this list never authorizes another submission.
pub async fn pending_lookups(
    worker: &PgPool,
    binding: Uuid,
    after: Option<Uuid>,
) -> Result<Vec<Uuid>, RefundError> {
    sqlx::query_scalar("SELECT * FROM relay_hosted.pending_refund_lookups($1,$2)")
        .bind(binding)
        .bind(after)
        .fetch_all(worker)
        .await
        .map_err(|_| RefundError::Storage)
}

pub async fn reconcile<A: RefundAdapter>(
    worker: &PgPool,
    adapter: &A,
    id: Uuid,
) -> Result<(), RefundError> {
    let binding = adapter.binding();
    let refund: AuthorizedRefund =
        sqlx::query_as("SELECT * FROM relay_hosted.refund_for_lookup($1,$2)")
            .bind(binding)
            .bind(id)
            .fetch_optional(worker)
            .await
            .map_err(|_| RefundError::Storage)?
            .ok_or(RefundError::Mismatch)?;
    if refund.terminal {
        return Ok(());
    }
    let observation = tokio::time::timeout(MERCHANT_TIMEOUT, adapter.lookup(&refund))
        .await
        .unwrap_or(RefundObservation::Uncertain);
    record(worker, binding, &refund, observation).await
}

async fn record(
    worker: &PgPool,
    binding: Uuid,
    refund: &AuthorizedRefund,
    observation: RefundObservation,
) -> Result<(), RefundError> {
    let (e, succeeded) = match observation {
        RefundObservation::Succeeded(e) => (e, true),
        RefundObservation::Failed(e) => (e, false),
        RefundObservation::Uncertain => return Ok(()),
    };
    if (
        e.merchant_binding,
        e.id,
        &e.invoice_reference,
        &e.currency,
        e.amount_minor,
    ) != (
        binding,
        refund.id,
        &refund.invoice_reference,
        &refund.currency,
        refund.amount_minor,
    ) || !super::valid_reference(&e.reference)
    {
        return Err(RefundError::Mismatch);
    }
    let stored = sqlx::query_scalar::<_, bool>(
        "SELECT relay_hosted.observe_payment_refund($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(binding)
    .bind(refund.id)
    .bind(&refund.invoice_reference)
    .bind(&refund.currency)
    .bind(refund.amount_minor)
    .bind(&e.reference)
    .bind(succeeded)
    .fetch_one(worker)
    .await
    .map_err(|_| RefundError::Storage)?;
    stored.then_some(()).ok_or(RefundError::Mismatch)
}
