//! Private merchant calls. Claim once before I/O; recover only by lookup.
use sqlx::PgPool;
use std::{future::ready, time::Duration};
use url::Url;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
pub struct AcceptedCheckout {
    /// Use this as the merchant's creation and lookup key.
    pub id: Uuid,
    pub method_code: String,
    pub currency: String,
    pub payable_minor: i64,
}

pub struct Invoice {
    pub reference: String,
    pub currency: String,
    pub payable_minor: i64,
    pub expires_at_s: i64,
    /// The merchant verified that this invoice can no longer be paid.
    pub closed: bool,
    /// This invoice is known payable and the adapter supports reopening it.
    pub can_resume: bool,
}

pub enum Observation {
    Invoice(Invoice),
    /// Proof that the operation neither created nor can still create an invoice.
    NotCreated,
    Uncertain,
}

pub trait CheckoutAdapter {
    fn binding(&self) -> Uuid;
    fn create(&self, intent: &AcceptedCheckout) -> impl Future<Output = Observation> + Send;
    fn lookup(&self, intent: &AcceptedCheckout) -> impl Future<Output = Observation> + Send;

    /// Fixed, qualified checkout origin. Omit it for adapters without web handoffs.
    fn checkout_origin(&self) -> Option<&str> {
        None
    }

    /// Read the original invoice. Return a handoff only with proof it is
    /// unpaid and payable now; this operation must never create an invoice.
    fn reopen(
        &self,
        _intent: &AcceptedCheckout,
        _reference: &str,
    ) -> impl Future<Output = Option<(Invoice, String)>> + Send {
        ready(None)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CheckoutError {
    Storage,
    Mismatch,
}

const MERCHANT_TIMEOUT: Duration = Duration::from_secs(10);
const RESUME_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(sqlx::FromRow)]
struct ResumeCheck {
    probe: Uuid,
    #[sqlx(flatten)]
    accepted: AcceptedCheckout,
    invoice_reference: String,
    expires_at_s: i64,
}

/// Serve one pending lookup for this merchant with the private worker role.
/// A lost or expired probe is disposable and never retries invoice creation.
pub async fn resume_next<A: CheckoutAdapter>(
    worker: &PgPool,
    adapter: &A,
) -> Result<bool, CheckoutError> {
    let binding = adapter.binding();
    let Some(probe): Option<ResumeCheck> =
        sqlx::query_as("SELECT * FROM relay_hosted.claim_checkout_resume($1)")
            .bind(binding)
            .fetch_optional(worker)
            .await
            .map_err(|_| CheckoutError::Storage)?
    else {
        return Ok(false);
    };
    let destination = if let Some(origin) = adapter.checkout_origin() {
        tokio::time::timeout(
            RESUME_TIMEOUT,
            adapter.reopen(&probe.accepted, &probe.invoice_reference),
        )
        .await
        .ok()
        .flatten()
        .filter(|(i, _)| {
            i.can_resume
                && !i.closed
                && i.reference == probe.invoice_reference
                && i.currency == probe.accepted.currency
                && i.payable_minor == probe.accepted.payable_minor
                && i.expires_at_s == probe.expires_at_s
        })
        .and_then(|(_, raw)| handoff(origin, &raw))
    } else {
        None
    };
    sqlx::query("SELECT relay_hosted.finish_checkout_resume($1,$2,$3,$4)")
        .bind(binding)
        .bind(probe.accepted.id)
        .bind(probe.probe)
        .bind(destination.as_ref().map(Url::as_str))
        .execute(worker)
        .await
        .map_err(|_| CheckoutError::Storage)?;
    Ok(true)
}

fn handoff(origin: &str, raw: &str) -> Option<Url> {
    if raw.len() > 2048
        || !raw.starts_with("https://")
        || raw
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control() || b == b'\\')
        || raw
            .split(['/', '?', '#'])
            .nth(2)
            .is_none_or(|authority| authority.is_empty() || authority.contains('@'))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    (url.host().is_some()
        && url.origin().ascii_serialization() == origin
        && url.fragment().is_none()
        && url.as_str().len() <= 2048)
        .then_some(url)
}

pub async fn create_next<A: CheckoutAdapter>(
    worker: &PgPool,
    adapter: &A,
) -> Result<Option<Uuid>, CheckoutError> {
    let binding = adapter.binding();
    let mut tx = worker.begin().await.map_err(|_| CheckoutError::Storage)?;
    let accepted = sqlx::query_as("SELECT * FROM relay_hosted.claim_checkout_invoice($1)")
        .bind(binding)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| CheckoutError::Storage)?;
    tx.commit().await.map_err(|_| CheckoutError::Storage)?;
    let Some(accepted): Option<AcceptedCheckout> = accepted else {
        return Ok(None);
    };
    let observation = tokio::time::timeout(MERCHANT_TIMEOUT, adapter.create(&accepted))
        .await
        .unwrap_or(Observation::Uncertain);
    record(
        worker,
        binding,
        &accepted,
        adapter.checkout_origin(),
        observation,
    )
    .await?;
    Ok(Some(accepted.id))
}

pub async fn reconcile_invoice<A: CheckoutAdapter>(
    worker: &PgPool,
    adapter: &A,
    id: Uuid,
) -> Result<(), CheckoutError> {
    let binding = adapter.binding();
    let accepted = sqlx::query_as("SELECT * FROM relay_hosted.checkout_invoice_for_lookup($1,$2)")
        .bind(binding)
        .bind(id)
        .fetch_optional(worker)
        .await
        .map_err(|_| CheckoutError::Storage)?
        .ok_or(CheckoutError::Mismatch)?;
    let observation = tokio::time::timeout(MERCHANT_TIMEOUT, adapter.lookup(&accepted))
        .await
        .unwrap_or(Observation::Uncertain);
    record(
        worker,
        binding,
        &accepted,
        adapter.checkout_origin(),
        observation,
    )
    .await
}

async fn record(
    worker: &PgPool,
    binding: Uuid,
    accepted: &AcceptedCheckout,
    origin: Option<&str>,
    observation: Observation,
) -> Result<(), CheckoutError> {
    let (reference, currency, payable, expiry, closed, can_resume) = match observation {
        Observation::Uncertain => (None, None, None, None, false, false),
        Observation::NotCreated => (None, None, None, None, true, false),
        Observation::Invoice(i) => {
            if i.currency != accepted.currency
                || i.payable_minor != accepted.payable_minor
                || !super::valid_reference(&i.reference)
            {
                return Err(CheckoutError::Mismatch);
            }
            (
                Some(i.reference),
                Some(i.currency),
                Some(i.payable_minor),
                Some(i.expires_at_s),
                i.closed,
                i.can_resume && origin.is_some(),
            )
        }
    };
    let stored = sqlx::query_scalar::<_, bool>(
        "SELECT relay_hosted.observe_checkout_invoice($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(binding)
    .bind(accepted.id)
    .bind(reference)
    .bind(currency)
    .bind(payable)
    .bind(expiry)
    .bind(closed)
    .bind(can_resume)
    .fetch_one(worker)
    .await
    .map_err(|_| CheckoutError::Storage)?;
    stored.then_some(()).ok_or(CheckoutError::Mismatch)
}

#[cfg(test)]
mod tests {
    use super::handoff;

    #[test]
    fn handoff_keeps_only_the_adapter_origin() {
        const ORIGIN: &str = "https://checkout.merchant.invalid";
        let valid = format!("{ORIGIN}/invoice/one?token=fixture");
        assert_eq!(handoff(ORIGIN, &valid).unwrap().as_str(), valid);
        for bad in [
            "http://checkout.merchant.invalid/invoice/one",
            "https://checkout.merchant.invalid:444/invoice/one",
            "https://checkout.merchant.invalid.evil.invalid/",
            "https://checkout.merchant.invalid@evil.invalid/",
            "https://@checkout.merchant.invalid/",
            "https://checkout.merchant.invalid\\else",
            "https://checkout.merchant.invalid/a#another",
            " https://checkout.merchant.invalid/",
            "//checkout.merchant.invalid/invoice/one",
            "javascript:void(0)",
        ] {
            assert!(handoff(ORIGIN, bad).is_none(), "{bad}");
        }
    }
}
