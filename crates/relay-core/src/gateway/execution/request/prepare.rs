use super::super::super::request::ServiceTierPolicy;
use super::prelude::*;
use super::recovery::adapter_error_response;
use crate::protocol::BridgedCompaction;
use crate::runtime::{DefaultServiceTier, ExecutorRoute};
use crate::scheduler::RoutingDiagnostics;
use crate::usage::ToolUseDiagnostics;

pub(super) enum RequestPrepare {
    Continue,
    Respond(Response<Body>),
    Ready(Box<PreparedRequestAttempt>),
}

pub(super) struct PreparedRequestAttempt {
    pub(super) route: ExecutorRoute,
    pub(super) source_model: String,
    pub(super) selected_error_origin: ErrorOrigin,
    pub(super) account_route: bool,
    pub(super) basis_points_route: bool,
    pub(super) route_responses_lite: Option<HeaderValue>,
    pub(super) client_stream: bool,
    pub(super) stream: bool,
    pub(super) summarize: bool,
    pub(super) adapter_request: PreparedAdapterRequest,
    pub(super) basis_points_request: Option<Value>,
    pub(super) reasoning_effort: ReasoningEffortDiagnostics,
    pub(super) adapter_is_passthrough: bool,
    pub(super) request_body: Vec<u8>,
    pub(super) tool_use: ToolUseDiagnostics,
}

pub(super) struct RequestPrepareInput<'a> {
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) request: &'a mut Value,
    pub(super) resolved_model: &'a str,
    pub(super) request_id: &'a str,
    pub(super) client_wire_api: WireApi,
    pub(super) stream: bool,
    pub(super) responses_lite: &'a Option<HeaderValue>,
    pub(super) automatic_responses_lite: bool,
    pub(super) service_tier_policy: &'a ServiceTierPolicy,
    pub(super) tool_policy: &'a mut RequestToolPolicy,
    pub(super) candidate_id: &'a str,
    pub(super) half_open_probe: bool,
    pub(super) diagnostics: RoutingDiagnostics,
    pub(super) client_context_id: &'a Option<String>,
    pub(super) client_transport: crate::UsageTransport,
    pub(super) basis_points_relay_retry_parameter: Option<&'static str>,
    pub(super) last_adapter_error: &'a mut Option<AdapterError>,
}

/// Build the upstream body for one reserved route. An incompatible route
/// continues the attempt loop; a client-shaped body returns immediately.
pub(super) fn prepare_request_attempt(input: RequestPrepareInput<'_>) -> RequestPrepare {
    let RequestPrepareInput {
        runtime,
        key,
        request,
        resolved_model,
        request_id,
        client_wire_api,
        stream,
        responses_lite,
        automatic_responses_lite,
        service_tier_policy,
        tool_policy,
        candidate_id,
        half_open_probe,
        diagnostics,
        client_context_id,
        client_transport,
        basis_points_relay_retry_parameter,
        last_adapter_error,
    } = input;
    let allowed_protocols = candidate_protocols(client_wire_api);
    let Some(mut route) = runtime.executor_route(
        candidate_id,
        resolved_model,
        &key.scope_snapshot(),
        allowed_protocols,
        stream,
    ) else {
        return RequestPrepare::Continue;
    };
    let selected_service_tier = service_tier_policy.select_for_model(runtime, &route.source_model);
    service_tier_policy.prepare_for_candidate(request, selected_service_tier, client_wire_api);
    route.half_open_probe = half_open_probe;
    route.routing = Some(diagnostics);
    route.client_context_id = client_context_id.clone();
    route.client_transport = client_transport;
    route.service_tier =
        service_tier_policy.effective_tier(request, selected_service_tier, client_wire_api);
    let selected_error_origin = route_error_origin(&route);
    let source_model = route.source_model.clone();
    debug_assert_eq!(client_wire_api, route.client_wire_api);
    let account_route = route.account_id.is_some();
    runtime.use_native_responses_when_speed_requested(&mut route);
    let basis_points_route = route.account_transport == AccountTransport::ExcelBasisPoints;
    if basis_points_route {
        if let Some(step) = reject_basis_points_admission(
            request,
            stream,
            service_tier_policy,
            selected_service_tier,
            responses_lite.is_some(),
            last_adapter_error,
        ) {
            return step;
        }
    }
    let route_responses_lite = route_responses_lite_header(
        client_wire_api,
        responses_lite,
        automatic_responses_lite,
        runtime,
        resolved_model,
        route.account_id.as_deref(),
    );
    if let Some(step) = normalize_prepared_responses_lite(request, route_responses_lite.is_some()) {
        return step;
    }
    let previous = match load_prepared_continuation(runtime, &route, &key.id, request) {
        Ok(previous) => previous,
        Err(response) => return RequestPrepare::Respond(response),
    };
    let client_stream = stream;
    let compaction = match prepare_attempt_compaction(
        client_wire_api,
        route.adapter.is_passthrough(),
        request,
        last_adapter_error,
    ) {
        Ok(compaction) => compaction,
        Err(step) => return step,
    };
    let summarize = compaction.summarize();
    let stream = if summarize { false } else { stream };
    if summarize && client_stream {
        if let Some(resolved) = runtime.executor_route(
            &route.candidate_id,
            resolved_model,
            &key.scope_snapshot(),
            allowed_protocols,
            false,
        ) {
            route.upstream_url = resolved.upstream_url;
            route.upstream_headers = resolved.upstream_headers;
        }
    }
    let mut adapter_request = match translate_prepared_request(
        &route,
        client_wire_api,
        &compaction,
        request,
        &source_model,
        stream,
        previous,
        request_id,
        last_adapter_error,
    ) {
        Ok(adapter_request) => adapter_request,
        Err(step) => return step,
    };
    if account_route {
        normalize_prepared_account_body(&mut adapter_request, route_responses_lite.is_some());
    }
    if let Some(step) = apply_prepared_tool_policy(tool_policy, &mut adapter_request) {
        return step;
    }
    let basis_points_request = basis_points_route.then(|| adapter_request.upstream_body().clone());
    if basis_points_route {
        if let Some(step) = rewrite_basis_points_body(
            &mut adapter_request,
            basis_points_relay_retry_parameter,
            last_adapter_error,
        ) {
            return step;
        }
    }
    let reasoning_effort = ReasoningEffortDiagnostics::from_bodies(
        request,
        adapter_request.upstream_body(),
        client_wire_api,
    );
    let adapter_is_passthrough = adapter_request.is_passthrough();
    let Ok(request_body) = serde_json::to_vec(adapter_request.upstream_body()) else {
        return RequestPrepare::Respond(api_error(
            StatusCode::BAD_REQUEST,
            "request body could not be serialized",
            error_codes::INVALID_REQUEST,
        ));
    };
    let tool_use = tool_policy.diagnostics.clone();
    RequestPrepare::Ready(Box::new(PreparedRequestAttempt {
        route,
        source_model,
        selected_error_origin,
        account_route,
        basis_points_route,
        route_responses_lite,
        client_stream,
        stream,
        summarize,
        adapter_request,
        basis_points_request,
        reasoning_effort,
        adapter_is_passthrough,
        request_body,
        tool_use,
    }))
}

fn reject_basis_points_admission(
    request: &Value,
    stream: bool,
    service_tier_policy: &ServiceTierPolicy,
    selected_service_tier: DefaultServiceTier,
    responses_lite: bool,
    last_adapter_error: &mut Option<AdapterError>,
) -> Option<RequestPrepare> {
    let error = super::super::compatibility::basis_points_admission_error(
        request,
        stream,
        service_tier_policy,
        selected_service_tier,
        responses_lite,
        false,
    )?;
    *last_adapter_error = Some(error);
    Some(RequestPrepare::Continue)
}

fn route_responses_lite_header(
    client_wire_api: WireApi,
    responses_lite: &Option<HeaderValue>,
    automatic_responses_lite: bool,
    runtime: &GatewayRuntime,
    resolved_model: &str,
    account_id: Option<&str>,
) -> Option<HeaderValue> {
    (client_wire_api == WireApi::Responses)
        .then(|| {
            super::super::responses_lite_header(
                responses_lite,
                automatic_responses_lite,
                runtime,
                resolved_model,
                account_id,
            )
        })
        .flatten()
}

fn normalize_prepared_responses_lite(request: &mut Value, enabled: bool) -> Option<RequestPrepare> {
    if !enabled {
        return None;
    }
    let Some(object) = request.as_object_mut() else {
        return Some(RequestPrepare::Respond(api_error(
            StatusCode::BAD_REQUEST,
            "request body must be a JSON object",
            error_codes::INVALID_REQUEST,
        )));
    };
    if !responses_lite_parallel_tool_calls_valid(object) {
        return Some(RequestPrepare::Respond(api_error(
            StatusCode::BAD_REQUEST,
            "responses Lite requires parallel_tool_calls to be a boolean",
            error_codes::INVALID_REQUEST,
        )));
    }
    // Apply the shared Lite contract before adapter translation. This
    // keeps native, bridged, OAuth, and API-source routes identical.
    normalize_responses_lite_request(object);
    None
}

#[allow(clippy::result_large_err)]
fn load_prepared_continuation(
    runtime: &GatewayRuntime,
    route: &ExecutorRoute,
    key_id: &str,
    request: &Value,
) -> Result<Option<crate::MessagesBridgeState>, Response<Body>> {
    if !route.adapter.uses_local_continuation_state() {
        return Ok(None);
    }
    let Some(response_id) = previous_response_id(request) else {
        return Ok(None);
    };
    runtime
        .load_messages_bridge_state(key_id, response_id, &route.candidate_id, now_ms())
        .map(Some)
        .map_err(adapter_error_response)
}

#[allow(clippy::result_large_err)]
fn prepare_attempt_compaction(
    client_wire_api: WireApi,
    passthrough: bool,
    request: &Value,
    last_adapter_error: &mut Option<AdapterError>,
) -> Result<BridgedCompaction, RequestPrepare> {
    if client_wire_api != WireApi::Responses || passthrough {
        return Ok(BridgedCompaction::Unchanged);
    }
    match crate::protocol::prepare_bridged_compaction(request) {
        Ok(compaction) => Ok(compaction),
        Err(error) => Err(stop_for_adapter_error(error, last_adapter_error)),
    }
}

#[allow(clippy::too_many_arguments, clippy::result_large_err)]
fn translate_prepared_request(
    route: &ExecutorRoute,
    client_wire_api: WireApi,
    compaction: &BridgedCompaction,
    request: &Value,
    source_model: &str,
    stream: bool,
    previous: Option<crate::MessagesBridgeState>,
    request_id: &str,
    last_adapter_error: &mut Option<AdapterError>,
) -> Result<PreparedAdapterRequest, RequestPrepare> {
    match route.adapter.prepare_request(AdapterRequestContext {
        client_wire_api,
        request: compaction.request(request),
        model: source_model,
        stream,
        reasoning_mode: route.reasoning_mode,
        cache_write_ttl: route.cache_write_ttl,
        previous,
        response_scope: &route.candidate_id,
        response_id_seed: request_id,
    }) {
        Ok(request) => Ok(request),
        Err(error) => Err(stop_for_adapter_error(error, last_adapter_error)),
    }
}

fn normalize_prepared_account_body(
    adapter_request: &mut PreparedAdapterRequest,
    responses_lite: bool,
) {
    let upstream_body = adapter_request.upstream_body_mut();
    let Value::Object(object) = upstream_body else {
        unreachable!("request object was validated before execution")
    };
    normalize_account_request(object, responses_lite);
}

fn apply_prepared_tool_policy(
    tool_policy: &mut RequestToolPolicy,
    adapter_request: &mut PreparedAdapterRequest,
) -> Option<RequestPrepare> {
    tool_policy
        .apply_adapter(adapter_request)
        .err()
        .map(|message| {
            RequestPrepare::Respond(api_error(
                StatusCode::BAD_REQUEST,
                message,
                error_codes::INVALID_REQUEST,
            ))
        })
}

fn rewrite_basis_points_body(
    adapter_request: &mut PreparedAdapterRequest,
    retry_parameter: Option<&'static str>,
    last_adapter_error: &mut Option<AdapterError>,
) -> Option<RequestPrepare> {
    match super::super::basis_points::prepare_upstream(
        adapter_request.upstream_body(),
        retry_parameter,
    ) {
        Ok(prepared) => {
            *adapter_request.upstream_body_mut() = prepared;
            None
        }
        Err(error) => Some(stop_for_adapter_error(error, last_adapter_error)),
    }
}

fn stop_for_adapter_error(
    error: AdapterError,
    last_adapter_error: &mut Option<AdapterError>,
) -> RequestPrepare {
    if error.is_route_incompatible() {
        *last_adapter_error = Some(error);
        RequestPrepare::Continue
    } else {
        RequestPrepare::Respond(adapter_error_response(error))
    }
}
