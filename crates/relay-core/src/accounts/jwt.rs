use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;

const MAX_UNVERIFIED_JWT_BYTES: usize = 64 * 1024;
const MAX_UNVERIFIED_JWT_PAYLOAD_BYTES: usize = 16 * 1024;

/// Decodes a bounded JWT payload without verifying its signature.
///
/// Use the result only as non-authoritative metadata. Authentication and
/// authorization must always be performed by the upstream provider.
pub fn decode_unverified_jwt_payload<T: for<'de> Deserialize<'de>>(token: &str) -> Option<T> {
    if token.is_empty() || token.len() > MAX_UNVERIFIED_JWT_BYTES {
        return None;
    }
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() {
        return None;
    }
    let payload = URL_SAFE_NO_PAD.decode(payload).ok()?;
    if payload.len() > MAX_UNVERIFIED_JWT_PAYLOAD_BYTES {
        return None;
    }
    serde_json::from_slice(&payload).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn token(payload: Value) -> String {
        format!(
            "{}.{}.signature",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
        )
    }

    #[test]
    fn unverified_jwt_decoder_requires_a_bounded_three_part_payload() {
        assert_eq!(
            decode_unverified_jwt_payload::<Value>(&token(json!({"sub":"account"}))),
            Some(json!({"sub":"account"}))
        );
        assert!(decode_unverified_jwt_payload::<Value>("header.payload").is_none());
        assert!(decode_unverified_jwt_payload::<Value>("header..signature").is_none());
        assert!(
            decode_unverified_jwt_payload::<Value>(&"x".repeat(MAX_UNVERIFIED_JWT_BYTES + 1))
                .is_none()
        );
        assert!(decode_unverified_jwt_payload::<Value>(&token(json!({
            "padding": "x".repeat(MAX_UNVERIFIED_JWT_PAYLOAD_BYTES + 1)
        })))
        .is_none());
    }
}
