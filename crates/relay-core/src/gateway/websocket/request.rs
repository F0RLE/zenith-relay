use super::{
    now_ms, AuthenticatedKey, ExecutorRoute, GatewayFailure, RESPONSES_LITE_METADATA_KEY,
    WEBSOCKET_PROTOCOLS,
};
use crate::error_codes;
use crate::gateway::continuation;
use crate::gateway::request::{
    client_context_fingerprint, codex_background_request_kind, is_managed_codex_client,
    repair_legacy_responses_call_ids, ServiceTierPolicy,
};
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
    service_tier_policy: ServiceTierPolicy,
    responses_lite_candidates: Vec<String>,
    pub(super) response_affinity_key: Option<String>,
    pub(super) requires_affinity_owner: bool,
    pub(super) has_unpaired_tool_output: bool,
    pub(super) prompt_affinity_key: Option<String>,
    pub(super) background_kind: Option<&'static str>,
    pub(super) wait_for_candidate_availability: bool,
}

impl ClientRequest {
    pub(super) fn parse(
        runtime: &GatewayRuntime,
        key: &AuthenticatedKey,
        headers: &HeaderMap,
        payload: &[u8],
    ) -> Result<Self, GatewayFailure> {
        Self::parse_on_connection(runtime, key, headers, payload, None)
    }

    pub(super) fn parse_on_connection(
        runtime: &GatewayRuntime,
        key: &AuthenticatedKey,
        headers: &HeaderMap,
        payload: &[u8],
        connection_response: Option<(&str, &str)>,
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
        let service_tier_policy = if is_managed_codex_client(headers) {
            ServiceTierPolicy::pool_owned(&value)
        } else {
            ServiceTierPolicy::client_owned(&value)
        };
        let managed_codex_client = is_managed_codex_client(headers);
        // Preserve client eligibility with the request. The live control is
        // checked in the connection/retry loop so an in-flight wait can be
        // disabled without reconnecting the client.
        let wait_for_candidate_availability = managed_codex_client;
        let background_kind = codex_background_request_kind(headers, &value);
        let request_id = crate::gateway::request::request_id();
        if let Some(kind) = background_kind {
            runtime.mark_request_origin(&request_id, kind);
        }
        let resolved_model = runtime
            .resolve_visible_model(key, &requested_model, WEBSOCKET_PROTOCOLS, now_ms())
            .or_else(|| {
                (managed_codex_client && runtime.chatgpt_retry_until_available())
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
            request_id,
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
            wait_for_candidate_availability,
        })
    }

    pub(super) fn apply_service_tier_for_route(
        &mut self,
        runtime: &GatewayRuntime,
        route: &ExecutorRoute,
    ) {
        self.service_tier_policy.prepare_for_candidate(
            &mut self.value,
            self.service_tier_policy
                .select_for_model(runtime, &route.source_model),
            WireApi::Responses,
        );
    }

    pub(super) fn service_tier(
        &self,
        runtime: &GatewayRuntime,
        route: &ExecutorRoute,
    ) -> DefaultServiceTier {
        self.service_tier_policy.effective_tier(
            &self.value,
            self.service_tier_policy
                .select_for_model(runtime, &route.source_model),
            WireApi::Responses,
        )
    }

    pub(super) fn payload_for(&self, route: &ExecutorRoute) -> Result<String, GatewayFailure> {
        serde_json::to_string(&self.value_for(route))
            .map_err(|_| GatewayFailure::invalid_request("request could not be serialized"))
    }

    pub(super) fn native_replay_value(&self) -> Value {
        self.value.clone()
    }

    /// Replace an owner-bound opaque continuation with materialized native
    /// history before selecting a replacement candidate.
    pub(super) fn replay_native_continuation(
        &mut self,
        runtime: &GatewayRuntime,
        local_key_id: &str,
        owner_candidate_id: &str,
        owner_model: &str,
    ) -> Result<bool, GatewayFailure> {
        let Some(previous_response_id) = self.previous_response_id() else {
            return Ok(false);
        };
        let Some(replay) = runtime.load_native_responses_replay(
            local_key_id,
            previous_response_id,
            owner_candidate_id,
            now_ms(),
        ) else {
            return Ok(false);
        };
        let replayed = match replay.replay_request(&self.value, owner_model, true) {
            Ok(value) => value,
            Err(error) if error.code() == error_codes::ADAPTER_CONTINUATION_MISMATCH => {
                return Ok(false)
            }
            Err(_) => {
                return Err(GatewayFailure::invalid_request(
                    "native continuation state is invalid",
                ))
            }
        };
        self.value = replayed;
        self.response_affinity_key = None;
        self.requires_affinity_owner = false;
        self.has_unpaired_tool_output = false;
        Ok(true)
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

    pub(super) fn replay_missing_response(
        &mut self,
        runtime: &GatewayRuntime,
        local_key_id: &str,
        route: &ExecutorRoute,
        attempted: &mut bool,
    ) -> Result<bool, GatewayFailure> {
        if *attempted || !self.requires_affinity_owner {
            return Ok(false);
        }
        if !self.replay_native_continuation(
            runtime,
            local_key_id,
            &route.candidate_id,
            &route.source_model,
        )? {
            return Ok(false);
        }
        *attempted = true;
        Ok(true)
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
        let responses_lite = self.responses_lite_for(route);
        if responses_lite {
            crate::gateway::request::normalize_responses_lite_request(object);
        }
        if route.account_id.is_some() {
            crate::gateway::request::normalize_account_request(object, responses_lite);
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

    pub(super) const fn has_unpaired_tool_output(&self) -> bool {
        self.has_unpaired_tool_output
    }

    pub(super) fn drop_previous_response_id(
        &mut self,
        runtime: &GatewayRuntime,
        local_key_id: &str,
    ) -> bool {
        if continuation::drop_materialized_previous_response_id(
            runtime,
            local_key_id,
            &mut self.value,
            &self.resolved_model,
            now_ms(),
        ) {
            self.response_affinity_key = None;
            self.requires_affinity_owner = false;
            self.has_unpaired_tool_output = false;
            true
        } else {
            false
        }
    }

    pub(super) fn repair_custom_tool_item_ids(&mut self) -> bool {
        crate::protocol::repair_custom_tool_item_ids(&mut self.value)
    }

    pub(super) fn repair_function_item_ids(&mut self) -> bool {
        crate::protocol::repair_call_prefixed_function_item_ids(&mut self.value)
    }

    pub(super) fn repair_message_item_ids(&mut self) -> bool {
        crate::protocol::remove_item_prefixed_message_ids(&mut self.value)
    }

    pub(super) fn repair_legacy_call_ids(&mut self) -> bool {
        if !repair_legacy_responses_call_ids(&mut self.value) {
            return false;
        }
        self.has_unpaired_tool_output =
            !crate::gateway::request::unpaired_tool_output_ids(&self.value).is_empty();
        self.requires_affinity_owner =
            self.has_previous_response_id() || self.has_unpaired_tool_output;
        true
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
