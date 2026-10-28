use super::events::{websocket_reset_delay_seconds, websocket_retry_headers};
use super::{
    ClientRequest, EventTerminalOutcome, GatewayFailure, RELAY_ERROR_ORIGIN_HEADER,
    RELAY_UPSTREAM_ORIGIN_HEADER, WEBSOCKET_PROTOCOLS, event_terminal, fallback_event_message,
    fallback_response_origin, incomplete_requires_cooldown, initial_payloads_are_empty_incomplete,
    semantic_output_payload, terminal_failure_status,
};
use crate::{
    ErrorOrigin, GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey, ProviderSource,
    RuntimeLocalKey, RuntimeSource, WireApi,
};
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use serde_json::Value;
use std::sync::Arc;

mod agent_messages;
mod client_requests;
mod frame_contract;

fn runtime() -> GatewayRuntime {
    runtime_with_events(Arc::new(|_| {}))
}

fn runtime_with_events(events: Arc<dyn Fn(crate::UsageEvent) + Send + Sync>) -> GatewayRuntime {
    GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(ProviderSource {
            id: "source".into(),
            name: "source".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "upstream-secret".into(),
            wire_api: WireApi::Responses,
            models: vec!["upstream-model".into()],
            proxy_url: None,
        })],
        vec![RuntimeLocalKey {
            key: LocalGatewayKey {
                id: "key".into(),
                secret: "local-secret".into(),
            },
            enabled: true,
            source_ids: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: Some("relay".into()),
        }],
        GatewayRuntimeOptions {
            diagnostics_enabled: || true,
            ..Default::default()
        },
        events,
    )
    .unwrap()
}
