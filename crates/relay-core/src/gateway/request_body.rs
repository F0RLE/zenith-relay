use super::errors::api_error;
use crate::error_codes;
use axum::body::Body;
use axum::http::{header::CONTENT_ENCODING, HeaderMap, Response, StatusCode};
use serde_json::{Map, Value};
use std::io::Read;

/// Upper-biased envelope accounting without serializing another copy. Include
/// parsed container allocations, repair/bridge copies and fixed request state;
/// compressed length alone would severely undercharge arrays and uploads.
pub(super) fn retained_request_bytes(value: &Value) -> usize {
    retained_value_bytes(value)
        .saturating_mul(3)
        .saturating_add(16 * 1024)
}

pub(super) fn retained_object_bytes(object: &Map<String, Value>) -> usize {
    object.iter().fold(0usize, |bytes, (key, value)| {
        bytes
            .saturating_add(128)
            .saturating_add(key.capacity())
            .saturating_add(retained_value_bytes(value))
    })
}

fn retained_value_bytes(value: &Value) -> usize {
    let allocation = match value {
        Value::String(value) => value.capacity(),
        Value::Array(values) => values.iter().fold(
            values
                .capacity()
                .saturating_mul(std::mem::size_of::<Value>()),
            |bytes, value| bytes.saturating_add(retained_value_bytes(value)),
        ),
        Value::Object(object) => retained_object_bytes(object),
        _ => 0,
    };
    allocation.saturating_add(std::mem::size_of::<Value>())
}

#[derive(Debug, PartialEq)]
enum ReadError {
    InvalidEncoding,
}

fn decode(bytes: &[u8], encoding: &str) -> Result<Vec<u8>, ReadError> {
    let mut reader: Box<dyn Read + '_> = match encoding {
        "gzip" => Box::new(flate2::read::MultiGzDecoder::new(bytes)),
        "zstd" => {
            let mut decoder =
                zstd::stream::read::Decoder::new(bytes).map_err(|_| ReadError::InvalidEncoding)?;
            decoder
                .window_log_max(26)
                .map_err(|_| ReadError::InvalidEncoding)?;
            Box::new(decoder)
        }
        _ => return Err(ReadError::InvalidEncoding),
    };
    let mut output = Vec::new();
    reader
        .read_to_end(&mut output)
        .map_err(|_| ReadError::InvalidEncoding)?;
    Ok(output)
}

pub(super) async fn read_json_object(
    headers: &HeaderMap,
    body: Body,
) -> Result<Map<String, Value>, Box<Response<Body>>> {
    let encodings = headers.get_all(CONTENT_ENCODING).iter().collect::<Vec<_>>();
    let encoding = match encodings.as_slice() {
        [] => "identity".to_string(),
        [value] => value.to_str().unwrap_or("").trim().to_ascii_lowercase(),
        _ => String::new(),
    };
    if !matches!(encoding.as_str(), "identity" | "gzip" | "zstd") {
        return Err(Box::new(api_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Encoding must be identity, gzip or zstd; stacked encodings are not supported",
            error_codes::REQUEST_ENCODING_UNSUPPORTED,
        )));
    }
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|_| Box::new(unread_body()))?;
    let bytes = if encoding == "identity" {
        bytes
    } else {
        tokio::task::spawn_blocking(move || decode(&bytes, &encoding))
            .await
            .map_err(|_| Box::new(invalid_encoding()))?
            .map_err(|_| Box::new(invalid_encoding()))?
            .into()
    };
    match serde_json::from_slice(&bytes) {
        Ok(Value::Object(object)) => Ok(object),
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
