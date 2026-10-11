use super::{ExecutorRoute, GatewayFailure};
use crate::gateway::request::{RequestToolPolicy, ServiceTierPolicy};
use crate::runtime::AccountTransport;
use crate::scheduler::rotation::SharedRequestBudget;
use crate::usage::ReasoningEffortDiagnostics;
use crate::{DefaultServiceTier, GatewayRuntime, ToolUseDiagnostics, WireApi};
use serde_json::Value;
use std::collections::HashSet;

mod continuation;
mod parse;

#[derive(Clone)]
pub(super) struct ClientRequest {
    pub(super) tool_policy: RequestToolPolicy,
    pub(super) request_id: String,
    pub(super) budget: SharedRequestBudget,
    request_body: Value,
    pub(super) requested_model: String,
    pub(super) resolved_model: String,
    pub(super) stream_id: Option<String>,
    pub(super) responses_lite: bool,
    pub(in crate::gateway::websocket) service_tier_policy: ServiceTierPolicy,
    responses_lite_candidates: Vec<String>,
    pub(super) response_affinity_key: Option<String>,
    pub(super) requires_affinity_owner: bool,
    pub(super) has_unpaired_tool_output: bool,
    pub(super) prompt_affinity_key: Option<String>,
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
        let routes = runtime.configured_executor_routes(
            key,
            &self.resolved_model,
            &[WireApi::Responses],
            false,
        );
        let excluded = routes
            .iter()
            .filter(|route| {
                !GatewayRuntime::route_supports_service_tier(
                    route,
                    self.service_tier_policy.selected_tier(),
                ) || (route.account_transport == AccountTransport::ExcelBasisPoints
                    && self.service_tier_policy.rejects_basis_points_client_speed())
            })
            .map(|route| route.candidate_id.clone())
            .collect::<HashSet<_>>();
        if !routes.is_empty() && excluded.len() == routes.len() {
            return Err(GatewayFailure::unsupported_service_tier());
        }
        Ok(excluded)
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
    ) -> Result<String, GatewayFailure> {
        if !GatewayRuntime::route_supports_service_tier(route, self.service_tier()) {
            return Err(GatewayFailure::unsupported_service_tier());
        }
        let (mut filtered_request, _) = self.filtered_value_for(route)?;
        runtime.apply_request_timezone(&route.candidate_id, &mut filtered_request);
        let payload = serde_json::to_string(&filtered_request)
            .map_err(|_| GatewayFailure::invalid_request("request could not be serialized"))?;
        self.tool_policy
            .observe_cache_context(runtime, route, &filtered_request);
        Ok(payload)
    }

    fn filtered_value_for(
        &self,
        route: &ExecutorRoute,
    ) -> Result<(Value, ToolUseDiagnostics), GatewayFailure> {
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
        let responses_lite = self.responses_lite_for(route);
        if responses_lite {
            crate::gateway::request::normalize_responses_lite_request(routed_object);
        }
        if route.account_id.is_some() {
            crate::gateway::request::normalize_account_request(routed_object, responses_lite);
        }
        routed_request
    }

    pub(super) fn responses_lite_for(&self, route: &ExecutorRoute) -> bool {
        self.responses_lite
            || route.account_id.as_deref().is_some_and(|candidate_id| {
                self.responses_lite_candidates
                    .iter()
                    .any(|candidate_model_id| candidate_model_id == candidate_id)
            })
    }

    pub(super) fn tool_use_for(&self, route: &ExecutorRoute) -> ToolUseDiagnostics {
        self.filtered_value_for(route)
            .map(|(_, diagnostics)| diagnostics)
            .unwrap_or_else(|_| self.tool_policy.diagnostics.clone())
    }
}
