use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;

const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_MESSAGE_CHARS: usize = 2_048;

/// A bounded error envelope, never the provider's response body or headers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", from = "Value")]
pub struct UpstreamErrorDetails {
    pub http_status: Option<u16>,
    pub code: Option<String>,
    pub error_type: Option<String>,
    pub message: Option<String>,
    pub redacted: bool,
    pub truncated: bool,
}

impl UpstreamErrorDetails {
    pub fn from_body(http_status: Option<u16>, body: &[u8]) -> Self {
        if body.len() > MAX_BODY_BYTES {
            return Self {
                truncated: true,
                ..Self::empty(http_status)
            };
        }
        match serde_json::from_slice::<Value>(body) {
            Ok(value) => Self::from_value(http_status, &value),
            Err(_) => {
                let mut details = Self::empty(http_status);
                let text = String::from_utf8_lossy(body);
                if text.trim_start().starts_with(['<', '{', '['])
                    || text
                        .lines()
                        .any(|line| line.starts_with("data:") || line.starts_with("event:"))
                {
                    details.redacted = true;
                } else {
                    details.set_message(&text);
                }
                details
            }
        }
    }

    pub fn from_value(http_status: Option<u16>, value: &Value) -> Self {
        let mut details = Self::empty(http_status);
        let envelope = [
            "/error",
            "/response/error",
            "/body/error",
            "/detail",
            "/body",
            "/response",
            "",
        ]
        .into_iter()
        .filter_map(|path| value.pointer(path))
        .find(|value| {
            value.is_string()
                || value.as_object().is_some_and(|object| {
                    ["message", "detail", "code", "type"]
                        .iter()
                        .any(|key| object.contains_key(*key))
                })
        });
        if let Some(envelope) = envelope {
            let code = envelope.get("code").and_then(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| value.as_u64().map(|value| value.to_string()))
            });
            details.code = code.as_deref().and_then(safe_identifier);
            let error_type = envelope
                .get("type")
                .or_else(|| envelope.get("errorType"))
                .or_else(|| envelope.get("status"))
                .and_then(Value::as_str);
            details.error_type = error_type.and_then(safe_identifier);
            details.redacted |= code.is_some() && details.code.is_none()
                || error_type.is_some() && details.error_type.is_none();
            if let Some(message) = envelope.as_str().or_else(|| {
                envelope
                    .get("message")
                    .or_else(|| envelope.get("detail"))
                    .and_then(Value::as_str)
            }) {
                details.set_message(message);
            }
        }
        details
    }

    fn empty(http_status: Option<u16>) -> Self {
        Self {
            http_status: http_status.filter(|status| (100..600).contains(status)),
            code: None,
            error_type: None,
            message: None,
            redacted: false,
            truncated: false,
        }
    }

    pub fn sanitized(&self) -> Self {
        let mut details = Self::empty(self.http_status);
        details.code = self.code.as_deref().and_then(safe_identifier);
        details.error_type = self.error_type.as_deref().and_then(safe_identifier);
        details.redacted = self.redacted
            || self.code.is_some() && details.code.is_none()
            || self.error_type.is_some() && details.error_type.is_none();
        details.truncated = self.truncated;
        if let Some(message) = &self.message {
            details.set_message(message);
        }
        details
    }

    fn set_message(&mut self, message: &str) {
        let (message, redacted, truncated) = sanitize_message(message);
        self.message = (!message.is_empty()).then_some(message);
        self.redacted |= redacted;
        self.truncated |= truncated;
    }
}

impl From<Value> for UpstreamErrorDetails {
    fn from(value: Value) -> Self {
        let status = value
            .get("httpStatus")
            .and_then(Value::as_u64)
            .and_then(|status| u16::try_from(status).ok());
        let mut details = Self::from_value(status, &value);
        details.redacted |= value
            .get("redacted")
            .and_then(Value::as_bool)
            .unwrap_or_default();
        details.truncated |= value
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or_default();
        details
    }
}

fn safe_identifier(value: &str) -> Option<String> {
    let normalized = crate::normalize_error_code(value)?;
    let (_, redacted, truncated) = sanitize_message(value);
    (!redacted && !truncated && !normalized.starts_with("eyj")).then(|| value.trim().to_string())
}

fn sanitize_message(value: &str) -> (String, bool, bool) {
    static SENSITIVE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        [
            r#"(?i)\b(?:authorization|proxy-authorization|cookie|set-cookie|x-api-key|api[_ -]?key|access[_ -]?token|refresh[_ -]?token|id[_ -]?token|password|secret)\b[\"']?\s*[:=]\s*(?:\"[^\"]*\"|'[^']*'|[^\n,;]+)"#,
            r"(?i)\b(?:bearer|basic)\s+[^\s,;]+",
            r"(?i)\b(?:https?|wss?)://[^\s<>]+",
            r#"[^\s<>\"'@]+@[^\s<>\"'@]+"#,
            r"(?i)\b(?:sk-|sess-|org-|user-|acct_|acc-)[a-z0-9_./+-]+",
            r"\beyJ[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+){1,2}",
            r"(?i)\b[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\b",
            r"\b[A-Za-z0-9_+/=-]{40,}\b",
            r#"(?is)\b(?:prompt|messages|request[_ ]?body|input(?:[_ ]?text)?|output|conversation|content|arguments)\b[\"']?\s*[:=].*"#,
            r#"(?is)\b(?:received|provided|supplied|got)\s*[:=]\s*.*"#,
        ].into_iter().map(|pattern| Regex::new(pattern).expect("static diagnostic redaction regex")).collect()
    });
    let mut message = value
        .chars()
        .take(MAX_BODY_BYTES)
        .map(|ch| {
            if (ch.is_control() && !matches!(ch, '\n' | '\t'))
                || matches!(ch, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                ' '
            } else {
                ch
            }
        })
        .collect::<String>();
    let mut redacted = false;
    for pattern in SENSITIVE.iter() {
        redacted |= pattern.is_match(&message);
        message = pattern.replace_all(&message, "[redacted]").into_owned();
    }
    let message = message.trim();
    let truncated =
        value.chars().count() > MAX_BODY_BYTES || message.chars().count() > MAX_MESSAGE_CHARS;
    (
        message.chars().take(MAX_MESSAGE_CHARS).collect(),
        redacted,
        truncated,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deserialization_rechecks_secrets_and_preserves_safe_unknown_identifiers() {
        let details: UpstreamErrorDetails = serde_json::from_value(json!({
            "httpStatus": 422, "code": "future_validation_error", "errorType": "INVALID_ARGUMENT",
            "message": "Invalid request: \"access_token\":\"synthetic-private\", input: synthetic-echo",
            "redacted": false, "truncated": true
        })).unwrap();
        assert_eq!(details.error_type.as_deref(), Some("INVALID_ARGUMENT"));
        assert_eq!(details.code.as_deref(), Some("future_validation_error"));
        assert!(details.redacted && details.truncated);
        assert!(!serde_json::to_string(&details)
            .unwrap()
            .contains("synthetic-"));
        assert_eq!(details.sanitized(), details);
    }

    #[test]
    fn jwt_and_malformed_structured_bodies_cannot_be_saved_as_plain_text() {
        let details = UpstreamErrorDetails::from_value(
            None,
            &json!({"error": {
                "code": "sk-synthetic-secret", "message": "Invalid token eyJabc.eyJdef.synthetic. \"messages\": synthetic-echo"
            }}),
        );
        assert!(details.redacted);
        assert!(details.code.is_none());
        assert!(!details.message.unwrap().contains("synthetic"));
        for body in [
            b"{\"input\":\"synthetic".as_slice(),
            b"data: {\"delta\":\"synthetic\"}\n\n",
        ] {
            let details = UpstreamErrorDetails::from_body(Some(502), body);
            assert!(details.redacted);
            assert!(details.message.is_none());
        }
    }

    #[test]
    fn numeric_provider_code_and_status_are_not_replaced_by_relay_codes() {
        let details = UpstreamErrorDetails::from_value(
            Some(400),
            &json!({"error": {
                "code": 400, "status": "INVALID_ARGUMENT", "message": "Invalid field: generationConfig"
            }}),
        );
        assert_eq!(details.code.as_deref(), Some("400"));
        assert_eq!(details.error_type.as_deref(), Some("INVALID_ARGUMENT"));
        assert_eq!(details.http_status, Some(400));
    }

    #[test]
    fn unknown_codes_and_nested_messages_remain_inspectable() {
        let detail = UpstreamErrorDetails::from_value(
            Some(422),
            &json!({
                "response": {"error": {"code": "future_validation_error", "type": "invalid_request_error", "message": "Invalid parameter: input[2].id"}},
                "input": "synthetic request content must not be retained"
            }),
        );
        assert_eq!(detail.code.as_deref(), Some("future_validation_error"));
        assert_eq!(
            detail.message.as_deref(),
            Some("Invalid parameter: input[2].id")
        );
        assert_eq!(detail.http_status, Some(422));
        assert!(!serde_json::to_string(&detail)
            .unwrap()
            .contains("synthetic request content"));
    }

    #[test]
    fn credentials_urls_identities_and_echoed_prompts_are_redacted() {
        let detail = UpstreamErrorDetails::from_value(
            Some(401),
            &json!({"error": {
                "code": "invalid_api_key",
                "message": "Authentication failed: Bearer synthetic-access. Contact private@example.test at https://example.test/?key=synthetic. prompt: synthetic conversation"
            }}),
        );
        let serialized = serde_json::to_string(&detail).unwrap();
        assert!(detail.redacted);
        assert!(serialized.contains("Authentication failed"));
        for private in [
            "synthetic-access",
            "private@example",
            "https://",
            "synthetic conversation",
        ] {
            assert!(!serialized.contains(private));
        }
    }

    #[test]
    fn plain_text_is_bounded_and_html_is_not_retained() {
        let text = UpstreamErrorDetails::from_body(Some(503), b"Provider maintenance in progress");
        assert_eq!(
            text.message.as_deref(),
            Some("Provider maintenance in progress")
        );
        let long = UpstreamErrorDetails::from_value(
            None,
            &json!({"message": "long message ".repeat(500)}),
        );
        assert!(long.truncated);
        assert_eq!(long.message.unwrap().chars().count(), MAX_MESSAGE_CHARS);
        let html = UpstreamErrorDetails::from_body(Some(502), b"<html>private diagnostic</html>");
        assert!(html.redacted);
        assert!(html.message.is_none());
    }
}
