use super::errors::api_error;
use crate::error_codes;
use axum::body::Body;
use axum::http::{header::CONTENT_ENCODING, HeaderMap, Response, StatusCode};
use serde_json::{Map, Value};
use std::{io::Read, sync::LazyLock};

const MAX_INLINE_JSON_BYTES: usize = 64 * 1024;
// Keep CPU-heavy decoding off network workers without filling Tokio's blocking
// pool with one runnable parser per incoming request. The permit follows the
// worker even when the caller disconnects.
static JSON_READ_JOBS: LazyLock<tokio::sync::Semaphore> = LazyLock::new(|| {
    tokio::sync::Semaphore::new(std::thread::available_parallelism().map_or(1, usize::from))
});

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
    let encoded_request_body = axum::body::to_bytes(request_body, usize::MAX)
        .await
        .map_err(|_| Box::new(unread_body()))?;
    let compressed = encoding != "identity";
    let parsed = if !compressed && encoded_request_body.len() <= MAX_INLINE_JSON_BYTES {
        parse_json_object(&encoded_request_body)
    } else {
        let permit = JSON_READ_JOBS
            .acquire()
            .await
            .map_err(|_| Box::new(unread_body()))?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if compressed {
                let decoded = decode(&encoded_request_body, &encoding)?;
                parse_json_object(&decoded)
            } else {
                parse_json_object(&encoded_request_body)
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
