use super::super::request::{RequestToolPolicy, ServiceTierPolicy};
use crate::runtime::AuthenticatedKey;
use crate::scheduler::rotation::SharedRequestBudget;
use crate::{GatewayRuntime, WireApi};
use axum::http::{HeaderMap, HeaderValue};
use serde_json::Value;
use std::sync::Arc;

pub(super) struct RequestExecution {
    pub(super) tool_policy: RequestToolPolicy,
    pub(super) runtime: Arc<GatewayRuntime>,
    pub(super) key: AuthenticatedKey,
    pub(super) request: Value,
    pub(super) service_tier_policy: ServiceTierPolicy,
    pub(super) requested_model: String,
    pub(super) resolved_model: String,
    pub(super) stream: bool,
    pub(super) request_id: String,
    pub(super) forwarded_headers: HeaderMap,
    pub(super) client_context_id: Option<String>,
    pub(super) response_affinity_key: Option<String>,
    pub(super) requires_affinity_owner: bool,
    pub(super) wire_api: WireApi,
    pub(super) responses_lite: Option<HeaderValue>,
    pub(super) allow_previous_response_reset: bool,
    pub(super) attempt_offset: u16,
    pub(super) budget: SharedRequestBudget,
}

mod completion;
mod dispatch;
mod drive;
mod execute;
mod failure;
mod prelude;
mod prepare;
mod recovery;
mod retry;
mod selection;
mod stream;
mod translate;

pub(super) use execute::execute_request;
pub(super) use recovery::{
    adapter_error_response, adapter_error_response_for_origin, recover_stale_tool_history,
    should_wait_for_candidate_availability,
};
pub(super) use retry::{
    basis_points_relay_error_response, handle_basis_points_relay_retry, mark_adapter_failure,
    BasisPointsRelayRetryContext,
};

#[cfg(test)]
mod tests;
