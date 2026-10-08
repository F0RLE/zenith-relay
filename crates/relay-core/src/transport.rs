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
    let retry_after_header = headers.get("retry-after")?.to_str().ok()?.trim();
    if let Ok(seconds) = retry_after_header.parse::<u64>() {
        return Some(seconds.saturating_mul(1_000));
    }
    Some(
        httpdate::parse_http_date(retry_after_header)
            .ok()?
            .duration_since(now)
            .ok()?
            .as_millis()
            .min(u128::from(u64::MAX)) as u64,
    )
}

pub(crate) async fn collect(response: reqwest::Response) -> Result<Vec<u8>> {
    let mut response_bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        response_bytes.extend_from_slice(&chunk?);
    }
    Ok(response_bytes)
}

pub async fn collect_limited(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(Error::UpstreamBodyTooLarge);
    }
    let mut response_bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if response_bytes.len().saturating_add(chunk.len()) > limit {
            return Err(Error::UpstreamBodyTooLarge);
        }
        response_bytes.extend_from_slice(&chunk);
    }
    Ok(response_bytes)
}
