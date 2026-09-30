use super::super::{
    now_ms, AuthenticatedKey, GatewayFailure, RESPONSES_LITE_METADATA_KEY, WEBSOCKET_PROTOCOLS,
};
use crate::gateway::continuation;
use crate::gateway::request::{
    client_context_fingerprint, codex_background_request_kind, is_managed_codex_client,
    RequestToolPolicy, ServiceTierPolicy,
};
use crate::scheduler::rotation::SharedRequestBudget;
use crate::GatewayRuntime;
use axum::http::HeaderMap;
use serde_json::Value;

impl super::ClientRequest {
    pub(in crate::gateway::websocket) fn error_stream_id(payload: &[u8]) -> Option<String> {
        serde_json::from_slice::<Value>(payload)
            .ok()?
            .get("stream_id")?
            .as_str()
            .filter(|stream_id| valid_stream_id(stream_id))
            .map(str::to_string)
    }

    pub(in crate::gateway::websocket) fn parse(
        runtime: &GatewayRuntime,
        key: &AuthenticatedKey,
        headers: &HeaderMap,
        payload: &[u8],
    ) -> Result<Self, GatewayFailure> {
        Self::parse_on_connection(runtime, key, headers, payload, None)
    }

    pub(in crate::gateway::websocket) fn parse_on_connection(
        runtime: &GatewayRuntime,
        key: &AuthenticatedKey,
        headers: &HeaderMap,
        payload: &[u8],
        connection_response: Option<(&str, &str)>,
    ) -> Result<Self, GatewayFailure> {
        if payload.len() > super::super::MAX_WEBSOCKET_MESSAGE_BYTES {
            return Err(GatewayFailure::invalid_request(
                "WebSocket request is too large",
            ));
        }
        let mut value: Value = serde_json::from_slice(payload)
            .map_err(|_| GatewayFailure::invalid_request("request must be valid JSON"))?;
        let tool_policy = RequestToolPolicy::new(runtime, &value);
        let object = value
            .as_object_mut()
            .ok_or_else(|| GatewayFailure::invalid_request("request must be a JSON object"))?;
        if object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "response.create")
        {
            return Err(GatewayFailure::invalid_request(
                "only response.create messages are supported",
            ));
        }
        let stream_id = match object.get("stream_id") {
            None => None,
            Some(Value::String(stream_id)) => {
                if !valid_stream_id(stream_id) {
                    return Err(GatewayFailure::invalid_stream_id());
                }
                Some(stream_id.clone())
            }
            Some(_) => return Err(GatewayFailure::invalid_stream_id()),
        };
        let requested_model = object
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .ok_or_else(|| GatewayFailure::invalid_request("model must be a non-empty string"))?
            .to_string();
        let service_tier_policy = if is_managed_codex_client(headers) {
            ServiceTierPolicy::pool_owned(&value)
        } else {
            ServiceTierPolicy::client_owned(&value)
        };
        let background_kind = codex_background_request_kind(headers, &value);
        let request_id = crate::gateway::request::request_id();
        if let Some(kind) = background_kind {
            runtime.mark_request_origin(&request_id, kind);
        }
        let resolved_model = runtime
            .resolve_visible_model(key, &requested_model, WEBSOCKET_PROTOCOLS, now_ms())
            .or_else(|| {
                runtime
                    .route_recovery_enabled()
                    .then(|| {
                        runtime.resolve_configured_model(key, &requested_model, WEBSOCKET_PROTOCOLS)
                    })
                    .flatten()
            })
            .ok_or_else(GatewayFailure::model_not_found)?;
        let responses_lite = headers
            .contains_key(crate::gateway::request::CODEX_RESPONSES_LITE_HEADER)
            || metadata_flag(&value, RESPONSES_LITE_METADATA_KEY);
        // Responses Lite is a transport contract, not an OAuth-only option.
        // Normalize it before route selection so every selected provider sees
        // the same serial-tool request shape.
        if responses_lite {
            let object = value
                .as_object_mut()
                .expect("request object was validated before normalization");
            if !crate::gateway::request::responses_lite_parallel_tool_calls_valid(object) {
                return Err(GatewayFailure::invalid_request(
                    "responses Lite requires parallel_tool_calls to be a boolean",
                ));
            }
            crate::gateway::request::normalize_responses_lite_request(object);
        }
        // Keep automatic Lite consistent with HTTP: a pool that can fall back
        // to a non-Lite or non-official Responses route must stay on full
        // Responses for the whole request contract. An explicit client Lite
        // signal is still preserved by `responses_lite` above.
        let responses_lite_candidates =
            if runtime.codex_model_responses_routes_all_support_lite(key, &resolved_model) {
                runtime.codex_model_responses_lite_candidates(&resolved_model)
            } else {
                Default::default()
            };
        let connection_affinity_key = connection_response
            .filter(|(id, _)| {
                value.get("previous_response_id").and_then(Value::as_str) == Some(*id)
            })
            .map(|(_, affinity)| affinity);
        let continuation = continuation::prepare_response_continuation(
            runtime,
            &key.id,
            &mut value,
            now_ms(),
            connection_affinity_key,
        )
        .map_err(|()| GatewayFailure::continuation_unavailable())?;
        let client_context_id = client_context_fingerprint(headers);
        let prompt_affinity_key = runtime.prompt_affinity_key(
            &key.id,
            &resolved_model,
            value.get("prompt_cache_key").and_then(Value::as_str),
            client_context_id.as_deref(),
        );
        Ok(Self {
            tool_policy,
            request_id,
            budget: SharedRequestBudget::for_incoming_request(runtime.request_dispatch_budget()),
            value,
            requested_model,
            resolved_model,
            stream_id,
            responses_lite,
            service_tier_policy,
            responses_lite_candidates,
            response_affinity_key: continuation.response_affinity_key,
            requires_affinity_owner: continuation.requires_affinity_owner,
            has_unpaired_tool_output: continuation.has_unpaired_tool_output,
            prompt_affinity_key,
            background_kind,
        })
    }
}

fn valid_stream_id(stream_id: &str) -> bool {
    !stream_id.is_empty()
        && stream_id.len() <= 256
        && stream_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn metadata_flag(value: &Value, key: &str) -> bool {
    value
        .get("client_metadata")
        .and_then(|metadata| metadata.get(key))
        .is_some_and(|value| match value {
            Value::Bool(value) => *value,
            Value::String(value) => value.eq_ignore_ascii_case("true"),
            _ => false,
        })
}
