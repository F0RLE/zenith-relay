use crate::accounts::TokenRefreshFailureKind;
use crate::normalize_error_code;
use serde_json::Value;

pub fn token_refresh_failure_kind(code: &str) -> TokenRefreshFailureKind {
    match code.trim().to_ascii_lowercase().as_str() {
        "invalid_grant" => TokenRefreshFailureKind::InvalidGrant,
        // This may be emitted after another concurrent refresh already
        // rotated the token. It is retriable, not a fresh-login condition.
        "refresh_token_reused" => TokenRefreshFailureKind::Transient,
        "refresh_token_expired" => TokenRefreshFailureKind::ExpiredRefreshToken,
        "invalid_refresh_token" | "refresh_token_invalidated" | "token_invalidated" => {
            TokenRefreshFailureKind::InvalidatedRefreshToken
        }
        _ => TokenRefreshFailureKind::Transient,
    }
}

pub fn token_refresh_provider_error_code(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let code = [
        value.pointer("/error/code").and_then(Value::as_str),
        value.get("code").and_then(Value::as_str),
        value.get("error").and_then(Value::as_str),
        value.pointer("/error/type").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .find_map(normalize_error_code);
    code
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
}
