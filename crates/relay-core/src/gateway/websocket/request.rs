use super::{request_scope, ExecutorRoute, GatewayFailure};
use crate::error_codes;
use crate::gateway::request::{RequestToolPolicy, ServiceTierPolicy};
use crate::scheduler::rotation::SharedRequestBudget;
use crate::usage::ReasoningEffortDiagnostics;
use crate::{DefaultServiceTier, GatewayRuntime, ToolUseDiagnostics, WireApi};
use axum::http::HeaderMap;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

mod continuation;
mod parse;

pub(super) struct PreparedPayload {
    pub(super) payload: String,
    pub(super) reasoning_effort: ReasoningEffortDiagnostics,
    pub(super) tool_use: ToolUseDiagnostics,
}

#[derive(Clone)]
pub(super) struct ClientRequest {
    local_key_id: String,
    pub(super) tool_policy: RequestToolPolicy,
    pub(super) request_id: String,
    pub(super) budget: SharedRequestBudget,
    request_body: Value,
    pub(super) requested_model: String,
    pub(super) resolved_model: String,
    pub(super) stream_id: Option<String>,
    pub(super) responses_lite: bool,
    pub(in crate::gateway::websocket) service_tier_policy: ServiceTierPolicy,
    pub(super) response_affinity_key: Option<String>,
    pub(super) requires_affinity_owner: bool,
    pub(super) has_unpaired_tool_output: bool,
    pub(super) prompt_affinity_key: Option<String>,
    pub(super) client_context_id: Option<String>,
    pub(super) background_kind: Option<&'static str>,
}

impl ClientRequest {
    pub(super) fn account_retained_input(&self) {
        self.budget
            .retain_input_bytes(crate::gateway::request_body::retained_request_bytes(
                &self.request_body,
            ));
    }

    pub(super) fn apply_service_tier(&mut self) {
        self.service_tier_policy.prepare_for_candidate(
            &mut self.request_body,
            self.service_tier_policy.selected_tier(),
            WireApi::Responses,
        );
    }

    pub(super) fn service_tier(&self) -> DefaultServiceTier {
        self.service_tier_policy.effective_tier(
            &self.request_body,
            self.service_tier_policy.selected_tier(),
            WireApi::Responses,
        )
    }

    pub(super) fn service_tier_exclusions(
        &self,
        runtime: &GatewayRuntime,
        key: &crate::runtime::AuthenticatedKey,
    ) -> Result<HashSet<String>, GatewayFailure> {
        self.compatible_routes(runtime, key)
            .map(|(excluded, _)| excluded)
    }

    pub(super) fn compatible_routes(
        &self,
        runtime: &GatewayRuntime,
        key: &crate::runtime::AuthenticatedKey,
    ) -> Result<(HashSet<String>, BTreeMap<String, ExecutorRoute>), GatewayFailure> {
        use crate::gateway::execution::compatibility::{incompatible_routes, RouteCompatibility};
        let started = Instant::now();
        let mut request = self.request_body.clone();
        if let Some(object) = request.as_object_mut() {
            object.remove("type");
            object.remove("stream_id");
        }
        let (excluded, admitted, error) = incompatible_routes(RouteCompatibility {
            runtime,
            key,
            model: &self.resolved_model,
            client: WireApi::Responses,
            request: &request,
            stream: true,
            tier_policy: &self.service_tier_policy,
            now_ms: crate::unix_time_ms(),
        });
        if admitted.is_empty() {
            if let Some(error) = error.filter(|error| {
                !(self.has_previous_response_id()
                    && error.code() == error_codes::ADAPTER_CONTINUATION_MISMATCH)
            }) {
                return Err(self.reject_adapter_request(runtime, error, started));
            }
        }
        Ok((excluded, admitted))
    }

    fn reject_adapter_request(
        &self,
        runtime: &GatewayRuntime,
        error: crate::AdapterError,
        started: Instant,
    ) -> GatewayFailure {
        use crate::gateway::execution::compatibility::{
            record_adapter_rejection, AdapterRejection,
        };

        record_adapter_rejection(
            runtime,
            AdapterRejection {
                request_id: &self.request_id,
                attempt: 0,
                local_key_id: &self.local_key_id,
                requested_model: &self.requested_model,
                resolved_model: &self.resolved_model,
                client_context_id: self.client_context_id.as_deref(),
                wire_api: WireApi::Responses,
                transport: crate::UsageTransport::Websocket,
                service_tier: self.service_tier(),
            },
            &error,
            started.elapsed().as_millis() as u64,
        );
        GatewayFailure::adapter(error)
    }

    #[cfg(test)]
    pub(super) fn payload_for(&self, route: &ExecutorRoute) -> Result<String, GatewayFailure> {
        let (filtered_request, _) = self.filtered_value_for(route)?;
        serde_json::to_string(&filtered_request)
            .map_err(|_| GatewayFailure::invalid_request("request could not be serialized"))
    }

    pub(super) fn observed_payload_for(
        &self,
        runtime: &GatewayRuntime,
        route: &mut ExecutorRoute,
        client_headers: &HeaderMap,
        prepared: &crate::runtime::PreparedAuthorization,
    ) -> Result<PreparedPayload, GatewayFailure> {
        let started = Instant::now();
        runtime
            .validate_continuation_route(
                route,
                &self.local_key_id,
                &self.request_body,
                crate::unix_time_ms(),
            )
            // No in-flight usage event exists until response.create is sent.
            // Record this final guard failure here, like admission rejection.
            .map_err(|error| self.reject_adapter_request(runtime, error, started))?;
        if !GatewayRuntime::route_supports_service_tier(route, self.service_tier()) {
            return Err(GatewayFailure::unsupported_service_tier());
        }
        let (mut filtered_request, mut tool_use) = self.filtered_value_for(route)?;
        runtime.apply_request_timezone(&route.candidate_id, &mut filtered_request);
        let turn_scope = request_scope(
            &self.local_key_id,
            client_headers,
            route.account_id.as_deref(),
            &route.source_model,
            route.upstream_url.as_str(),
        );
        runtime.guard_codex_payload(&mut filtered_request, turn_scope.as_ref(), prepared);
        let reasoning_effort = ReasoningEffortDiagnostics::from_bodies(
            &self.request_body,
            &filtered_request,
            WireApi::Responses,
            WireApi::Responses,
        );
        self.tool_policy
            .observe_cache_context(runtime, route, &mut filtered_request);
        let payload = serde_json::to_string(&filtered_request)
            .map_err(|_| GatewayFailure::invalid_request("request could not be serialized"))?;
        route.observe_websocket_payload(&payload);
        tool_use.capture_upstream_context(&filtered_request);
        Ok(PreparedPayload {
            payload,
            reasoning_effort,
            tool_use,
        })
    }

    fn filtered_value_for(
        &self,
        route: &ExecutorRoute,
    ) -> Result<(Value, ToolUseDiagnostics), GatewayFailure> {
        if route.account_id.is_some()
            && route.account_transport == crate::runtime::AccountTransport::NativeResponses
            && self.responses_lite
            && self.request_body.get("parallel_tool_calls") == Some(&Value::Bool(true))
        {
            return Err(GatewayFailure::invalid_request(
                "selected Responses Lite transport does not support parallel_tool_calls: true",
            ));
        }
        let mut filtered_request = self.request_for_route(route);
        let mut policy = self.tool_policy.clone();
        policy
            .apply(&mut filtered_request)
            .map_err(GatewayFailure::invalid_request)?;
        Ok((filtered_request, policy.diagnostics))
    }

    pub(super) fn http_payload(&self) -> Result<Vec<u8>, GatewayFailure> {
        let mut request_body = self.request_body.clone();
        let request_object = request_body
            .as_object_mut()
            .expect("request object was validated before routing");
        request_object.remove("type");
        request_object.remove("stream_id");
        request_object.insert("stream".to_string(), Value::Bool(true));
        serde_json::to_vec(&request_body)
            .map_err(|_| GatewayFailure::invalid_request("request could not be serialized"))
    }

    pub(super) fn reasoning_effort_for(&self, route: &ExecutorRoute) -> ReasoningEffortDiagnostics {
        ReasoningEffortDiagnostics::from_bodies(
            &self.request_body,
            &self.request_for_route(route),
            WireApi::Responses,
            WireApi::Responses,
        )
    }

    fn request_for_route(&self, route: &ExecutorRoute) -> Value {
        let mut routed_request = self.request_body.clone();
        let routed_object = routed_request
            .as_object_mut()
            .expect("request object was validated before routing");
        routed_object.insert(
            "type".to_string(),
            Value::String("response.create".to_string()),
        );
        routed_object.insert(
            "model".to_string(),
            Value::String(route.source_model.clone()),
        );
        let responses_lite = self.responses_lite;
        if route.account_id.is_some()
            && route.account_transport == crate::runtime::AccountTransport::NativeResponses
        {
            crate::gateway::request::normalize_account_request(routed_object, responses_lite);
            crate::gateway::request::apply_account_prompt_cache_key(
                routed_object,
                &self.local_key_id,
                &self.resolved_model,
                self.client_context_id.as_deref(),
            );
        }
        routed_request
    }

    pub(super) fn tool_use_for(&self, route: &ExecutorRoute) -> ToolUseDiagnostics {
        self.filtered_value_for(route)
            .map(|(_, diagnostics)| diagnostics)
            .unwrap_or_else(|_| self.tool_policy.diagnostics.clone())
    }
}
