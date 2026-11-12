use super::UsageTransport;
use crate::runtime::{AccountTransport, ExecutorRoute};
use axum::http::{HeaderMap, header::CONTENT_TYPE};
use serde::{Deserialize, Serialize};

mod payload;
mod server_timing;
mod stream;
pub use payload::{
    FullRequestReason, RequestContentEncoding, RequestPayloadDiagnostics, RequestPayloadMode,
    SocketOutputMismatch, SocketReplayField, SocketReplayItemKind, SocketUnknownFieldChanges,
};
pub use server_timing::CodexServerTiming;
pub use stream::StreamResponseDiagnostics;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPreparationDiagnostics {
    /// SIWC request preparation omitted an unsupported client output-token cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub siwc_output_limit_omitted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_ms: Option<u64>,
    /// Initial schema validation is a subset of body_ms, not an extra stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_validation_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_ms: Option<u64>,
    /// Final JSON encoding, separate from catalog work in new observations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serialization_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<AttachmentPreparationDiagnostics>,
    /// Opt-in prepared-context accounting after attachment resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_accounting_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentPreparationDiagnostics {
    /// Absent when no inline images require credential preparation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ms: Option<u64>,
    pub staging_ms: u64,
    /// Wall time for concurrent resolution, including coalesced-upload waits.
    pub resolve_ms: u64,
    pub image_count: u64,
    pub file_references: u64,
    /// Successful physical uploads performed by this request.
    pub uploaded_images: u64,
    pub uploaded_bytes: u64,
    /// Inline images satisfied by cached or shared uploads.
    pub reused_images: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportDiagnostics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_http_status: Option<u16>,
    /// Response constructed for HTTP ingress, not proof of client receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downstream_http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<TransportFailureKind>,
    /// Dispatch boundary, not proof that the provider accepted a generation.
    pub upstream: UsageTransport,
    pub mode: TransportMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<TransportFallbackReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<TransportResponseFormat>,
    /// Observed upstream response version, independent of client ingress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handshake_ms: Option<u64>,
    /// Native WS: false is selected-route setup before generation, true is reuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_reused: Option<bool>,
    /// Why SIWC opened a fresh socket; bounded local state, not provider text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_reset_reason: Option<ConnectionResetReason>,
    /// New native WS attempt after authorization changed on the same candidate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reauth: Option<bool>,
    /// First HTTP response headers since the executor's dispatch clock started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_headers_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_response: Option<StreamResponseDiagnostics>,
    /// Selected-attempt preparation duration before the dispatch clock starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_preparation_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_preparation: Option<RequestPreparationDiagnostics>,
    /// Encoded HTTP body or WebSocket JSON text; excludes framing/TLS and delivery claims.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_payload: Option<RequestPayloadDiagnostics>,
    /// Raw upstream completion offset, before client translation/validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_response_ms: Option<u64>,
    /// First meaningful normalized SSE output offset; not client receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_first_output_ms: Option<u64>,
    /// Successful normalized response readiness offset; not client receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_response_ready_ms: Option<u64>,
    /// Optional provider-reported values. Not additive Relay phases or cache-hit evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_server_timing: Option<CodexServerTiming>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportFailureKind {
    Connection,
    Upstream,
    Local,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionResetReason {
    NewSession,
    MissingContext,
    SessionUnavailable,
    SessionExpired,
    AuthorizationChanged,
    ConnectionExpired,
    ConnectionClosed,
    PendingFrame,
    HeartbeatFailed,
    ResponseInterrupted,
    ResponseFailed,
    UnprovenOutput,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    Native,
    ProtocolConversion,
    BasisPoints,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportFallbackReason {
    BasisPointsHttp,
    ProtocolConversion,
    WebsocketDisabled,
    WebsocketUnavailable,
    WebsocketSlowHandshake,
    ExistingHttpBridge,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportResponseFormat {
    Json,
    Sse,
    WebsocketFrames,
}

impl ExecutorRoute {
    pub(crate) fn observe_authorized_dispatch_failure(
        &mut self,
        error: &crate::runtime::AuthorizedRequestError,
    ) {
        use crate::runtime::AuthorizedRequestError;
        if let Some(transport) = self
            .routing
            .as_mut()
            .and_then(|routing| routing.transport.as_mut())
        {
            if matches!(error, AuthorizedRequestError::WebsocketSend) {
                transport.upstream = UsageTransport::Websocket;
                transport.fallback_reason = None;
                transport.http_version = None;
                transport.upstream_http_status = None;
                transport.response_headers_ms = None;
                transport.response_format = Some(TransportResponseFormat::WebsocketFrames);
            }
            transport.failure_kind = match error {
                AuthorizedRequestError::Transport(error) if error.is_connect() => {
                    Some(TransportFailureKind::Connection)
                }
                AuthorizedRequestError::NotReplayable
                | AuthorizedRequestError::DispatchBudgetExhausted
                | AuthorizedRequestError::ModelUnavailable
                | AuthorizedRequestError::ReasoningUnavailable => Some(TransportFailureKind::Local),
                _ => None,
            };
        }
    }

    pub(crate) fn observe_http_dispatch(&mut self, fallback: Option<TransportFallbackReason>) {
        if !self.diagnostics_enabled {
            return;
        }
        let mode = if self.account_transport == AccountTransport::ExcelBasisPoints {
            TransportMode::BasisPoints
        } else if self.adapter.is_passthrough() {
            TransportMode::Native
        } else {
            TransportMode::ProtocolConversion
        };
        let fallback_reason =
            (self.client_transport == UsageTransport::Websocket).then(|| match mode {
                TransportMode::BasisPoints => TransportFallbackReason::BasisPointsHttp,
                TransportMode::ProtocolConversion => TransportFallbackReason::ProtocolConversion,
                TransportMode::Native => {
                    fallback.unwrap_or(TransportFallbackReason::ExistingHttpBridge)
                }
            });
        if let Some(routing) = self.routing.as_mut() {
            routing.transport = Some(TransportDiagnostics {
                upstream_http_status: None,
                downstream_http_status: None,
                failure_kind: None,
                upstream: UsageTransport::Http,
                mode,
                fallback_reason,
                response_format: None,
                http_version: None,
                handshake_ms: None,
                connection_reused: None,
                connection_reset_reason: None,
                reauth: None,
                response_headers_ms: None,
                stream_response: None,
                request_preparation_ms: None,
                request_preparation: None,
                request_payload: None,
                upstream_response_ms: None,
                client_first_output_ms: None,
                client_response_ready_ms: None,
                codex_server_timing: None,
            });
        }
    }

    pub(crate) fn observe_request_preparation(
        &mut self,
        elapsed_ms: u64,
        preparation: RequestPreparationDiagnostics,
    ) {
        if let Some(transport) = self
            .routing
            .as_mut()
            .and_then(|routing| routing.transport.as_mut())
        {
            transport.request_preparation_ms.get_or_insert(elapsed_ms);
            transport.request_preparation.get_or_insert(preparation);
        }
    }

    pub(crate) fn observe_http_response(&mut self, headers: &HeaderMap, elapsed_ms: u64) {
        let Some(transport) = self
            .routing
            .as_mut()
            .and_then(|routing| routing.transport.as_mut())
        else {
            return;
        };
        // SIWC wraps WebSocket events as SSE internally. Those synthetic
        // headers are not an observed HTTP response from the provider.
        if transport.upstream == UsageTransport::Websocket {
            return;
        }
        transport.response_headers_ms.get_or_insert(elapsed_ms);
        transport.response_format = headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(response_format);
    }

    pub(crate) fn observe_websocket_dispatch(&mut self, handshake_ms: Option<u64>) {
        if !self.diagnostics_enabled {
            return;
        }
        if let Some(routing) = self.routing.as_mut() {
            routing.transport = Some(TransportDiagnostics {
                upstream_http_status: None,
                downstream_http_status: None,
                failure_kind: None,
                upstream: UsageTransport::Websocket,
                mode: TransportMode::Native,
                fallback_reason: None,
                response_format: None,
                http_version: None,
                handshake_ms,
                connection_reused: Some(false),
                connection_reset_reason: None,
                reauth: None,
                response_headers_ms: None,
                stream_response: None,
                request_preparation_ms: None,
                request_preparation: None,
                request_payload: None,
                upstream_response_ms: None,
                client_first_output_ms: None,
                client_response_ready_ms: None,
                codex_server_timing: None,
            });
        }
    }

    pub(crate) fn observe_websocket_handshake(
        &mut self,
        elapsed_ms: u64,
        upgraded: bool,
        reauth: bool,
    ) {
        self.observe_websocket_dispatch(Some(elapsed_ms));
        if let Some(transport) = self
            .routing
            .as_mut()
            .and_then(|routing| routing.transport.as_mut())
        {
            transport.reauth = Some(reauth);
        }
        if upgraded {
            self.observe_websocket_response();
        }
    }

    pub(crate) fn observe_websocket_response(&mut self) {
        if let Some(transport) = self
            .routing
            .as_mut()
            .and_then(|routing| routing.transport.as_mut())
        {
            transport.response_format = Some(TransportResponseFormat::WebsocketFrames);
        }
    }
}

fn response_format(content_type: &str) -> Option<TransportResponseFormat> {
    let media_type = content_type.split(';').next()?.trim();
    if media_type.eq_ignore_ascii_case("text/event-stream") {
        Some(TransportResponseFormat::Sse)
    } else if media_type.eq_ignore_ascii_case("application/json")
        || media_type.to_ascii_lowercase().ends_with("+json")
    {
        Some(TransportResponseFormat::Json)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::SourceAdapter;
    use crate::{
        GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeLocalKey,
        RuntimeSource, WireApi,
    };
    use serde_json::json;
    use std::sync::Arc;

    fn route() -> ExecutorRoute {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "synthetic-source".into(),
                name: "Synthetic".into(),
                base_url: "https://provider.invalid/v1".into(),
                api_key: "synthetic-secret".into(),
                wire_api: WireApi::Responses,
                models: vec!["synthetic-model".into()],
                proxy_url: None,
            })],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "synthetic-key".into(),
                secret: "synthetic-local".into(),
            })],
            GatewayRuntimeOptions {
                diagnostics_enabled: || true,
                ..Default::default()
            },
            Arc::new(|_| {}),
        )
        .unwrap();
        let key = runtime
            .authenticate(Some(&axum::http::HeaderValue::from_static(
                "Bearer synthetic-local",
            )))
            .unwrap();
        let mut route = runtime
            .executor_route(
                "synthetic-source",
                "synthetic-model",
                &key.scope_snapshot(),
                &WireApi::ALL,
                false,
            )
            .unwrap();
        route.routing = Some(
            serde_json::from_value(json!({
                "reason":"only_eligible","eligibleCandidates":1,"quotaRemainingBasisPoints":null,
                "inFlightBefore":0,"dispatchesBefore":0
            }))
            .unwrap(),
        );
        route
    }

    #[test]
    fn native_converted_and_bps_paths_keep_ingress_and_missing_measurements_separate() {
        let mut route = route();
        assert!(route.routing.as_ref().unwrap().transport.is_none());
        for client in [UsageTransport::Http, UsageTransport::Websocket] {
            route.client_transport = client;
            for (account, adapter, mode, fallback) in [
                (
                    AccountTransport::NativeResponses,
                    SourceAdapter::Native,
                    TransportMode::Native,
                    TransportFallbackReason::WebsocketUnavailable,
                ),
                (
                    AccountTransport::NativeResponses,
                    SourceAdapter::ResponsesToMessages,
                    TransportMode::ProtocolConversion,
                    TransportFallbackReason::ProtocolConversion,
                ),
                (
                    AccountTransport::ExcelBasisPoints,
                    SourceAdapter::Native,
                    TransportMode::BasisPoints,
                    TransportFallbackReason::BasisPointsHttp,
                ),
            ] {
                route.account_transport = account;
                route.adapter = adapter;
                route.observe_http_dispatch(Some(TransportFallbackReason::WebsocketUnavailable));
                let actual = route.routing.as_ref().unwrap().transport.as_ref().unwrap();
                assert_eq!(actual.upstream, UsageTransport::Http);
                assert_eq!(actual.mode, mode);
                assert_eq!(
                    actual.fallback_reason,
                    (client == UsageTransport::Websocket).then_some(fallback)
                );
                assert_eq!(actual.handshake_ms, None);
                assert_eq!(actual.response_format, None);
                assert_eq!(route.client_transport, client);
            }
        }
        route.observe_websocket_handshake(12, true, false);
        let actual = route.routing.as_ref().unwrap().transport.as_ref().unwrap();
        assert_eq!(actual.handshake_ms, Some(12));
        assert_eq!(actual.connection_reused, Some(false));
        assert_eq!(actual.reauth, Some(false));
        route.observe_websocket_handshake(15, false, true);
        let failed = route.routing.as_ref().unwrap().transport.as_ref().unwrap();
        assert_eq!(failed.reauth, Some(true));
        assert_eq!(failed.handshake_ms, Some(15));
        assert_eq!(failed.response_format, None);
        route.observe_websocket_handshake(16, true, true);
        let actual = route.routing.as_ref().unwrap().transport.as_ref().unwrap();
        assert_eq!(
            actual.response_format,
            Some(TransportResponseFormat::WebsocketFrames)
        );
        route.observe_websocket_reuse();
        assert_eq!(
            route
                .routing
                .as_ref()
                .unwrap()
                .transport
                .as_ref()
                .unwrap()
                .reauth,
            None
        );
        assert_eq!(
            route
                .routing
                .as_ref()
                .unwrap()
                .transport
                .as_ref()
                .unwrap()
                .connection_reused,
            Some(true)
        );
        assert_eq!(
            route
                .routing
                .as_ref()
                .unwrap()
                .transport
                .as_ref()
                .unwrap()
                .handshake_ms,
            None
        );
        route.observe_http_dispatch(None);
        let actual = route.routing.as_ref().unwrap().transport.as_ref().unwrap();
        assert_eq!(actual.response_format, None);
        assert_eq!(actual.handshake_ms, None);
        assert_eq!(actual.connection_reused, None);
        assert_eq!(actual.reauth, None);
    }

    #[test]
    fn websocket_timing_does_not_leak_across_reused_or_unknown_connections() {
        let mut route = route();
        let mut timing = json!({"type":"responsesapi.websocket_timing",
            "timing_metrics":{"engine_service_total_ms":42}});
        route.observe_websocket_handshake(12, true, false);
        let transport = route.routing.as_mut().unwrap().transport.as_mut().unwrap();
        transport.observe_codex_server_timing(&timing, Some("first"));
        assert!(transport.codex_server_timing.is_some());

        // This is the same reset used by the bridge before its second dispatch.
        route.observe_websocket_reuse();
        let transport = route.routing.as_mut().unwrap().transport.as_mut().unwrap();
        assert!(transport.codex_server_timing.is_none());
        transport.observe_codex_server_timing(&timing, Some("second"));
        assert!(transport.codex_server_timing.is_none());
        timing["response_id"] = json!("first");
        transport.observe_codex_server_timing(&timing, Some("second"));
        assert!(transport.codex_server_timing.is_none());
        timing["response_id"] = json!("second");
        transport.observe_codex_server_timing(&timing, Some("second"));
        assert!(transport.codex_server_timing.is_some());

        transport.codex_server_timing = None;
        transport.connection_reused = None;
        timing.as_object_mut().unwrap().remove("response_id");
        transport.observe_codex_server_timing(&timing, Some("third"));
        assert!(transport.codex_server_timing.is_none());
    }

    #[test]
    fn response_format_uses_actual_media_type_not_client_stream_flag() {
        assert_eq!(
            response_format("Text/Event-Stream; charset=utf-8"),
            Some(TransportResponseFormat::Sse)
        );
        assert_eq!(
            response_format("application/problem+json"),
            Some(TransportResponseFormat::Json)
        );
        assert_eq!(
            response_format("application/json; charset=utf-8"),
            Some(TransportResponseFormat::Json)
        );
        assert_eq!(response_format("text/html"), None);
    }

    #[test]
    fn http_header_timing_keeps_first_observation_and_resets_on_new_dispatch() {
        let mut route = route();
        route.observe_http_dispatch(None);
        let headers = HeaderMap::new();
        route.observe_http_response(&headers, 0);
        route.observe_http_response(&headers, 55);
        let transport = route.routing.as_ref().unwrap().transport.as_ref().unwrap();
        assert_eq!(transport.response_headers_ms, Some(0));
        route.observe_http_dispatch(None);
        let transport = route.routing.as_ref().unwrap().transport.as_ref().unwrap();
        assert_eq!(transport.response_headers_ms, None);
        route.observe_websocket_dispatch(None);
        let transport = route.routing.as_ref().unwrap().transport.as_ref().unwrap();
        assert_eq!(transport.response_headers_ms, None);
    }

    #[test]
    fn stage_timings_preserve_missing_zero_and_reset_between_attempts() {
        let legacy = json!({"upstream":"http", "mode":"basis_points"});
        let decoded: TransportDiagnostics = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
        let mut route = route();
        route.observe_http_dispatch(None);
        let preparation = RequestPreparationDiagnostics {
            siwc_output_limit_omitted: None,
            body_ms: Some(0),
            schema_validation_ms: Some(0),
            catalog_ms: Some(0),
            serialization_ms: Some(0),
            attachments_ms: Some(0),
            attachments: None,
            context_accounting_ms: None,
        };
        route.observe_request_preparation(0, preparation.clone());
        route.observe_request_preparation(42, RequestPreparationDiagnostics::default());
        let transport = route.routing.as_mut().unwrap().transport.as_mut().unwrap();
        assert_eq!(transport.request_preparation_ms, Some(0));
        assert_eq!(transport.request_preparation.as_ref(), Some(&preparation));
        transport.upstream_response_ms = Some(0);
        transport.client_first_output_ms = Some(0);
        transport.client_response_ready_ms = Some(0);
        let encoded = serde_json::to_value(&*transport).unwrap();
        for field in [
            "requestPreparationMs",
            "upstreamResponseMs",
            "clientFirstOutputMs",
            "clientResponseReadyMs",
        ] {
            assert_eq!(encoded[field], 0);
        }
        assert_eq!(
            encoded["requestPreparation"],
            json!({
                "bodyMs":0, "schemaValidationMs":0, "catalogMs":0, "serializationMs":0, "attachmentsMs":0
            })
        );
        assert_eq!(
            serde_json::from_value::<TransportDiagnostics>(encoded).unwrap(),
            *transport
        );
        route.observe_http_dispatch(None);
        let transport = route.routing.as_ref().unwrap().transport.as_ref().unwrap();
        assert_eq!(transport.request_preparation_ms, None);
        assert_eq!(transport.request_preparation, None);
        assert_eq!(transport.upstream_response_ms, None);
        assert_eq!(transport.client_first_output_ms, None);
        assert_eq!(transport.client_response_ready_ms, None);
        let partial: RequestPreparationDiagnostics =
            serde_json::from_value(json!({"bodyMs": 0})).unwrap();
        assert_eq!(serde_json::to_value(partial).unwrap(), json!({"bodyMs": 0}));
    }

    #[test]
    fn siwc_socket_diagnostics_do_not_report_synthetic_http_headers() {
        let mut route = route();
        route.observe_http_dispatch(None);
        route.observe_request_preparation(9, RequestPreparationDiagnostics::default());
        let mut response: reqwest::Response = axum::http::Response::builder()
            .status(200)
            .header(CONTENT_TYPE, "text/event-stream")
            .body("")
            .unwrap()
            .into();
        response
            .extensions_mut()
            .insert(crate::runtime::siwc_socket::SocketDiagnostics {
                reused: true,
                handshake_ms: None,
                reset_reason: None,
            });
        route.observe_http_payload(&response);
        route.observe_http_response(response.headers(), 15);
        let transport = route.routing.as_ref().unwrap().transport.as_ref().unwrap();
        assert_eq!(transport.upstream, UsageTransport::Websocket);
        assert_eq!(
            transport.response_format,
            Some(TransportResponseFormat::WebsocketFrames)
        );
        assert_eq!(transport.connection_reused, Some(true));
        assert_eq!(transport.http_version, None);
        assert_eq!(transport.upstream_http_status, None);
        assert_eq!(transport.response_headers_ms, None);
        assert_eq!(transport.request_preparation_ms, Some(9));
        response
            .extensions_mut()
            .remove::<crate::runtime::siwc_socket::SocketDiagnostics>();
        route.observe_http_payload(&response);
        route.observe_http_response(response.headers(), 20);
        let transport = route.routing.as_ref().unwrap().transport.as_ref().unwrap();
        assert_eq!(transport.upstream, UsageTransport::Http);
        assert_eq!(transport.connection_reused, None);
        assert_eq!(transport.response_headers_ms, Some(20));
    }

    #[test]
    fn upstream_http_version_is_observed_and_never_carried_between_attempts() {
        fn version(route: &ExecutorRoute) -> Option<&str> {
            let transport = route.routing.as_ref()?.transport.as_ref()?;
            transport.http_version.as_deref()
        }
        let mut route = route();
        for (http_version, label) in [
            (axum::http::Version::HTTP_11, "HTTP/1.1"),
            (axum::http::Version::HTTP_2, "HTTP/2.0"),
        ] {
            route.observe_http_dispatch(None);
            let response = reqwest::Response::from(
                axum::http::Response::builder()
                    .status(429)
                    .version(http_version)
                    .body(String::new())
                    .unwrap(),
            );
            route.observe_http_payload(&response);
            assert_eq!(version(&route), Some(label));
            assert_eq!(
                route
                    .routing
                    .as_ref()
                    .unwrap()
                    .transport
                    .as_ref()
                    .unwrap()
                    .upstream_http_status,
                Some(429)
            );
            route.observe_websocket_dispatch(None);
            assert_eq!(version(&route), None);
            assert_eq!(
                route
                    .routing
                    .as_ref()
                    .unwrap()
                    .transport
                    .as_ref()
                    .unwrap()
                    .upstream_http_status,
                None
            );
        }
        route.observe_http_dispatch(None);
        assert_eq!(version(&route), None);
    }

    #[tokio::test]
    async fn connection_and_local_failures_do_not_invent_an_http_response() {
        let mut route = route();
        route.observe_http_dispatch(None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let error = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap_err();
        assert!(error.is_connect());
        route.observe_authorized_dispatch_failure(
            &crate::runtime::AuthorizedRequestError::Transport(error),
        );
        let transport = route.routing.as_ref().unwrap().transport.as_ref().unwrap();
        assert_eq!(
            transport.failure_kind,
            Some(TransportFailureKind::Connection)
        );
        assert_eq!(transport.upstream_http_status, None);
        assert_eq!(transport.downstream_http_status, None);
        route.observe_http_dispatch(None);
        route.observe_authorized_dispatch_failure(
            &crate::runtime::AuthorizedRequestError::NotReplayable,
        );
        assert_eq!(
            route
                .routing
                .as_ref()
                .unwrap()
                .transport
                .as_ref()
                .unwrap()
                .failure_kind,
            Some(TransportFailureKind::Local)
        );
        route.observe_http_dispatch(None);
        route.observe_authorized_dispatch_failure(
            &crate::runtime::AuthorizedRequestError::HeadersTimeout,
        );
        assert_eq!(
            route
                .routing
                .as_ref()
                .unwrap()
                .transport
                .as_ref()
                .unwrap()
                .failure_kind,
            None
        );
    }
}
