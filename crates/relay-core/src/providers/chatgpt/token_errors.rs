use crate::accounts::TokenRefreshFailureKind;
use crate::error_codes;
use crate::normalize_error_code;
use serde_json::Value;

pub fn token_refresh_failure_kind(code: &str) -> TokenRefreshFailureKind {
    match code.trim().to_ascii_lowercase().as_str() {
        error_codes::INVALID_GRANT => TokenRefreshFailureKind::InvalidGrant,
        // This may be emitted after another concurrent refresh already
        // rotated the token. It is retriable, not a fresh-login condition.
        error_codes::REFRESH_TOKEN_REUSED => TokenRefreshFailureKind::Transient,
        error_codes::REFRESH_TOKEN_EXPIRED => TokenRefreshFailureKind::ExpiredRefreshToken,
        error_codes::INVALID_REFRESH_TOKEN
        | error_codes::REFRESH_TOKEN_INVALIDATED
        | error_codes::TOKEN_INVALIDATED => TokenRefreshFailureKind::InvalidatedRefreshToken,
        _ => TokenRefreshFailureKind::Transient,
    }
}

pub fn token_refresh_provider_error_code(response_body: &[u8]) -> Option<String> {
    if response_body.len() > 32 * 1_024 {
        return None;
    }
    let error_payload = crate::strict_json::parse(std::str::from_utf8(response_body).ok()?).ok()?;
    let fields = error_payload.as_object()?;
    let error = fields.get("error");
    // Presence establishes precedence. A malformed code keeps the result unknown
    // instead of borrowing a terminal grant error from a lower-priority field.
    if let Some(code) = error
        .and_then(Value::as_object)
        .and_then(|fields| fields.get("code"))
    {
        return code.as_str().and_then(normalize_error_code);
    }
    if let Some(code) = fields.get("code") {
        return code.as_str().and_then(normalize_error_code);
    }
    match error? {
        Value::String(code) => normalize_error_code(code),
        Value::Object(fields) => fields.get("type")?.as_str().and_then(normalize_error_code),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{token_refresh_failure_kind, token_refresh_provider_error_code};
    use crate::accounts::TokenRefreshFailureKind;
    use crate::normalize_error_code;

    #[test]
    fn refresh_errors_only_mark_irrecoverable_tokens_for_reauthentication() {
        assert_eq!(
            token_refresh_failure_kind("invalid_grant"),
            TokenRefreshFailureKind::InvalidGrant
        );
        assert_eq!(
            token_refresh_failure_kind("refresh_token_reused"),
            TokenRefreshFailureKind::Transient
        );
        assert_eq!(
            token_refresh_failure_kind("refresh_token_expired"),
            TokenRefreshFailureKind::ExpiredRefreshToken
        );
        assert_eq!(
            token_refresh_failure_kind("refresh_token_invalidated"),
            TokenRefreshFailureKind::InvalidatedRefreshToken
        );
        assert_eq!(
            token_refresh_failure_kind("unsupported_country_region_territory"),
            TokenRefreshFailureKind::Transient
        );
    }

    #[test]
    fn provider_error_prefers_a_specific_rotation_code() {
        assert_eq!(
            token_refresh_provider_error_code(
                br#"{"error":"invalid_grant","code":"refresh_token_reused"}"#
            )
            .as_deref(),
            Some("refresh_token_reused")
        );
        assert_eq!(
            token_refresh_provider_error_code(
                br#"{"error":{"type":"invalid_request_error","code":"refresh_token_expired"}}"#
            )
            .as_deref(),
            Some("refresh_token_expired")
        );
        assert_eq!(
            token_refresh_provider_error_code(br#"{"error":{"code":"<script>"}}"#),
            None
        );
        assert_eq!(
            normalize_error_code(" refresh_token_reused ").as_deref(),
            Some("refresh_token_reused")
        );
    }

    #[test]
    fn explicit_refresh_codes_fence_lower_priority_fields() {
        for code in serde_json::json!([null, 42, [], {}, "", "<script>"])
            .as_array()
            .unwrap()
        {
            for body in [
                serde_json::json!({"error":{"code":code,"type":"invalid_grant"}}),
                serde_json::json!({"code":code,"error":"invalid_grant"}),
                serde_json::json!({"code":"invalid_grant","error":{"code":code,"type":"invalid_grant"}}),
            ] {
                assert_eq!(
                    token_refresh_provider_error_code(&serde_json::to_vec(&body).unwrap()),
                    None,
                    "{body}"
                );
            }
        }
        assert_eq!(
            token_refresh_provider_error_code(
                br#"{"error":{"code":"provider_unknown","type":"invalid_grant"}}"#
            )
            .as_deref(),
            Some("provider_unknown")
        );
        assert_eq!(
            token_refresh_provider_error_code(br#"{"error":{"type":"invalid_grant"}}"#).as_deref(),
            Some("invalid_grant")
        );
    }

    #[test]
    fn ambiguous_refresh_envelopes_stay_unknown() {
        for body in [
            r#"{"error":"provider_unknown","error":"invalid_grant"}"#,
            r#"{"error":{"code":"provider_unknown","code":"invalid_grant"}}"#,
            r#"{"error":{"code":"provider_unknown","co\u0064e":"invalid_grant"}}"#,
            r#"{"error":{"type":"provider_unknown","type":"invalid_grant"}}"#,
            r#"{"error":"invalid_grant","metadata":{"flag":false,"flag":true}}"#,
            r#"{"error":"invalid_grant"} {}"#,
            r#"{"error":"invalid_grant","value":NaN}"#,
        ] {
            assert_eq!(
                token_refresh_provider_error_code(body.as_bytes()),
                None,
                "{body}"
            );
        }
        let mut body = br#"{"error":"invalid_grant"}"#.to_vec();
        body.resize(32 * 1_024, b' ');
        assert_eq!(
            token_refresh_provider_error_code(&body).as_deref(),
            Some("invalid_grant")
        );
        body.push(b' ');
        assert_eq!(token_refresh_provider_error_code(&body), None);
    }
}
