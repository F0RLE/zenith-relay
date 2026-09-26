use crate::{Error, Result};
use futures_util::StreamExt;

pub(crate) const MAX_MODEL_CATALOG_BODY_BYTES: usize = 4 * 1024 * 1024;

/// A relative floor measured at receipt. Consumers must preserve the floor
/// when converting it to a monotonic deadline; do not cap a long provider hint
/// to a shorter polling interval.
pub(crate) fn retry_after_ms(
    headers: &reqwest::header::HeaderMap,
    now: std::time::SystemTime,
) -> Option<u64> {
    let value = headers.get("retry-after")?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds.saturating_mul(1_000));
    }
    Some(
        httpdate::parse_http_date(value)
            .ok()?
            .duration_since(now)
            .ok()?
            .as_millis()
            .min(u128::from(u64::MAX)) as u64,
    )
}

pub(crate) async fn collect_limited(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(Error::UpstreamBodyTooLarge);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(Error::UpstreamBodyTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
