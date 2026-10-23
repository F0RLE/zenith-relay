use super::errors::api_error;
use crate::error_codes;
use axum::body::Body;
use axum::http::{header::CONTENT_ENCODING, HeaderMap, Response, StatusCode};
use serde_json::{Map, Value};
use std::io::Read;

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
    let request_bytes = if encoding == "identity" {
        encoded_request_body
    } else {
        tokio::task::spawn_blocking(move || decode(&encoded_request_body, &encoding))
            .await
            .map_err(|_| Box::new(invalid_encoding()))?
            .map_err(|_| Box::new(invalid_encoding()))?
            .into()
    };
    match serde_json::from_slice(&request_bytes) {
        Ok(Value::Object(request_fields)) => Ok(request_fields),
        _ => Err(Box::new(api_error(
            StatusCode::BAD_REQUEST,
            "request body must be a JSON object",
            error_codes::INVALID_REQUEST,
        ))),
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
