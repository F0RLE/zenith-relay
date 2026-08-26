use super::{
    now_ms, AuthenticatedKey, ExecutorRoute, GatewayFailure, RESPONSES_LITE_METADATA_KEY,
    WEBSOCKET_PROTOCOLS,
};
use crate::gateway::request::apply_default_service_tier_if_missing;
use crate::gateway::request::client_context_fingerprint;
use crate::gateway::request::codex_background_request_kind;
use crate::usage::ReasoningEffortDiagnostics;
use crate::{DefaultServiceTier, GatewayRuntime, ToolUseDiagnostics, WireApi};
use axum::http::HeaderMap;
use serde_json::Value;

#[derive(Clone)]
pub(super) struct ClientRequest {
    pub(super) request_id: String,
    value: Value,
    pub(super) requested_model: String,
    pub(super) resolved_model: String,
    pub(super) stream_id: Option<String>,
    pub(super) responses_lite: bool,
    client_supplied_service_tier: bool,
    responses_lite_candidates: Vec<String>,
    pub(super) response_affinity_key: Option<String>,
    pub(super) prompt_affinity_key: Option<String>,
    pub(super) background_kind: Option<&'static str>,
}

impl ClientRequest {
    pub(super) fn parse(
        runtime: &GatewayRuntime,
        key: &AuthenticatedKey,
        headers: &HeaderMap,
        payload: &[u8],
    ) -> Result<Self, GatewayFailure> {
        if payload.len() > super::MAX_WEBSOCKET_MESSAGE_BYTES {
            return Err(GatewayFailure::invalid_request(
                "WebSocket request is too large",
            ));
        }
        let mut value: Value = serde_json::from_slice(payload)
            .map_err(|_| GatewayFailure::invalid_request("request must be valid JSON"))?;
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
                let stream_id = stream_id.trim();
                if stream_id.is_empty()
                    || stream_id.len() > 256
                    || stream_id.chars().any(char::is_control)
                {
                    return Err(GatewayFailure::invalid_request(
                        "stream_id must be a valid non-empty string",
                    ));
                }
                Some(stream_id.to_string())
            }
            Some(_) => {
                return Err(GatewayFailure::invalid_request(
                    "stream_id must be a valid non-empty string",
                ));
            }
        };
        let requested_model = object
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .ok_or_else(|| GatewayFailure::invalid_request("model must be a non-empty string"))?
            .to_string();
        let client_supplied_service_tier = object.contains_key("service_tier");
        let background_kind = codex_background_request_kind(headers, &value);
        let request_id = crate::gateway::request::request_id();
        if let Some(kind) = background_kind {
            runtime.mark_request_origin(&request_id, kind);
        }
        let resolved_model = runtime
            .resolve_visible_model(key, &requested_model, WEBSOCKET_PROTOCOLS, now_ms())
            .ok_or_else(GatewayFailure::model_not_found)?;
        let responses_lite = headers
            .contains_key(crate::gateway::request::CODEX_RESPONSES_LITE_HEADER)
            || metadata_flag(&value, RESPONSES_LITE_METADATA_KEY);
        let responses_lite_candidates =
            runtime.codex_model_responses_lite_candidates(&resolved_model);
        let response_affinity_key = runtime
            .response_affinity_key(value.get("previous_response_id").and_then(Value::as_str));
        let client_context_id = client_context_fingerprint(headers);
        let prompt_affinity_key = runtime.prompt_affinity_key(
            &key.id,
            &resolved_model,
            value.get("prompt_cache_key").and_then(Value::as_str),
            client_context_id.as_deref(),
        );
        Ok(Self {
            request_id,
            value,
            requested_model,
            resolved_model,
            stream_id,
            responses_lite,
            client_supplied_service_tier,
            responses_lite_candidates,
            response_affinity_key,
            prompt_affinity_key,
            background_kind,
        })
    }

    pub(super) fn apply_service_tier_for_route(
        &mut self,
        runtime: &GatewayRuntime,
        route: &ExecutorRoute,
    ) {
        if !self.client_supplied_service_tier {
            self.value
                .as_object_mut()
                .expect("request object was validated before routing")
                .remove("service_tier");
        }
        apply_default_service_tier_if_missing(
            &mut self.value,
            runtime.model_service_tier_for_candidate(&route.candidate_id, &route.source_model),
        );
    }

    pub(super) fn service_tier(&self) -> DefaultServiceTier {
        crate::gateway::request::request_service_tier(&self.value)
    }

    pub(super) fn payload_for(&self, route: &ExecutorRoute) -> Result<String, GatewayFailure> {
        serde_json::to_string(&self.value_for(route))
            .map_err(|_| GatewayFailure::invalid_request("request could not be serialized"))
    }

    pub(super) fn http_payload(&self) -> Result<Vec<u8>, GatewayFailure> {
        let mut value = self.value.clone();
        let object = value
            .as_object_mut()
            .expect("request object was validated before routing");
        object.remove("type");
        object.remove("stream_id");
        object.insert("stream".to_string(), Value::Bool(true));
        serde_json::to_vec(&value)
            .map_err(|_| GatewayFailure::invalid_request("request could not be serialized"))
    }

    pub(super) fn reasoning_effort_for(&self, route: &ExecutorRoute) -> ReasoningEffortDiagnostics {
        ReasoningEffortDiagnostics::from_bodies(
            &self.value,
            &self.value_for(route),
            WireApi::Responses,
        )
    }

    fn value_for(&self, route: &ExecutorRoute) -> Value {
        let mut value = self.value.clone();
        let object = value
            .as_object_mut()
            .expect("request object was validated before routing");
        object.insert(
            "type".to_string(),
            Value::String("response.create".to_string()),
        );
        object.insert(
            "model".to_string(),
            Value::String(route.source_model.clone()),
        );
        if route.account_id.is_some() {
            crate::gateway::request::normalize_account_request(
                object,
                self.responses_lite_for(route),
            );
        }
        value
    }

    pub(super) fn responses_lite_for(&self, route: &ExecutorRoute) -> bool {
        self.responses_lite
            || route.account_id.as_deref().is_some_and(|candidate_id| {
                self.responses_lite_candidates
                    .iter()
                    .any(|id| id == candidate_id)
            })
    }

    pub(super) fn tool_use_for(&self, route: &ExecutorRoute) -> ToolUseDiagnostics {
        let client = crate::gateway::request::tool_use_diagnostics(&self.value);
        self.payload_for(route)
            .map(|payload| {
                crate::gateway::request::with_forwarded_tool_diagnostics(
                    &client,
                    payload.as_bytes(),
                )
            })
            .unwrap_or(client)
    }

    pub(super) fn has_previous_response_id(&self) -> bool {
        self.previous_response_id().is_some()
    }

    pub(super) fn previous_response_id(&self) -> Option<&str> {
        self.value
            .get("previous_response_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub(super) fn has_tool_call_output(&self) -> bool {
        crate::gateway::request::contains_tool_call_output(&self.value)
    }

    pub(super) fn drop_previous_response_id(&mut self) -> bool {
        let Some(object) = self.value.as_object_mut() else {
            return false;
        };
        if object.remove("previous_response_id").is_some() {
            self.response_affinity_key = None;
            true
        } else {
            false
        }
    }

    pub(super) fn recover_invalid_encrypted_content(&mut self) -> bool {
        let mut attempted = false;
        crate::gateway::request::try_recover_encrypted_content(&mut self.value, &mut attempted)
    }

    pub(super) fn repair_custom_tool_item_ids(&mut self) -> bool {
        crate::protocol::repair_custom_tool_item_ids(&mut self.value)
    }
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
