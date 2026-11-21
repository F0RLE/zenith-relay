//! The caller holds the account/ledger single-flight guard for the whole action.
//! Host mutations must validate the captured account revision atomically with
//! their write. A stale action must never clear a newer redemption marker.

use super::{
    ResetCreditsError, ResetCreditsSnapshot, ResetErrorKind, ResetHttpResponse,
    ensure_reset_success, parse_reset_response,
};
use reqwest::StatusCode;
use std::future::Future;

pub trait ResetHost: Sync {
    type Authorization: Send + Sync;
    type Error: Send;

    fn validate(&self) -> Result<(), Self::Error>;
    fn error(error: ResetCreditsError) -> Self::Error;
    fn prepare(&self) -> impl Future<Output = Result<Self::Authorization, Self::Error>> + Send;
    fn recover(
        &self,
        rejected: &Self::Authorization,
    ) -> impl Future<Output = Result<Self::Authorization, Self::Error>> + Send;
    fn balance(
        &self,
        authorization: &Self::Authorization,
    ) -> impl Future<Output = Result<ResetHttpResponse, Self::Error>> + Send;
    /// Recheck the rule, membership and quota under the same mutation lock as
    /// the durable marker. Return false without writing when no longer needed.
    fn record_dispatch(
        &self,
        expiry_ms: Option<u64>,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send;
    fn clear_rejected(&self) -> Result<(), Self::Error>;
    fn redeem(
        &self,
        authorization: &Self::Authorization,
        request_id: &str,
    ) -> impl Future<Output = Result<ResetHttpResponse, Self::Error>> + Send;
}

pub fn definitely_rejected(status: StatusCode) -> bool {
    status.is_client_error()
        && !matches!(
            status,
            StatusCode::REQUEST_TIMEOUT | StatusCode::CONFLICT | StatusCode::TOO_EARLY
        )
}

pub fn available_expiry(snapshot: &ResetCreditsSnapshot) -> Result<Option<u64>, ResetCreditsError> {
    let message = match snapshot.available_count {
        Some(1..) => {
            return Ok(snapshot
                .next_expires_at
                .and_then(|expiry| u64::try_from(expiry).ok())
                .and_then(|expiry| expiry.checked_mul(1_000)));
        }
        Some(0) => "no reset credits are currently available for this account",
        None => "reset credit availability is not confirmed for this account",
    };
    Err(ResetCreditsError::new(ResetErrorKind::Conflict, message))
}

pub async fn redeem<H: ResetHost>(host: &H, request_id: &str) -> Result<bool, H::Error> {
    host.validate()?;
    let mut authorization = host.prepare().await?;
    host.validate()?;
    let mut balance = host.balance(&authorization).await?;
    host.validate()?;
    if balance.status == StatusCode::UNAUTHORIZED {
        authorization = host.recover(&authorization).await?;
        host.validate()?;
        balance = host.balance(&authorization).await?;
        host.validate()?;
    }
    let expiry =
        available_expiry(&parse_reset_response(balance).map_err(H::error)?).map_err(H::error)?;
    if !host.record_dispatch(expiry).await? {
        return Ok(false);
    }
    host.validate()?;
    let mut response = host.redeem(&authorization, request_id).await?;
    host.validate()?;
    if response.status == StatusCode::UNAUTHORIZED {
        host.clear_rejected()?;
        authorization = host.recover(&authorization).await?;
        host.validate()?;
        if !host.record_dispatch(expiry).await? {
            return Ok(false);
        }
        host.validate()?;
        // Only a proven authorization rejection permits another dispatch, and
        // both attempts retain the same provider idempotency key.
        response = host.redeem(&authorization, request_id).await?;
        host.validate()?;
    }
    if definitely_rejected(response.status) {
        host.clear_rejected()?;
    }
    ensure_reset_success(response).map_err(H::error)?;
    Ok(true)
}

#[cfg(test)]
mod tests;
