use crate::{Error, Result};
use futures_util::StreamExt;

pub(crate) const MAX_MODEL_CATALOG_BODY_BYTES: usize = 4 * 1024 * 1024;

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
