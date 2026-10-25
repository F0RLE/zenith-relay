use super::TransportMode;
use crate::runtime::ExecutorRoute;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestPayloadMode {
    Full,
    Continuation,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FullRequestReason {
    BasisPointsTransport,
    ProtocolConversion,
    /// A fact about this payload, not an inferred client decision or cache miss.
    NoPreviousResponseId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestContentEncoding {
    Identity,
    Zstd,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPayloadDiagnostics {
    pub mode: RequestPayloadMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_request_reason: Option<FullRequestReason>,
    pub json_bytes: u64,
    pub body_bytes: u64,
    pub content_encoding: RequestContentEncoding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression_ms: Option<u64>,
    /// Encoding wall time, including preparation and blocking-pool scheduling.
    /// Microseconds preserve sub-millisecond observations; not codec CPU time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression_us: Option<u64>,
    /// HTTP execute to response headers, excluding compression/authorization.
    /// Includes connection setup and upstream waiting; not isolated upload time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_send_to_headers_ms: Option<u64>,
}

impl RequestPayloadDiagnostics {
    pub(crate) fn identity(bytes: &[u8], mode: TransportMode) -> Self {
        // Borrow the top-level ID when possible, allowing JSON escapes. Never build
        // another history tree or retain/log the ID, prompts, schemas or arguments.
        #[derive(Deserialize)]
        struct Continuation<'a> {
            #[serde(borrow)]
            previous_response_id: Option<std::borrow::Cow<'a, str>>,
        }
        let continuation = serde_json::from_slice::<Continuation<'_>>(bytes)
            .ok()
            .and_then(|body| body.previous_response_id)
            .is_some_and(|id| !id.trim().is_empty());
        Self {
            mode: if continuation {
                RequestPayloadMode::Continuation
            } else {
                RequestPayloadMode::Full
            },
            full_request_reason: (!continuation).then_some(match mode {
                TransportMode::BasisPoints => FullRequestReason::BasisPointsTransport,
                TransportMode::ProtocolConversion => FullRequestReason::ProtocolConversion,
                TransportMode::Native => FullRequestReason::NoPreviousResponseId,
            }),
            json_bytes: bytes.len() as u64,
            body_bytes: bytes.len() as u64,
            content_encoding: RequestContentEncoding::Identity,
            compression_ms: None,
            compression_us: None,
            http_send_to_headers_ms: None,
        }
    }
}

impl ExecutorRoute {
    pub(crate) fn observe_websocket_reuse(&mut self) {
        self.observe_websocket_dispatch(None);
        if let Some(transport) = self
            .routing
            .as_mut()
            .and_then(|routing| routing.transport.as_mut())
        {
            transport.connection_reused = Some(true);
        }
    }

    pub(crate) fn observe_http_payload(&mut self, response: &reqwest::Response) {
        if let Some(transport) = self
            .routing
            .as_mut()
            .and_then(|routing| routing.transport.as_mut())
        {
            transport.request_payload = response
                .extensions()
                .get::<RequestPayloadDiagnostics>()
                .cloned();
        }
    }

    pub(crate) fn observe_websocket_payload(&mut self, payload: &str) {
        if let Some(transport) = self
            .routing
            .as_mut()
            .and_then(|routing| routing.transport.as_mut())
        {
            transport.request_payload = Some(RequestPayloadDiagnostics::identity(
                payload.as_bytes(),
                transport.mode,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn payload_timings_preserve_missing_legacy_fields_and_measured_zero() {
        let mut observed = RequestPayloadDiagnostics::identity(b"{}", TransportMode::Native);
        let legacy = serde_json::to_value(&observed).unwrap();
        assert!(legacy.get("compressionUs").is_none());
        assert!(legacy.get("httpSendToHeadersMs").is_none());
        let restored: RequestPayloadDiagnostics = serde_json::from_value(legacy).unwrap();
        assert_eq!(restored.compression_us, None);
        assert_eq!(restored.http_send_to_headers_ms, None);
        observed.compression_ms = Some(0);
        observed.compression_us = Some(420);
        observed.http_send_to_headers_ms = Some(0);
        let encoded = serde_json::to_value(observed).unwrap();
        assert_eq!(encoded["compressionUs"], 420);
        assert_eq!(encoded["httpSendToHeadersMs"], 0);
    }

    #[test]
    fn payload_mode_uses_actual_top_level_continuation_without_private_values() {
        let body = br#"{"previous_response_id":"resp_synthetic","input":[{"role":"user","content":"synthetic private text"}]}"#;
        let observed = RequestPayloadDiagnostics::identity(body, TransportMode::Native);
        assert_eq!(observed.mode, RequestPayloadMode::Continuation);
        assert_eq!(observed.full_request_reason, None);
        assert_eq!(observed.body_bytes, body.len() as u64);
        let encoded = serde_json::to_string(&observed).unwrap();
        assert!(!encoded.contains("resp_synthetic"));
        assert!(!encoded.contains("private text"));
        let escaped = RequestPayloadDiagnostics::identity(
            br#"{"previous_response_id":"resp\u005fsynthetic"}"#,
            TransportMode::Native,
        );
        assert_eq!(escaped.mode, RequestPayloadMode::Continuation);
        for (mode, reason) in [
            (
                TransportMode::Native,
                FullRequestReason::NoPreviousResponseId,
            ),
            (
                TransportMode::BasisPoints,
                FullRequestReason::BasisPointsTransport,
            ),
            (
                TransportMode::ProtocolConversion,
                FullRequestReason::ProtocolConversion,
            ),
        ] {
            let full = RequestPayloadDiagnostics::identity(
                br#"{"input":[{"previous_response_id":"nested_only"}]}"#,
                mode,
            );
            assert_eq!(full.mode, RequestPayloadMode::Full);
            assert_eq!(full.full_request_reason, Some(reason));
        }
    }
}
