use super::super::super::errors::{api_error, AttemptFailure};
use super::super::super::request::{
    account_endpoint_url, responses_lite_parallel_tool_calls_valid, AccountEndpoint,
    RequestToolPolicy, ServiceTierPolicy,
};
use super::super::super::response::route_error_origin;
use crate::error_codes;
use crate::protocol::AdapterError;
use crate::runtime::{AccountTransport, AuthenticatedKey, DefaultServiceTier, ExecutorRoute};
use crate::scheduler::RoutingDiagnostics;
use crate::usage::{ReasoningEffortDiagnostics, ToolUseDiagnostics};
use crate::{ErrorOrigin, GatewayRuntime};
use axum::body::Body;
use axum::http::{HeaderValue, Response, StatusCode};
use serde_json::Value;

pub(super) enum AccountPrepare {
    Continue,
    Respond(Response<Body>),
    Ready(Box<PreparedAccountAttempt>),
}

pub(super) struct PreparedAccountAttempt {
    pub(super) route: ExecutorRoute,
    pub(super) basis_points_route: bool,
    pub(super) route_responses_lite: Option<HeaderValue>,
    pub(super) selected_error_origin: ErrorOrigin,
    pub(super) upstream_url: url::Url,
    pub(super) upstream_body: Value,
    pub(super) request_body: Vec<u8>,
    pub(super) reasoning_effort: ReasoningEffortDiagnostics,
    pub(super) tool_use: ToolUseDiagnostics,
}

pub(super) struct AccountPrepareInput<'a> {
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) request: &'a mut Value,
    pub(super) resolved_model: &'a str,
    pub(super) endpoint: AccountEndpoint,
    pub(super) responses_lite: &'a Option<HeaderValue>,
    pub(super) rewrite_model: bool,
    pub(super) automatic_responses_lite: bool,
    pub(super) service_tier_policy: &'a ServiceTierPolicy,
    pub(super) tool_policy: &'a mut RequestToolPolicy,
    pub(super) candidate_id: &'a str,
    pub(super) half_open_probe: bool,
    pub(super) diagnostics: RoutingDiagnostics,
    pub(super) client_context_id: &'a Option<String>,
    pub(super) basis_points_relay_retry_parameter: Option<&'static str>,
    pub(super) last_failure: &'a mut Option<AttemptFailure>,
    pub(super) last_adapter_error: &'a mut Option<crate::protocol::AdapterError>,
}

/// Turn one reserved account into a serialized upstream attempt. A route that
/// cannot carry this endpoint continues the loop; a client-shaped body returns
/// immediately.
pub(super) fn prepare_account_attempt(input: AccountPrepareInput<'_>) -> AccountPrepare {
    let AccountPrepareInput {
        runtime,
        key,
        request,
        resolved_model,
        endpoint,
        responses_lite,
        rewrite_model,
        automatic_responses_lite,
        service_tier_policy,
        tool_policy,
        candidate_id,
        half_open_probe,
        diagnostics,
        client_context_id,
        basis_points_relay_retry_parameter,
        last_failure,
        last_adapter_error,
        ..
    } = input;
    let Some(mut route) = runtime.executor_route(
        candidate_id,
        resolved_model,
        &key.scope_snapshot(),
        &[crate::WireApi::Responses],
        false,
    ) else {
        return AccountPrepare::Continue;
    };
    if route.account_id.is_none() {
        return AccountPrepare::Continue;
    }
    let selected_service_tier = service_tier_policy.select_for_model(runtime, &route.source_model);
    service_tier_policy.prepare_for_candidate(
        request,
        selected_service_tier,
        crate::WireApi::Responses,
    );
    route.half_open_probe = half_open_probe;
    route.routing = Some(diagnostics);
    route.client_context_id = client_context_id.clone();
    route.service_tier = service_tier_policy.effective_tier(
        request,
        selected_service_tier,
        crate::WireApi::Responses,
    );
    runtime.use_native_responses_when_speed_requested(&mut route);
    let basis_points_route = route.account_transport == AccountTransport::ExcelBasisPoints;
    if basis_points_route {
        if let Some(step) = reject_account_basis_points(
            request,
            service_tier_policy,
            selected_service_tier,
            responses_lite.is_some(),
            endpoint,
            last_adapter_error,
        ) {
            return step;
        }
    }
    let route_responses_lite = account_responses_lite_header(
        responses_lite,
        automatic_responses_lite,
        runtime,
        resolved_model,
        route.account_id.as_deref(),
    );
    let selected_error_origin = route_error_origin(&route);
    let Some(upstream_url) =
        account_upstream_url(&route, endpoint, basis_points_route, last_failure)
    else {
        return AccountPrepare::Continue;
    };
    let mut upstream_body = match prepare_account_upstream_body(
        request,
        &route.source_model,
        rewrite_model,
        endpoint,
        route_responses_lite.is_some(),
        basis_points_route,
    ) {
        Ok(body) => body,
        Err(step) => return step,
    };
    // Only the native Responses wake path has the provider contract for
    // `tool_search`. Compact and alpha/search are separate account endpoints
    // and must keep their ordinary full catalog.
    if let Some(step) = apply_account_tool_policy(
        tool_policy,
        &mut upstream_body,
        endpoint == AccountEndpoint::Wake && !basis_points_route,
    ) {
        return step;
    }
    if basis_points_route {
        match rewrite_account_basis_points_body(
            &upstream_body,
            basis_points_relay_retry_parameter,
            last_adapter_error,
        ) {
            Ok(prepared) => upstream_body = prepared,
            Err(step) => return step,
        }
    }
    let reasoning_effort =
        ReasoningEffortDiagnostics::from_bodies(request, &upstream_body, crate::WireApi::Responses);
    let Ok(request_body) = serde_json::to_vec(&upstream_body) else {
        return AccountPrepare::Respond(api_error(
            StatusCode::BAD_REQUEST,
            "request body could not be serialized",
            error_codes::INVALID_REQUEST,
        ));
    };
    let tool_use = tool_policy.diagnostics.clone();
    AccountPrepare::Ready(Box::new(PreparedAccountAttempt {
        route,
        basis_points_route,
        route_responses_lite,
        selected_error_origin,
        upstream_url,
        upstream_body,
        request_body,
        reasoning_effort,
        tool_use,
    }))
}

fn reject_account_basis_points(
    request: &Value,
    service_tier_policy: &ServiceTierPolicy,
    selected_service_tier: DefaultServiceTier,
    responses_lite: bool,
    endpoint: AccountEndpoint,
    last_adapter_error: &mut Option<AdapterError>,
) -> Option<AccountPrepare> {
    let error = super::super::compatibility::basis_points_admission_error(
        request,
        request.get("stream").and_then(Value::as_bool) == Some(true),
        service_tier_policy,
        selected_service_tier,
        responses_lite,
        endpoint != AccountEndpoint::Wake,
    )?;
    *last_adapter_error = Some(error);
    Some(AccountPrepare::Continue)
}

fn account_responses_lite_header(
    responses_lite: &Option<HeaderValue>,
    automatic_responses_lite: bool,
    runtime: &GatewayRuntime,
    resolved_model: &str,
    account_id: Option<&str>,
) -> Option<HeaderValue> {
    responses_lite.clone().or_else(|| {
        (automatic_responses_lite
            && account_id.is_some_and(|candidate_id| {
                runtime
                    .codex_model_responses_lite_candidates(resolved_model)
                    .iter()
                    .any(|id| id == candidate_id)
            }))
        .then(|| HeaderValue::from_static("true"))
    })
}

fn account_upstream_url(
    route: &ExecutorRoute,
    endpoint: AccountEndpoint,
    basis_points_route: bool,
    last_failure: &mut Option<AttemptFailure>,
) -> Option<url::Url> {
    if basis_points_route {
        return Some(route.upstream_url.clone());
    }
    let upstream_url = account_endpoint_url(route.upstream_url.clone(), endpoint);
    if upstream_url.is_none() {
        *last_failure = Some(AttemptFailure::invalid_request());
    }
    upstream_url
}

#[allow(clippy::result_large_err)]
fn prepare_account_upstream_body(
    request: &Value,
    source_model: &str,
    rewrite_model: bool,
    endpoint: AccountEndpoint,
    responses_lite: bool,
    basis_points_route: bool,
) -> Result<Value, AccountPrepare> {
    let mut upstream_body = request.clone();
    if rewrite_model {
        upstream_body
            .as_object_mut()
            .unwrap()
            .insert("model".to_string(), Value::String(source_model.to_string()));
    }
    if responses_lite && !basis_points_route {
        if let Some(object) = upstream_body.as_object_mut() {
            if !responses_lite_parallel_tool_calls_valid(object) {
                return Err(AccountPrepare::Respond(api_error(
                    StatusCode::BAD_REQUEST,
                    "responses Lite requires parallel_tool_calls to be a boolean",
                    error_codes::INVALID_REQUEST,
                )));
            }
            if endpoint == AccountEndpoint::Compact {
                crate::gateway::request::normalize_compact_account_request(object, true);
            } else {
                crate::gateway::request::normalize_account_request(object, true);
            }
        }
    }
    if basis_points_route {
        if let Some(object) = upstream_body.as_object_mut() {
            crate::gateway::request::normalize_basis_points_request(object);
        }
    }
    Ok(upstream_body)
}

fn apply_account_tool_policy(
    tool_policy: &mut RequestToolPolicy,
    upstream_body: &mut Value,
    allow_deferred_tool_search: bool,
) -> Option<AccountPrepare> {
    tool_policy
        .apply_value(upstream_body, allow_deferred_tool_search)
        .err()
        .map(|message| {
            AccountPrepare::Respond(api_error(
                StatusCode::BAD_REQUEST,
                message,
                error_codes::INVALID_REQUEST,
            ))
        })
}

#[allow(clippy::result_large_err)]
fn rewrite_account_basis_points_body(
    upstream_body: &Value,
    retry_parameter: Option<&'static str>,
    last_adapter_error: &mut Option<AdapterError>,
) -> Result<Value, AccountPrepare> {
    match super::super::basis_points::prepare_upstream(upstream_body, retry_parameter) {
        Ok(prepared) => Ok(prepared),
        Err(error) if error.is_route_incompatible() => {
            *last_adapter_error = Some(error);
            Err(AccountPrepare::Continue)
        }
        Err(error) => Err(AccountPrepare::Respond(
            super::super::request::adapter_error_response(error),
        )),
    }
}
