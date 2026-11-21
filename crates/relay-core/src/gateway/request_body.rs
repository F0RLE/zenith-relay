use super::errors::api_error;
use crate::error_codes;
use axum::body::Body;
use axum::http::{
    HeaderMap, Response, StatusCode,
    header::{CONTENT_ENCODING, CONTENT_LENGTH},
};
use futures_util::StreamExt;
use serde_json::{Map, Value};
use std::{
    io::Read,
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{Instant, timeout_at};

const MAX_INLINE_JSON_BYTES: usize = 64 * 1024;
const REQUEST_BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
// Admit large bodies before buffering them, then retain the same permit through
// decoding/parsing, including when the caller disconnects during blocking work.
static JSON_READ_JOBS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| {
    Arc::new(Semaphore::new(
        std::thread::available_parallelism().map_or(1, usize::from),
    ))
});

struct BufferedRequest {
    bytes: Vec<u8>,
    permit: Option<OwnedSemaphorePermit>,
}

async fn buffer_request(
    body: Body,
    admit_before_read: bool,
    jobs: &Arc<Semaphore>,
) -> Result<BufferedRequest, Box<Response<Body>>> {
    let acquire = || async {
        Arc::clone(jobs)
            .acquire_owned()
            .await
            .map_err(|_| Box::new(unread_body()))
    };
    let mut permit = if admit_before_read {
        Some(acquire().await?)
    } else {
        None
    };
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    let mut read_deadline = Instant::now() + REQUEST_BODY_IDLE_TIMEOUT;
    while let Some(chunk) = timeout_at(read_deadline, stream.next())
        .await
        .map_err(|_| Box::new(body_read_timeout()))?
    {
        let chunk = chunk.map_err(|_| Box::new(unread_body()))?;
        if chunk.is_empty() {
            // Empty frames are not upload progress. Check explicitly because
            // timeout_at accepts an immediately ready future after its deadline.
            if Instant::now() >= read_deadline {
                return Err(Box::new(body_read_timeout()));
            }
            tokio::task::yield_now().await;
            continue;
        }
        if permit.is_none() && bytes.len().saturating_add(chunk.len()) > MAX_INLINE_JSON_BYTES {
            // Unknown/chunked lengths use a small prefix. Do not continue
            // polling or copying the large body while its worker is occupied.
            permit = Some(acquire().await?);
        }
        bytes
            .try_reserve(chunk.len())
            .map_err(|_| Box::new(unread_body()))?;
        bytes.extend_from_slice(&chunk);
        // Waiting for our own admission is not client idleness. Start a fresh
        // read deadline after admission and actual bytes, never for parsing.
        read_deadline = Instant::now() + REQUEST_BODY_IDLE_TIMEOUT;
    }
    Ok(BufferedRequest { bytes, permit })
}

/// Upper-biased envelope accounting without serializing another copy. Include
/// parsed container allocations, repair/bridge copies and fixed request state;
/// compressed length alone would severely undercharge arrays and uploads.
pub(super) fn retained_request_bytes(request_json: &Value) -> usize {
    retained_value_bytes(request_json)
        .saturating_mul(3)
        .saturating_add(16 * 1024)
}

pub(super) fn retained_object_bytes(request_fields: &Map<String, Value>) -> usize {
    request_fields
        .iter()
        .fold(0usize, |bytes, (key, field_value)| {
            bytes
                .saturating_add(128)
                .saturating_add(key.capacity())
                .saturating_add(retained_value_bytes(field_value))
        })
}

fn retained_value_bytes(json_value: &Value) -> usize {
    let allocation = match json_value {
        Value::String(text) => text.capacity(),
        // Arbitrary-precision numbers retain their decimal text as well.
        Value::Number(number) => number.as_str().len(),
        Value::Array(items) => items.iter().fold(
            items
                .capacity()
                .saturating_mul(std::mem::size_of::<Value>()),
            |bytes, item| bytes.saturating_add(retained_value_bytes(item)),
        ),
        Value::Object(object_fields) => retained_object_bytes(object_fields),
        _ => 0,
    };
    allocation.saturating_add(std::mem::size_of::<Value>())
}

#[derive(Debug, PartialEq)]
enum ReadError {
    InvalidEncoding,
    InvalidJson,
}

fn decode(encoded_body: &[u8], encoding: &str) -> Result<Vec<u8>, ReadError> {
    let mut reader: Box<dyn Read + '_> = match encoding {
        "gzip" => Box::new(flate2::read::MultiGzDecoder::new(encoded_body)),
        "zstd" => {
            let mut decoder = zstd::stream::read::Decoder::new(encoded_body)
                .map_err(|_| ReadError::InvalidEncoding)?;
            decoder
                .window_log_max(26)
                .map_err(|_| ReadError::InvalidEncoding)?;
            Box::new(decoder)
        }
        _ => return Err(ReadError::InvalidEncoding),
    };
    let mut decoded_request_body = Vec::new();
    reader
        .read_to_end(&mut decoded_request_body)
        .map_err(|_| ReadError::InvalidEncoding)?;
    Ok(decoded_request_body)
}

pub(super) async fn read_json_object(
    headers: &HeaderMap,
    request_body: Body,
) -> Result<Map<String, Value>, Box<Response<Body>>> {
    read_json_object_inner(headers, request_body, false).await
}

pub(super) async fn read_responses_json_object(
    headers: &HeaderMap,
    request_body: Body,
) -> Result<Map<String, Value>, Box<Response<Body>>> {
    read_json_object_inner(headers, request_body, true).await
}

async fn read_json_object_inner(
    headers: &HeaderMap,
    request_body: Body,
    codex_page_context: bool,
) -> Result<Map<String, Value>, Box<Response<Body>>> {
    let encodings = headers.get_all(CONTENT_ENCODING).iter().collect::<Vec<_>>();
    let encoding = match encodings.as_slice() {
        [] => "identity".to_string(),
        [encoding_header] => encoding_header
            .to_str()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase(),
        _ => String::new(),
    };
    if !matches!(encoding.as_str(), "identity" | "gzip" | "zstd") {
        return Err(Box::new(api_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Encoding must be identity, gzip or zstd; stacked encodings are not supported",
            error_codes::REQUEST_ENCODING_UNSUPPORTED,
        )));
    }
    let compressed = encoding != "identity";
    let known_large = headers
        .get(CONTENT_LENGTH)
        .and_then(|length| length.to_str().ok())
        .and_then(|length| length.parse::<u64>().ok())
        .is_some_and(|length| length > MAX_INLINE_JSON_BYTES as u64);
    let BufferedRequest {
        bytes: encoded_request_body,
        permit,
    } = buffer_request(request_body, compressed || known_large, &JSON_READ_JOBS).await?;
    let parsed = if !compressed && encoded_request_body.len() <= MAX_INLINE_JSON_BYTES {
        parse_client_json_object(&encoded_request_body, codex_page_context)
    } else {
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if compressed {
                let decoded = decode(&encoded_request_body, &encoding)?;
                parse_client_json_object(&decoded, codex_page_context)
            } else {
                parse_client_json_object(&encoded_request_body, codex_page_context)
            }
        })
        .await
        .map_err(|_| {
            Box::new(if compressed {
                invalid_encoding()
            } else {
                unread_body()
            })
        })?
    };
    parsed.map_err(|error| {
        Box::new(match error {
            ReadError::InvalidEncoding => invalid_encoding(),
            ReadError::InvalidJson => api_error(
                StatusCode::BAD_REQUEST,
                "request body must be a JSON object",
                error_codes::INVALID_REQUEST,
            ),
        })
    })
}

fn parse_client_json_object(
    request_bytes: &[u8],
    codex_page_context: bool,
) -> Result<Map<String, Value>, ReadError> {
    let mut fields = parse_json_object(request_bytes)?;
    if codex_page_context {
        super::request::normalize_codex_page_context(&mut fields, request_bytes);
    }
    Ok(fields)
}

fn parse_json_object(request_bytes: &[u8]) -> Result<Map<String, Value>, ReadError> {
    match serde_json::from_slice(request_bytes) {
        Ok(Value::Object(request_fields)) => Ok(request_fields),
        _ => Err(ReadError::InvalidJson),
    }
}

fn unread_body() -> Response<Body> {
    api_error(
        StatusCode::BAD_REQUEST,
        "request body could not be read",
        error_codes::INVALID_REQUEST,
    )
}

fn body_read_timeout() -> Response<Body> {
    api_error(
        StatusCode::REQUEST_TIMEOUT,
        "request body upload timed out waiting for data",
        error_codes::REQUEST_TIMEOUT,
    )
}

fn invalid_encoding() -> Response<Body> {
    api_error(
        StatusCode::BAD_REQUEST,
        "compressed request body is corrupt, incomplete or exceeds the decoder window limit",
        error_codes::REQUEST_ENCODING_INVALID,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test(start_paused = true)]
    async fn stalled_upload_releases_slot_for_next_large_body() {
        use std::{future::Future, task::Poll};

        for (admit_before_read, prefix_len) in [
            (true, None),
            (true, Some(MAX_INLINE_JSON_BYTES + 1)),
            (false, Some(MAX_INLINE_JSON_BYTES + 1)),
        ] {
            let jobs = Arc::new(Semaphore::new(1));
            let prefix = prefix_len.map(|len| axum::body::Bytes::from(vec![b'x'; len]));
            let stream =
                futures_util::stream::iter(prefix.into_iter().map(Ok::<_, std::io::Error>))
                    .chain(futures_util::stream::pending());
            let mut stalled = Box::pin(buffer_request(
                Body::from_stream(stream),
                admit_before_read,
                &jobs,
            ));
            assert!(
                std::future::poll_fn(|cx| Poll::Ready(stalled.as_mut().poll(cx)))
                    .await
                    .is_pending()
            );
            assert_eq!(jobs.available_permits(), 0);
            let mut next = Box::pin(buffer_request(Body::from("{}"), true, &jobs));
            assert!(
                std::future::poll_fn(|cx| Poll::Ready(next.as_mut().poll(cx)))
                    .await
                    .is_pending()
            );

            tokio::time::advance(REQUEST_BODY_IDLE_TIMEOUT).await;
            let error = stalled.await.err().unwrap();
            assert_eq!(error.status(), StatusCode::REQUEST_TIMEOUT);
            let body = next.await.unwrap();
            assert_eq!(body.bytes, b"{}");
            assert_eq!(jobs.available_permits(), 0);
            drop(body);
            assert_eq!(jobs.available_permits(), 1);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn upload_progress_refreshes_idle_deadline_without_total_upload_limit() {
        for admit_before_read in [false, true] {
            let jobs = Arc::new(Semaphore::new(1));
            let stream = futures_util::stream::unfold(0, |index| async move {
                if index == 3 {
                    return None;
                }
                tokio::time::sleep(REQUEST_BODY_IDLE_TIMEOUT * 3 / 4).await;
                Some((
                    Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"x")),
                    index + 1,
                ))
            });
            let started = Instant::now();
            let body = buffer_request(Body::from_stream(stream), admit_before_read, &jobs)
                .await
                .unwrap();
            assert_eq!(body.bytes, b"xxx");
            assert!(started.elapsed() > REQUEST_BODY_IDLE_TIMEOUT * 2);
            drop(body);
            assert_eq!(jobs.available_permits(), 1);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn empty_chunks_do_not_refresh_upload_idle_deadline() {
        for admit_before_read in [false, true] {
            let jobs = Arc::new(Semaphore::new(1));
            let stream = futures_util::stream::unfold((), |()| async {
                tokio::time::sleep(REQUEST_BODY_IDLE_TIMEOUT / 4).await;
                Some((Ok::<_, std::io::Error>(axum::body::Bytes::new()), ()))
            });
            let started = Instant::now();
            let error = buffer_request(Body::from_stream(stream), admit_before_read, &jobs)
                .await
                .err()
                .unwrap();
            assert_eq!(error.status(), StatusCode::REQUEST_TIMEOUT);
            assert_eq!(started.elapsed(), REQUEST_BODY_IDLE_TIMEOUT);
            assert_eq!(jobs.available_permits(), 1);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn admission_wait_is_not_charged_to_upload_idle_deadline() {
        use std::{future::Future, task::Poll};

        for admit_before_read in [false, true] {
            let jobs = Arc::new(Semaphore::new(1));
            let occupied = Arc::clone(&jobs).acquire_owned().await.unwrap();
            let chunks = [
                axum::body::Bytes::from(vec![b'x'; MAX_INLINE_JSON_BYTES + 1]),
                axum::body::Bytes::from_static(b"z"),
            ];
            let stream =
                futures_util::stream::iter(chunks.into_iter().map(Ok::<_, std::io::Error>));
            let mut read = Box::pin(buffer_request(
                Body::from_stream(stream),
                admit_before_read,
                &jobs,
            ));
            assert!(
                std::future::poll_fn(|cx| Poll::Ready(read.as_mut().poll(cx)))
                    .await
                    .is_pending()
            );
            tokio::time::advance(REQUEST_BODY_IDLE_TIMEOUT * 2).await;
            drop(occupied);
            let body = read.await.unwrap();
            assert_eq!(body.bytes.len(), MAX_INLINE_JSON_BYTES + 2);
            assert_eq!(body.bytes.last(), Some(&b'z'));
            drop(body);
            assert_eq!(jobs.available_permits(), 1);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_json_upload_reports_request_timeout_for_each_encoding() {
        for encoding in ["identity", "gzip", "zstd"] {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_ENCODING, encoding.parse().unwrap());
            let body = Body::from_stream(futures_util::stream::pending::<
                Result<axum::body::Bytes, std::io::Error>,
            >());
            let started = Instant::now();
            let error = read_json_object(&headers, body).await.unwrap_err();
            assert_eq!(error.status(), StatusCode::REQUEST_TIMEOUT);
            assert_eq!(started.elapsed(), REQUEST_BODY_IDLE_TIMEOUT);
            let body = axum::body::to_bytes(error.into_body(), 4096).await.unwrap();
            let error: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(error["error"]["code"], error_codes::REQUEST_TIMEOUT);
        }
    }

    #[tokio::test]
    async fn cancelled_admitted_upload_releases_its_slot() {
        use std::{future::Future, task::Poll};

        let jobs = Arc::new(Semaphore::new(1));
        let body = Body::from_stream(futures_util::stream::pending::<
            Result<axum::body::Bytes, std::io::Error>,
        >());
        let mut read = Box::pin(buffer_request(body, true, &jobs));
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(read.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        assert_eq!(jobs.available_permits(), 0);
        drop(read);
        assert_eq!(jobs.available_permits(), 1);
    }

    #[tokio::test]
    async fn known_large_and_compressed_bodies_wait_before_polling_and_release_on_cancel() {
        use std::{
            future::Future,
            pin::pin,
            sync::atomic::{AtomicUsize, Ordering},
            task::Poll,
        };
        let jobs = Arc::new(Semaphore::new(1));
        let occupied = Arc::clone(&jobs).acquire_owned().await.unwrap();
        let polls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&polls);
        let stream = futures_util::stream::poll_fn(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Some(Ok::<_, std::io::Error>(
                axum::body::Bytes::from_static(b"{}"),
            )))
        });
        let mut read = Box::pin(buffer_request(Body::from_stream(stream), true, &jobs));
        let pending = std::future::poll_fn(|cx| Poll::Ready(read.as_mut().poll(cx))).await;
        assert!(pending.is_pending());
        assert_eq!(polls.load(Ordering::SeqCst), 0);

        let mut small = pin!(buffer_request(Body::from("{}"), false, &jobs));
        let ready = std::future::poll_fn(|cx| Poll::Ready(small.as_mut().poll(cx))).await;
        assert!(
            matches!(ready, Poll::Ready(Ok(body)) if body.bytes == b"{}" && body.permit.is_none())
        );

        drop(read);
        drop(occupied);
        assert_eq!(jobs.available_permits(), 1);
        let body = buffer_request(Body::from("{}"), true, &jobs).await.unwrap();
        assert_eq!(jobs.available_permits(), 0);
        drop(body);
        assert_eq!(jobs.available_permits(), 1);
    }

    #[tokio::test]
    async fn chunked_bodies_stop_reading_at_the_inline_boundary_until_admitted() {
        use std::{
            future::Future,
            sync::atomic::{AtomicUsize, Ordering},
            task::Poll,
        };
        let jobs = Arc::new(Semaphore::new(1));
        let occupied = Arc::clone(&jobs).acquire_owned().await.unwrap();
        let polls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&polls);
        let chunks = [
            axum::body::Bytes::from(vec![b'x'; MAX_INLINE_JSON_BYTES]),
            axum::body::Bytes::from_static(b"y"),
            axum::body::Bytes::from_static(b"z"),
        ];
        let stream = futures_util::stream::iter(chunks.into_iter().map(Ok::<_, std::io::Error>))
            .inspect(move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
            });
        let mut read = Box::pin(buffer_request(Body::from_stream(stream), false, &jobs));
        let pending = std::future::poll_fn(|cx| Poll::Ready(read.as_mut().poll(cx))).await;
        assert!(pending.is_pending());
        assert_eq!(polls.load(Ordering::SeqCst), 2);
        drop(occupied);
        let body = read.await.unwrap();
        assert_eq!(polls.load(Ordering::SeqCst), 3);
        assert_eq!(body.bytes.len(), MAX_INLINE_JSON_BYTES + 2);
        assert_eq!(&body.bytes[MAX_INLINE_JSON_BYTES..], b"yz");
        assert_eq!(jobs.available_permits(), 0);
        drop(body);
        assert_eq!(jobs.available_permits(), 1);
    }

    #[tokio::test]
    async fn failed_body_read_releases_its_admission_permit() {
        let jobs = Arc::new(Semaphore::new(1));
        let stream = futures_util::stream::iter([
            Ok(axum::body::Bytes::from_static(b"{")),
            Err(std::io::Error::other("synthetic-private-body-error")),
        ]);
        let error = buffer_request(Body::from_stream(stream), true, &jobs)
            .await
            .err()
            .unwrap();
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(jobs.available_permits(), 1);
        let body = axum::body::to_bytes(error.into_body(), 4096).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("synthetic-private"));
    }

    #[tokio::test]
    async fn page_context_normalization_is_responses_only_for_every_body_encoding() {
        let context = serde_json::json!({
            "type":"function_call_output", "namespace":"agent",
            "name":"codex_apps_open_page", "output":r#"{"page_id":null}"#
        });
        for large in [false, true] {
            let raw = serde_json::json!({"model":"synthetic", "input":[context],
                "extension":if large { "x".repeat(MAX_INLINE_JSON_BYTES) } else { String::new() }
            })
            .to_string();
            for encoding in ["identity", "gzip", "zstd"] {
                let bytes = match encoding {
                    "gzip" => {
                        let mut encoder =
                            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                        encoder.write_all(raw.as_bytes()).unwrap();
                        encoder.finish().unwrap()
                    }
                    "zstd" => zstd::stream::encode_all(raw.as_bytes(), 1).unwrap(),
                    _ => raw.as_bytes().to_vec(),
                };
                let mut headers = HeaderMap::new();
                headers.insert(CONTENT_ENCODING, encoding.parse().unwrap());
                let ordinary = read_json_object(&headers, Body::from(bytes.clone()))
                    .await
                    .unwrap();
                assert_eq!(ordinary["input"][0], context);
                let responses = read_responses_json_object(&headers, Body::from(bytes))
                    .await
                    .unwrap();
                assert_eq!(responses["input"][0]["role"], "user");
                assert_eq!(
                    responses["input"][0]["content"][0]["text"],
                    context["output"]
                );
                assert_eq!(responses["extension"], ordinary["extension"]);
            }
        }
    }

    #[tokio::test]
    async fn native_json_roundtrip_preserves_order_and_exact_numbers() {
        use crate::{AdapterRequestContext, MessagesReasoningMode, SourceAdapter, WireApi};

        let body = r#"{"model":"alias","tools":[{"type":"function","name":"lookup","parameters":{"type":"object","properties":{"zeta":{"type":"number"},"alpha":{"type":"integer"}}}}],"extension":{"z":123456789012345678901234567890123456789,"a":0.12345678901234567890123456789,"exponent":1e+30}}"#;
        let fields = read_json_object(&HeaderMap::new(), Body::from(body))
            .await
            .unwrap();
        let request = Value::Object(fields);
        for client_wire_api in [
            WireApi::Responses,
            WireApi::ChatCompletions,
            WireApi::Messages,
            WireApi::Gemini,
        ] {
            let prepared = SourceAdapter::Native
                .prepare_request(AdapterRequestContext {
                    client_wire_api,
                    request: &request,
                    model: "resolved-model",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
                    previous: None,
                    response_scope: "synthetic-native-route",
                    response_id_seed: "synthetic-request",
                })
                .unwrap();
            let expected = if client_wire_api == WireApi::Gemini {
                body.replacen("\"model\":\"alias\",", "", 1)
            } else {
                body.replacen("\"alias\"", "\"resolved-model\"", 1)
            };
            assert_eq!(
                serde_json::to_string(prepared.upstream_body()).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn large_json_waits_for_a_worker_without_blocking_small_requests() {
        use std::{future::Future, pin::pin, sync::mpsc, task::Poll};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_time()
            .build()
            .unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = runtime.spawn_blocking(move || {
            started_tx.send(()).unwrap();
            let _ = release_rx.recv();
        });
        started_rx.recv().unwrap();
        runtime.block_on(async {
            let headers = HeaderMap::new();
            let large = format!("{{\"input\":\"{}\"}}", "x".repeat(MAX_INLINE_JSON_BYTES));
            let mut large_read = pin!(read_json_object(&headers, Body::from(large)));
            let pending_large =
                std::future::poll_fn(|context| Poll::Ready(large_read.as_mut().poll(context)))
                    .await;
            let mut small_read = pin!(read_json_object(
                &headers,
                Body::from(r#"{"input":"small"}"#)
            ));
            let ready_small =
                std::future::poll_fn(|context| Poll::Ready(small_read.as_mut().poll(context)))
                    .await;
            // Always release the occupied worker before asserting, so a failure
            // cannot strand runtime shutdown behind this synthetic job.
            release_tx.send(()).unwrap();
            assert!(pending_large.is_pending());
            assert!(matches!(ready_small, Poll::Ready(Ok(fields)) if fields["input"] == "small"));
            let fields = large_read.await.unwrap();
            assert_eq!(
                fields["input"].as_str().unwrap().len(),
                MAX_INLINE_JSON_BYTES
            );
            worker.await.unwrap();
        });
    }

    #[tokio::test]
    async fn encoding_paths_preserve_json_and_distinguish_parse_errors() {
        let json = r#"{"z":123456789012345678901234567890,"a":"Unicode: привет","fraction":0.12345678901234567890123456789}"#;
        for encoding in ["identity", "gzip", "zstd"] {
            for padding in [0, MAX_INLINE_JSON_BYTES] {
                for (raw, valid) in [(json, true), ("[]", false), ("{invalid", false)] {
                    let input = format!("{}{raw}", " ".repeat(padding));
                    let body = match encoding {
                        "gzip" => {
                            let mut gzip = flate2::write::GzEncoder::new(
                                Vec::new(),
                                flate2::Compression::fast(),
                            );
                            gzip.write_all(input.as_bytes()).unwrap();
                            gzip.finish().unwrap()
                        }
                        "zstd" => zstd::stream::encode_all(input.as_bytes(), 1).unwrap(),
                        _ => input.into_bytes(),
                    };
                    let mut headers = HeaderMap::new();
                    headers.insert(CONTENT_ENCODING, encoding.parse().unwrap());
                    let result = read_json_object(&headers, Body::from(body)).await;
                    if valid {
                        assert_eq!(serde_json::to_string(&result.unwrap()).unwrap(), json);
                    } else {
                        let response = result.unwrap_err();
                        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
                        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                            .await
                            .unwrap();
                        let error: Value = serde_json::from_slice(&bytes).unwrap();
                        assert_eq!(error["error"]["code"], error_codes::INVALID_REQUEST);
                    }
                }
            }
        }
    }

    #[test]
    fn retained_bytes_include_arbitrary_precision_number_storage() {
        let digits = "7".repeat(4096);
        let number: Value = serde_json::from_str(&digits).unwrap();
        assert!(retained_request_bytes(&number) >= 3 * digits.len() + 16 * 1024);
    }

    #[test]
    fn truncated_compressed_streams_are_rejected() {
        let input = br#"{"model":"synthetic","input":"hello"}"#;
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(input).unwrap();
        for (encoding, bytes) in [
            ("gzip", gzip.finish().unwrap()),
            (
                "zstd",
                zstd::stream::encode_all(input.as_slice(), 1).unwrap(),
            ),
        ] {
            assert_eq!(decode(&bytes, encoding).unwrap(), input);
            assert_eq!(
                decode(&bytes[..bytes.len() - 2], encoding),
                Err(ReadError::InvalidEncoding)
            );
        }
    }

    #[tokio::test]
    async fn rejects_stacked_encoding_before_parsing() {
        let mut headers = HeaderMap::new();
        headers.append(CONTENT_ENCODING, "gzip".parse().unwrap());
        headers.append(CONTENT_ENCODING, "zstd".parse().unwrap());
        assert_eq!(
            read_json_object(&headers, Body::empty())
                .await
                .unwrap_err()
                .status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }
}
