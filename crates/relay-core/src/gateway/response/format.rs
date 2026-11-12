use axum::body::Bytes;
use axum::http::{Response, header};
use futures_util::StreamExt;
use reqwest::ResponseBuilderExt;
use std::time::{Duration, Instant};

const MAX_PREFIX_BYTES: usize = 512;

/// Restores consumed chunks unchanged; only a recognized SSE prefix repairs MIME.
pub(in crate::gateway) async fn normalize_event_stream(
    mut upstream: reqwest::Response,
    started: Instant,
    transport: Option<&mut crate::usage::TransportDiagnostics>,
    progress_timeout: Option<Duration>,
) -> reqwest::Response {
    let declared_sse = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"));
    let decoded = upstream
        .headers()
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .all(|value| value.as_bytes().eq_ignore_ascii_case(b"identity"));
    if !upstream.status().is_success() || declared_sse || !decoded {
        return upstream;
    }
    let status = upstream.status();
    let version = upstream.version();
    let url = upstream.url().clone();
    let mut headers = std::mem::take(upstream.headers_mut());
    let extensions = std::mem::take(upstream.extensions_mut());
    let mut stream: crate::gateway::streaming::UpstreamStream = Box::pin(upstream.bytes_stream());
    if progress_timeout.is_some() {
        stream = crate::gateway::streaming::guard_progress(stream, progress_timeout);
    }
    let mut prefix = Vec::with_capacity(MAX_PREFIX_BYTES);
    let mut chunks = Vec::new();
    let mut detected = false;
    let mut first_body_ms = None;
    for _ in 0..MAX_PREFIX_BYTES {
        match stream.next().await {
            Some(Ok(chunk)) => {
                if !chunk.is_empty() {
                    first_body_ms.get_or_insert(started.elapsed().as_millis() as u64);
                }
                let take = chunk.len().min(MAX_PREFIX_BYTES - prefix.len());
                prefix.extend_from_slice(&chunk[..take]);
                chunks.push(Ok::<Bytes, reqwest::Error>(chunk));
                if let Some(sse) = event_stream_prefix(&prefix) {
                    detected = sse;
                    break;
                }
                if prefix.len() == MAX_PREFIX_BYTES {
                    break;
                }
            }
            None => break,
            Some(Err(error)) => {
                chunks.push(Err(error));
                break;
            }
        }
    }
    if detected {
        headers.insert(
            header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("text/event-stream"),
        );
        if let Some(timing) = transport {
            timing.response_format = Some(crate::usage::TransportResponseFormat::Sse);
            if let Some(first_body_ms) = first_body_ms {
                timing
                    .stream_response
                    .get_or_insert_default()
                    .observe_body(true, first_body_ms);
            }
        }
    }
    let body = reqwest::Body::wrap_stream(futures_util::stream::iter(chunks).chain(stream));
    let mut restored = Response::builder()
        .status(status)
        .version(version)
        .url(url)
        .body(body)
        .expect("existing upstream response metadata is valid");
    *restored.headers_mut() = headers;
    restored.extensions_mut().extend(extensions);
    restored.into()
}

fn event_stream_prefix(bytes: &[u8]) -> Option<bool> {
    let bom = b"\xef\xbb\xbf";
    let bytes = if let Some(bytes) = bytes.strip_prefix(bom) {
        bytes
    } else if bom.starts_with(bytes) {
        return None;
    } else {
        bytes
    };
    let bytes = bytes.trim_ascii_start();
    if bytes.is_empty() {
        return None;
    }
    let mut partial = false;
    for field in [b":".as_slice(), b"data:", b"event:", b"id:", b"retry:"] {
        if bytes.starts_with(field) {
            return Some(true);
        }
        partial |= field.starts_with(bytes);
    }
    (!partial).then_some(false)
}

#[cfg(test)]
mod tests;
