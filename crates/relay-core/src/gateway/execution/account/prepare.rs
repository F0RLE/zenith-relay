use super::super::super::errors::{AttemptFailure, api_error};
use super::super::super::request::{
    AccountEndpoint, RequestToolPolicy, ServiceTierPolicy, account_endpoint_url,
    responses_lite_parallel_tool_calls_valid,
};
use super::super::super::response::route_error_origin;
use crate::error_codes;
use crate::protocol::AdapterError;
use crate::runtime::{AccountTransport, AuthenticatedKey, DefaultServiceTier, ExecutorRoute};
use crate::scheduler::RoutingDiagnostics;
use crate::usage::{ReasoningEffortDiagnostics, RequestPreparationDiagnostics, ToolUseDiagnostics};
use crate::{ErrorOrigin, GatewayRuntime};
use axum::body::Body;
use axum::http::{HeaderValue, Response, StatusCode};
use serde_json::Value;
use std::borrow::Cow;

pub(super) enum AccountPrepare {
    Continue,
    Respond(Response<Body>),
    Ready(Box<PreparedAccountAttempt>),
}

pub(super) struct PreparedAccountAttempt {
    pub(super) preparation: RequestPreparationDiagnostics,
    pub(super) bps_catalog: super::super::basis_points::RequestCatalog,
    pub(super) route: ExecutorRoute,
    pub(super) basis_points_route: bool,
    pub(super) route_responses_lite: Option<HeaderValue>,
    pub(super) selected_error_origin: ErrorOrigin,
    pub(super) upstream_url: url::Url,
    pub(super) upstream_body: Value,
    /// BPS is encoded only after catalog preparation and image resolution.
    pub(super) request_body: Option<Vec<u8>>,
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
    pub(super) service_tier_policy: &'a ServiceTierPolicy,
    pub(super) tool_policy: &'a mut RequestToolPolicy,
    pub(super) candidate_id: &'a str,
    pub(super) half_open_probe: bool,
    pub(super) diagnostics: RoutingDiagnostics,
    pub(super) client_context_id: &'a Option<String>,
    pub(super) request_id: &'a str,
    pub(super) last_failure: &'a mut Option<AttemptFailure>,
    pub(super) last_adapter_error: &'a mut Option<crate::protocol::AdapterError>,
}

/// Turn one reserved account into a serialized upstream attempt. A route that
/// cannot carry this endpoint continues the loop; a client-shaped body returns
/// immediately.
pub(super) fn prepare_account_attempt(
    account_prepare_input: AccountPrepareInput<'_>,
) -> AccountPrepare {
    let AccountPrepareInput {
        runtime,
        key,
        request,
        resolved_model,
        endpoint,
        responses_lite,
        rewrite_model,
        service_tier_policy,
        tool_policy,
        candidate_id,
        half_open_probe,
        diagnostics,
        client_context_id,
        request_id,
        last_failure,
        last_adapter_error,
        ..
    } = account_prepare_input;
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
    let selected_service_tier = service_tier_policy.selected_tier();
    if !GatewayRuntime::route_supports_service_tier(&route, selected_service_tier) {
        *last_adapter_error = Some(AdapterError::parameter_unsupported_for("service_tier"));
        return AccountPrepare::Continue;
    }
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
    let basis_points_route = route.account_transport == AccountTransport::ExcelBasisPoints;
    let siwc_route = route.account_transport == AccountTransport::Siwc;
    if siwc_route && endpoint == AccountEndpoint::AlphaSearch {
        return AccountPrepare::Respond(super::super::request::adapter_error_response(
            AdapterError::parameter_unsupported_for("endpoint"),
            Some(request_id),
        ));
    }
    if basis_points_route
        && let Some(step) = reject_account_basis_points(
            request,
            service_tier_policy,
            selected_service_tier,
            endpoint,
            last_adapter_error,
        )
    {
        return step;
    }
    // BPS accepts the item-level catalog without inheriting native Lite's
    // serial execution restriction.
    let route_responses_lite = if basis_points_route || siwc_route {
        None
    } else {
        responses_lite.clone()
    };
    let normalize_responses_lite =
        route_responses_lite.is_some() || (basis_points_route && responses_lite.is_some());
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
        normalize_responses_lite,
        basis_points_route,
    ) {
        Ok(upstream_response_body) => upstream_response_body,
        Err(step) => return step,
    };
    if !siwc_route && let Some(timezone) = runtime.request_timezone(candidate_id) {
        crate::request_timezone::rewrite_request_timezone(upstream_body.to_mut(), &timezone);
    }
    if siwc_route && endpoint == AccountEndpoint::Compact {
        let fields = upstream_body
            .to_mut()
            .as_object_mut()
            .expect("validated account request");
        if !super::super::super::compaction::prepare_routed_compaction_request(fields) {
            return AccountPrepare::Respond(super::super::request::adapter_error_response(
                AdapterError::invalid_request(),
                Some(request_id),
            ));
        }
        match crate::protocol::prepare_bridged_compaction(&upstream_body) {
            Ok(compaction) => {
                upstream_body = Cow::Owned(compaction.request(&upstream_body).clone())
            }
            Err(error) => {
                return AccountPrepare::Respond(super::super::request::adapter_error_response(
                    error,
                    Some(request_id),
                ));
            }
        }
    }
    // Only a text-summary bridge omits tools; ordinary account requests keep them.
    if let Some(step) = apply_account_tool_policy(tool_policy, &upstream_body) {
        return step;
    }
    if siwc_route {
        if let Err(error) = crate::providers::chatgpt::prepare_siwc_request(upstream_body.to_mut())
        {
            return AccountPrepare::Respond(super::super::request::adapter_error_response(
                error,
                Some(request_id),
            ));
        }
        crate::gateway::request::apply_account_prompt_cache_key(
            upstream_body
                .to_mut()
                .as_object_mut()
                .expect("validated SIWC request"),
            &key.id,
            resolved_model,
            client_context_id.as_deref(),
        );
    }
    let mut preparation = RequestPreparationDiagnostics::default();
    let mut bps_catalog = super::super::basis_points::RequestCatalog::default();
    if basis_points_route {
        match rewrite_account_basis_points_body(
            &upstream_body,
            request_id,
            last_adapter_error,
            &mut preparation.schema_validation_ms,
            tool_policy.diagnostics.client_context.is_some(),
            &mut bps_catalog,
        ) {
            Ok(prepared) => upstream_body = Cow::Owned(prepared),
            Err(step) => return step,
        }
    }
    let reasoning_effort = if basis_points_route {
        ReasoningEffortDiagnostics::from_basis_points_bodies(
            request,
            &upstream_body,
            crate::WireApi::Responses,
        )
    } else {
        ReasoningEffortDiagnostics::from_bodies(
            request,
            &upstream_body,
            crate::WireApi::Responses,
            crate::WireApi::Responses,
        )
    };
    let Ok(request_body) = (!basis_points_route)
        .then(|| serde_json::to_vec(&upstream_body))
        .transpose()
    else {
        return AccountPrepare::Respond(api_error(
            StatusCode::BAD_REQUEST,
            "request body could not be serialized",
            error_codes::INVALID_REQUEST,
        ));
    };
    let tool_use = tool_policy.diagnostics.clone();
    AccountPrepare::Ready(Box::new(PreparedAccountAttempt {
        preparation,
        bps_catalog,
        route,
        basis_points_route,
        route_responses_lite,
        selected_error_origin,
        upstream_url,
        upstream_body: upstream_body.into_owned(),
        request_body,
        reasoning_effort,
        tool_use,
    }))
}

fn reject_account_basis_points(
    request: &Value,
    service_tier_policy: &ServiceTierPolicy,
    selected_service_tier: DefaultServiceTier,
    endpoint: AccountEndpoint,
    last_adapter_error: &mut Option<AdapterError>,
) -> Option<AccountPrepare> {
    let error = super::super::compatibility::basis_points_admission_error(
        request,
        request.get("stream").and_then(Value::as_bool) == Some(true),
        service_tier_policy,
        selected_service_tier,
        endpoint != AccountEndpoint::Wake,
    )?;
    *last_adapter_error = Some(error);
    Some(AccountPrepare::Continue)
}

fn account_upstream_url(
    route: &ExecutorRoute,
    endpoint: AccountEndpoint,
    basis_points_route: bool,
    last_failure: &mut Option<AttemptFailure>,
) -> Option<url::Url> {
    if basis_points_route || route.account_transport == AccountTransport::Siwc {
        return Some(route.upstream_url.clone());
    }
    let upstream_url = account_endpoint_url(route.upstream_url.clone(), endpoint);
    if upstream_url.is_none() {
        *last_failure = Some(AttemptFailure::invalid_request());
    }
    upstream_url
}

#[allow(clippy::result_large_err)]
fn prepare_account_upstream_body<'a>(
    request: &'a Value,
    source_model: &str,
    rewrite_model: bool,
    endpoint: AccountEndpoint,
    responses_lite: bool,
    basis_points_route: bool,
) -> Result<Cow<'a, Value>, AccountPrepare> {
    // BPS creates its own wire history. Borrow the client body until a real
    // route-specific rewrite needs ownership instead of cloning it twice.
    let mut upstream_body = if basis_points_route {
        Cow::Borrowed(request)
    } else {
        Cow::Owned(request.clone())
    };
    if rewrite_model && request["model"].as_str() != Some(source_model) {
        upstream_body
            .to_mut()
            .as_object_mut()
            .unwrap()
            .insert("model".to_string(), Value::String(source_model.to_string()));
    }
    if responses_lite && let Some(object) = upstream_body.as_object() {
        if !responses_lite_parallel_tool_calls_valid(object) {
            return Err(AccountPrepare::Respond(api_error(
                StatusCode::BAD_REQUEST,
                "responses Lite requires parallel_tool_calls to be a boolean",
                error_codes::INVALID_REQUEST,
            )));
        }
        if !basis_points_route && object.get("parallel_tool_calls") == Some(&Value::Bool(true)) {
            return Err(AccountPrepare::Respond(api_error(
                StatusCode::BAD_REQUEST,
                "selected Responses Lite transport does not support parallel_tool_calls: true",
                error_codes::INVALID_REQUEST,
            )));
        }
        if !basis_points_route {
            let object = upstream_body.to_mut().as_object_mut().unwrap();
            if endpoint == AccountEndpoint::Compact {
                crate::gateway::request::normalize_compact_account_request(object, true);
            } else {
                crate::gateway::request::normalize_account_request(object, true);
            }
        }
    }
    // Empty context_management is omitted by BPS preparation itself.
    Ok(upstream_body)
}

fn apply_account_tool_policy(
    tool_policy: &mut RequestToolPolicy,
    upstream_body: &Value,
) -> Option<AccountPrepare> {
    tool_policy.apply_value(upstream_body).err().map(|message| {
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
    request_id: &str,
    last_adapter_error: &mut Option<AdapterError>,
    schema_validation_ms: &mut Option<u64>,
    diagnostics_enabled: bool,
    catalog: &mut super::super::basis_points::RequestCatalog,
) -> Result<Value, AccountPrepare> {
    match super::super::basis_points::prepare_request_with_catalog(
        upstream_body,
        schema_validation_ms,
        diagnostics_enabled,
        catalog,
    ) {
        Ok(prepared) => Ok(prepared),
        Err(error) if error.is_route_incompatible() => {
            *last_adapter_error = Some(error);
            Err(AccountPrepare::Continue)
        }
        Err(error) => Err(AccountPrepare::Respond(
            super::super::request::adapter_error_response(error, Some(request_id)),
        )),
    }
}

#[cfg(test)]
mod tests;
