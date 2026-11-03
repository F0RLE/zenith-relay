//! Encode only after ownership guards. Compression never creates a retry path.
use crate::usage::{RequestContentEncoding, RequestPayloadDiagnostics, TransportMode};
use reqwest::{Method, Request, header};
use std::time::Instant;

const MIN_COMPRESS_BYTES: usize = 64 * 1024;
const MAX_COMPRESS_BYTES: usize = 8 * 1024 * 1024;
// Avoid an unbounded blocking queue under parallel load.
// A busy encoder keeps the original identity request; it never waits to send.
static COMPRESSION_JOBS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

fn supported(request: &Request, native_codex_account: bool) -> bool {
    // The official Codex client opts into Zstd only for the Codex backend.
    // Do not infer support from Responses compatibility, a model name, or a proxy.
    // BPS parses gzip/Zstd bytes as JSON and rejects them; see BASIS_POINTS.md.
    native_codex_account
        && request.method() == Method::POST
        && request.url().scheme() == "https"
        && request.url().host_str() == Some("chatgpt.com")
        && request.url().port_or_known_default() == Some(443)
        && matches!(
            request.url().path().trim_end_matches('/'),
            "/backend-api/codex/responses" | "/backend-api/codex/v1/responses"
        )
        && !request.headers().contains_key(header::CONTENT_ENCODING)
}

pub(super) async fn encode(
    mut request: Request,
    native_codex_account: bool,
    diagnostics_mode: Option<TransportMode>,
) -> (Request, Option<RequestPayloadDiagnostics>) {
    let json = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        });
    // An existing encoding is opaque: do not call encoded bytes JSON or identity.
    let body = request
        .body()
        .and_then(|body| body.as_bytes())
        .filter(|_| json && !request.headers().contains_key(header::CONTENT_ENCODING));
    let mut observed = body
        .zip(diagnostics_mode)
        .map(|(body, mode)| RequestPayloadDiagnostics::identity(body, mode));
    let compress = supported(&request, native_codex_account)
        && body.is_some_and(|body| (MIN_COMPRESS_BYTES..=MAX_COMPRESS_BYTES).contains(&body.len()));
    if !compress {
        return (request, observed);
    }
    let Ok(permit) = COMPRESSION_JOBS.try_acquire() else {
        return (request, observed);
    };
    let started = Instant::now();
    // Request::try_clone shares reusable body bytes; Body itself has no public
    // clone method. Drop the temporary headers immediately, before the CPU job.
    // Retain the original unsent request even if the blocking task fails.
    let Some(compression_body) = shared_body(&request) else {
        return (request, observed);
    };
    // Cancelling a request can finish bounded local compression, never a network send.
    let encoded = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        zstd::stream::encode_all(
            compression_body
                .as_bytes()
                .expect("only reusable bodies enter compression"),
            1,
        )
    })
    .await;
    if let Some(observed) = observed.as_mut() {
        let elapsed = started.elapsed();
        observed.compression_ms = Some(elapsed.as_millis() as u64);
        observed.compression_us = Some(elapsed.as_micros() as u64);
    }
    if let Ok(Ok(encoded)) = encoded
        && encoded.len()
            < request
                .body()
                .and_then(|body| body.as_bytes())
                .map_or(0, <[u8]>::len)
    {
        if let Some(observed) = observed.as_mut() {
            observed.body_bytes = encoded.len() as u64;
            observed.content_encoding = RequestContentEncoding::Zstd;
        }
        *request.body_mut() = Some(encoded.into());
        request.headers_mut().insert(
            header::CONTENT_ENCODING,
            header::HeaderValue::from_static("zstd"),
        );
        request.headers_mut().remove(header::CONTENT_LENGTH);
    }
    (request, observed)
}

fn shared_body(request: &Request) -> Option<reqwest::Body> {
    request.try_clone()?.body_mut().take()
}

#[cfg(test)]
mod benchmark_tests;

#[cfg(test)]
mod tests {
    use super::*;
    fn request(url: &str, text: &str) -> Request {
        reqwest::Client::new()
            .post(url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(serde_json::json!({"input":text}).to_string())
            .build()
            .unwrap()
    }
    #[tokio::test]
    async fn known_native_backend_compresses_losslessly_after_payload_preparation() {
        let first_request = request(
            "https://chatgpt.com/backend-api/codex/responses",
            &"synthetic".repeat(10_000),
        );
        let original = first_request.body().unwrap().as_bytes().unwrap().to_vec();
        let (encoded, observed) = encode(first_request, true, Some(TransportMode::Native)).await;
        let observed = observed.unwrap();
        assert_eq!(encoded.headers()[header::CONTENT_ENCODING], "zstd");
        let body = encoded.body().unwrap().as_bytes().unwrap();
        assert_eq!(zstd::stream::decode_all(body).unwrap(), original);
        assert_eq!(observed.json_bytes, original.len() as u64);
        assert_eq!(observed.body_bytes, body.len() as u64);
        assert!(observed.body_bytes < observed.json_bytes);
        assert!(observed.compression_ms.is_some());
        assert!(observed.compression_us.is_some());
        assert_eq!(
            observed.compression_ms,
            observed.compression_us.map(|micros| micros / 1_000)
        );
        // Saturation does not enqueue another compression job or block dispatch.
        let _busy = COMPRESSION_JOBS.acquire_many(2).await.unwrap();
        let request = request(
            "https://chatgpt.com/backend-api/codex/responses",
            &"synthetic".repeat(10_000),
        );
        let (encoded, observed) = encode(request, true, Some(TransportMode::Native)).await;
        let observed = observed.unwrap();
        assert!(!encoded.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(observed.body_bytes, observed.json_bytes);
        assert_eq!(observed.compression_ms, None);
        assert_eq!(observed.compression_us, None);
    }
    #[tokio::test]
    async fn other_origins_endpoints_and_non_account_routes_never_compress() {
        for (url, account) in [
            ("https://chatgpt.com/backend-api/codex/responses", false),
            ("https://example.test/backend-api/codex/responses", true),
            (
                "https://chatgpt.com/backend-api/codex/responses/compact",
                true,
            ),
            ("https://chatgpt.com:8443/backend-api/codex/responses", true),
            ("http://chatgpt.com/backend-api/codex/responses", true),
            ("https://api.openai.com/v1/responses", true),
        ] {
            let request = request(url, &"synthetic".repeat(10_000));
            let (encoded, observed) = encode(request, account, Some(TransportMode::Native)).await;
            assert!(!encoded.headers().contains_key(header::CONTENT_ENCODING));
            assert_eq!(
                observed.unwrap().content_encoding,
                RequestContentEncoding::Identity
            );
        }
    }
    #[tokio::test]
    async fn basis_points_requests_keep_json_until_upstream_supports_request_decoding() {
        // Identity JSON reaches BPS body validation, but gzip/Zstd fail UTF-8
        // decoding. Even a misclassified account must not enable Codex compression
        // on this different endpoint.
        for native_codex_account in [false, true] {
            for url in [
                crate::providers::chatgpt::BASIS_POINTS_RESPONSES_URL,
                "https://bps.openai.com/basispoints/api/responses/compact",
                "https://bps.openai.com/basispoints/api/responses/access",
            ] {
                let mut request = request(url, &"synthetic".repeat(10_000));
                let original = request.body().unwrap().as_bytes().unwrap().to_vec();
                request.headers_mut().insert(
                    header::CONTENT_LENGTH,
                    header::HeaderValue::from_str(&original.len().to_string()).unwrap(),
                );
                let (encoded, observed) =
                    encode(request, native_codex_account, Some(TransportMode::Native)).await;
                let observed = observed.unwrap();
                assert!(!encoded.headers().contains_key(header::CONTENT_ENCODING));
                assert_eq!(encoded.body().unwrap().as_bytes().unwrap(), original);
                assert_eq!(
                    encoded.headers()[header::CONTENT_LENGTH].to_str().unwrap(),
                    original.len().to_string()
                );
                assert_eq!(observed.content_encoding, RequestContentEncoding::Identity);
                assert_eq!(observed.body_bytes, original.len() as u64);
                assert_eq!(observed.json_bytes, observed.body_bytes);
                assert_eq!(observed.compression_ms, None);
                assert_eq!(observed.compression_us, None);
            }
        }
    }

    #[tokio::test]
    async fn small_requests_and_debug_off_keep_their_contract() {
        let (encoded, observed) = encode(
            request("https://chatgpt.com/backend-api/codex/responses", "small"),
            true,
            None,
        )
        .await;
        assert!(observed.is_none());
        assert!(!encoded.headers().contains_key(header::CONTENT_ENCODING));
    }
    #[tokio::test]
    async fn already_encoded_bodies_are_opaque_and_not_reencoded() {
        let mut request = request(
            "https://chatgpt.com/backend-api/codex/responses",
            "synthetic",
        );
        request.headers_mut().insert(
            header::CONTENT_ENCODING,
            header::HeaderValue::from_static("gzip"),
        );
        let original = request.body().unwrap().as_bytes().unwrap().to_vec();
        let (encoded, observed) = encode(request, true, Some(TransportMode::Native)).await;
        assert_eq!(encoded.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(encoded.body().unwrap().as_bytes().unwrap(), original);
        assert!(observed.is_none());
    }

    #[test]
    fn encoding_shares_input_storage_and_keeps_the_original_request() {
        let request = request(
            "https://chatgpt.com/backend-api/codex/responses",
            &"synthetic".repeat(10_000),
        );
        let shared = shared_body(&request).unwrap();
        let original = request.body().unwrap().as_bytes().unwrap();
        assert_eq!(shared.as_bytes().unwrap().as_ptr(), original.as_ptr());
        assert_eq!(shared.as_bytes().unwrap().len(), original.len());
        drop(shared);
        assert_eq!(request.body().unwrap().as_bytes().unwrap(), original);
    }

    #[tokio::test]
    async fn oversized_and_streaming_bodies_remain_unencoded() {
        let oversized = request(
            "https://chatgpt.com/backend-api/codex/responses",
            &"x".repeat(MAX_COMPRESS_BYTES + 1),
        );
        let (oversized, observed) = encode(oversized, true, Some(TransportMode::Native)).await;
        assert!(!oversized.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(observed.unwrap().compression_us, None);

        let mut streaming = request("https://chatgpt.com/backend-api/codex/responses", "");
        *streaming.body_mut() = Some(reqwest::Body::wrap_stream(futures_util::stream::once(
            async { Ok::<_, std::io::Error>("synthetic") },
        )));
        assert!(shared_body(&streaming).is_none());
        let (streaming, observed) = encode(streaming, true, Some(TransportMode::Native)).await;
        assert!(!streaming.headers().contains_key(header::CONTENT_ENCODING));
        assert!(observed.is_none());
        assert!(streaming.body().unwrap().as_bytes().is_none());
    }
}
