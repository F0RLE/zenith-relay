use super::super::super::request::ServiceTierPolicy;
use super::prelude::*;
use super::recovery::adapter_error_response;
use crate::protocol::BridgedCompaction;
use crate::runtime::{DefaultServiceTier, ExecutorRoute};
use crate::scheduler::RoutingDiagnostics;
use crate::usage::{RequestPreparationDiagnostics, ToolUseDiagnostics};

pub(super) enum RequestPrepare {
    Continue,
    Respond(Response<Body>),
    Ready(Box<PreparedRequestAttempt>),
}

pub(super) struct PreparedRequestAttempt {
    pub(super) preparation: RequestPreparationDiagnostics,
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
    pub(super) siwc_catalog_request: Option<Value>,
    pub(super) reasoning_effort: ReasoningEffortDiagnostics,
    pub(super) adapter_is_passthrough: bool,
    /// BPS is encoded only after catalog preparation and image resolution.
    pub(super) request_body: Option<Vec<u8>>,
    pub(super) tool_use: ToolUseDiagnostics,
}

pub(super) struct RequestPrepareInput<'a> {
    pub(super) route: ExecutorRoute,
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) request: &'a mut Value,
    pub(super) resolved_model: &'a str,
    pub(super) request_id: &'a str,
    pub(super) client_wire_api: WireApi,
    pub(super) stream: bool,
    pub(super) responses_lite: &'a Option<HeaderValue>,
    pub(super) service_tier_policy: &'a ServiceTierPolicy,
    pub(super) tool_policy: &'a mut RequestToolPolicy,
    pub(super) candidate_id: &'a str,
    pub(super) half_open_probe: bool,
    pub(super) diagnostics: RoutingDiagnostics,
    pub(super) client_context_id: &'a Option<String>,
    pub(super) client_transport: crate::UsageTransport,
    pub(super) last_adapter_error: &'a mut Option<AdapterError>,
}

/// Build the upstream body for one reserved route. An incompatible route
/// continues the attempt loop; a client-shaped body returns immediately.
pub(super) fn prepare_request_attempt(
    request_prepare_input: RequestPrepareInput<'_>,
) -> RequestPrepare {
    let RequestPrepareInput {
        mut route,
        runtime,
        key,
        request,
        resolved_model,
        request_id,
        client_wire_api,
        stream,
        responses_lite,
        service_tier_policy,
        tool_policy,
        candidate_id,
        half_open_probe,
        diagnostics,
        client_context_id,
        client_transport,
        last_adapter_error,
    } = request_prepare_input;
    let selected_service_tier = service_tier_policy.selected_tier();
    if !GatewayRuntime::route_supports_service_tier(&route, selected_service_tier) {
        *last_adapter_error = Some(AdapterError::parameter_unsupported_for("service_tier"));
        return RequestPrepare::Continue;
    }
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
    let basis_points_route = route.account_transport == AccountTransport::ExcelBasisPoints;
    let siwc_route = route.account_transport == AccountTransport::Siwc;
    if basis_points_route {
        if let Some(step) = reject_basis_points_admission(
            request,
            stream,
            service_tier_policy,
            selected_service_tier,
            last_adapter_error,
        ) {
            return step;
        }
    }
    // Lite item catalogs can arrive on BPS too, but serial execution is a
    // native transport restriction, not a property of the client item format.
    let route_responses_lite =
        if basis_points_route || siwc_route || client_wire_api != WireApi::Responses {
            None
        } else {
            responses_lite.clone()
        };
    let normalize_responses_lite = route_responses_lite.is_some()
        || (basis_points_route
            && client_wire_api == WireApi::Responses
            && responses_lite.is_some());
    if let Some(step) = validate_prepared_responses_lite(request, normalize_responses_lite) {
        return step;
    }
    if account_route
        && route_responses_lite.is_some()
        && request.get("parallel_tool_calls") == Some(&Value::Bool(true))
    {
        return RequestPrepare::Respond(api_error(
            StatusCode::BAD_REQUEST,
            "selected Responses Lite transport does not support parallel_tool_calls: true",
            error_codes::INVALID_REQUEST,
        ));
    }
    let previous_continuation =
        match load_prepared_continuation(runtime, &route, &key.id, request, request_id) {
            Ok(previous_continuation) => previous_continuation,
            Err(response) => return RequestPrepare::Respond(response),
        };
    let client_stream = stream;
    let compaction_plan = match prepare_attempt_compaction(
        client_wire_api,
        &route,
        request,
        request_id,
        last_adapter_error,
    ) {
        Ok(compaction_plan) => compaction_plan,
        Err(step) => return step,
    };
    let should_summarize = compaction_plan.summarize();
    let upstream_stream = if should_summarize { false } else { stream };
    if should_summarize && client_stream {
        runtime.set_route_streaming(&mut route, false);
    }
    let mut adapter_request = match translate_prepared_request(
        runtime,
        &route,
        client_wire_api,
        &compaction_plan,
        request,
        &source_model,
        upstream_stream,
        previous_continuation,
        request_id,
        last_adapter_error,
    ) {
        Ok(adapter_request) => adapter_request,
        Err(step) => return step,
    };
    if account_route && !siwc_route {
        normalize_prepared_account_body(
            &mut adapter_request,
            route_responses_lite.is_some(),
            &key.id,
            resolved_model,
            client_context_id.as_deref(),
        );
        runtime.apply_request_timezone(candidate_id, adapter_request.upstream_body_mut());
    }
    if let Some(step) = apply_prepared_tool_policy(tool_policy, &mut adapter_request) {
        return step;
    }
    let siwc_catalog_request = (siwc_route
        && !should_summarize
        && super::super::basis_points::discovery::native::eligible(
            adapter_request.upstream_body(),
        ))
    .then(|| adapter_request.upstream_body().clone());
    if siwc_route {
        if let Err(error) =
            crate::providers::chatgpt::prepare_siwc_request(adapter_request.upstream_body_mut())
        {
            return RequestPrepare::Respond(super::adapter_error_response(error, Some(request_id)));
        }
        // SIWC skips Codex normalization, not the shared account cache-key policy.
        crate::gateway::request::apply_account_prompt_cache_key(
            adapter_request
                .upstream_body_mut()
                .as_object_mut()
                .expect("validated SIWC request"),
            &key.id,
            resolved_model,
            client_context_id.as_deref(),
        );
    }
    let mut preparation = RequestPreparationDiagnostics::default();
    let basis_points_request = if basis_points_route {
        match rewrite_basis_points_body(
            &mut adapter_request,
            &mut preparation.schema_validation_ms,
            tool_policy.diagnostics.client_context.is_some(),
        ) {
            Ok(original) => Some(original),
            Err(error) => return stop_for_adapter_error(error, request_id, last_adapter_error),
        }
    } else {
        None
    };
    let reasoning_effort = if basis_points_route {
        ReasoningEffortDiagnostics::from_basis_points_bodies(
            request,
            adapter_request.upstream_body(),
            client_wire_api,
        )
    } else {
        ReasoningEffortDiagnostics::from_bodies(
            request,
            adapter_request.upstream_body(),
            client_wire_api,
            route.adapter.upstream_protocol(client_wire_api).wire_api(),
        )
    };
    let adapter_is_passthrough = adapter_request.is_passthrough();
    tool_policy.observe_cache_context(runtime, &mut route, adapter_request.upstream_body_mut());
    let Ok(request_body) = (!basis_points_route)
        .then(|| serde_json::to_vec(adapter_request.upstream_body()))
        .transpose()
    else {
        return RequestPrepare::Respond(api_error(
            StatusCode::BAD_REQUEST,
            "request body could not be serialized",
            error_codes::INVALID_REQUEST,
        ));
    };
    let tool_use = tool_policy.diagnostics.clone();
    RequestPrepare::Ready(Box::new(PreparedRequestAttempt {
        preparation,
        route,
        source_model,
        selected_error_origin,
        account_route,
        basis_points_route,
        route_responses_lite,
        client_stream,
        stream: upstream_stream,
        summarize: should_summarize,
        adapter_request,
        basis_points_request,
        siwc_catalog_request,
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
    last_adapter_error: &mut Option<AdapterError>,
) -> Option<RequestPrepare> {
    let error = super::super::compatibility::basis_points_admission_error(
        request,
        stream,
        service_tier_policy,
        selected_service_tier,
        false,
    )?;
    *last_adapter_error = Some(error);
    Some(RequestPrepare::Continue)
}

fn validate_prepared_responses_lite(request: &Value, enabled: bool) -> Option<RequestPrepare> {
    if !enabled {
        return None;
    }
    let Some(request_fields) = request.as_object() else {
        return Some(RequestPrepare::Respond(api_error(
            StatusCode::BAD_REQUEST,
            "request body must be a JSON object",
            error_codes::INVALID_REQUEST,
        )));
    };
    if !responses_lite_parallel_tool_calls_valid(request_fields) {
        return Some(RequestPrepare::Respond(api_error(
            StatusCode::BAD_REQUEST,
            "responses Lite requires parallel_tool_calls to be a boolean",
            error_codes::INVALID_REQUEST,
        )));
    }
    None
}

#[allow(clippy::result_large_err)]
fn load_prepared_continuation(
    runtime: &GatewayRuntime,
    route: &ExecutorRoute,
    key_id: &str,
    request: &Value,
    request_id: &str,
) -> Result<Option<crate::AdapterContinuationState>, Response<Body>> {
    if !route.adapter.uses_local_continuation_state() {
        return Ok(None);
    }
    runtime
        .load_adapter_continuation(route, key_id, request, now_ms())
        .map_err(|error| adapter_error_response(error, Some(request_id)))
}

#[allow(clippy::result_large_err)]
fn prepare_attempt_compaction(
    client_wire_api: WireApi,
    route: &ExecutorRoute,
    request: &Value,
    request_id: &str,
    last_adapter_error: &mut Option<AdapterError>,
) -> Result<BridgedCompaction, RequestPrepare> {
    let siwc_route = route.account_transport == AccountTransport::Siwc;
    if client_wire_api != WireApi::Responses || (route.adapter.is_passthrough() && !siwc_route) {
        return Ok(BridgedCompaction::Unchanged);
    }
    match crate::protocol::prepare_bridged_compaction(request) {
        Ok(compaction) => Ok(compaction),
        Err(error) if siwc_route => Err(RequestPrepare::Respond(adapter_error_response(
            error,
            Some(request_id),
        ))),
        Err(error) => Err(stop_for_adapter_error(
            error,
            request_id,
            last_adapter_error,
        )),
    }
}

#[allow(clippy::too_many_arguments, clippy::result_large_err)]
fn translate_prepared_request(
    runtime: &GatewayRuntime,
    route: &ExecutorRoute,
    client_wire_api: WireApi,
    compaction: &BridgedCompaction,
    request: &Value,
    source_model: &str,
    stream: bool,
    previous_bridge_state: Option<crate::AdapterContinuationState>,
    request_id: &str,
    last_adapter_error: &mut Option<AdapterError>,
) -> Result<PreparedAdapterRequest, RequestPrepare> {
    let capabilities = runtime
        .model_capabilities(source_model)
        .for_route(route.route_capability.as_ref());
    let prepared = route.adapter.prepare_request_with_capabilities(
        AdapterRequestContext {
            client_wire_api,
            request: compaction.request(request),
            model: source_model,
            stream,
            reasoning_mode: route.reasoning_mode,
            cache_write_ttl: route.cache_write_ttl,
            previous: previous_bridge_state,
            response_scope: &route.candidate_id,
            response_id_seed: request_id,
        },
        &capabilities,
    );
    match prepared {
        Ok(request) => Ok(request),
        Err(error) => Err(stop_for_adapter_error(
            error,
            request_id,
            last_adapter_error,
        )),
    }
}

fn normalize_prepared_account_body(
    adapter_request: &mut PreparedAdapterRequest,
    responses_lite: bool,
    local_key_id: &str,
    model: &str,
    client_context_id: Option<&str>,
) {
    let upstream_body = adapter_request.upstream_body_mut();
    let Value::Object(upstream_fields) = upstream_body else {
        unreachable!("request object was validated before execution")
    };
    normalize_account_request(upstream_fields, responses_lite);
    crate::gateway::request::apply_account_prompt_cache_key(
        upstream_fields,
        local_key_id,
        model,
        client_context_id,
    );
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
    schema_validation_ms: &mut Option<u64>,
    diagnostics_enabled: bool,
) -> Result<Value, AdapterError> {
    // The original is needed by response validation, but need not be copied.
    let original = std::mem::take(adapter_request.upstream_body_mut());
    let prepared = super::super::basis_points::prepare_request_with_timing(
        &original,
        schema_validation_ms,
        diagnostics_enabled,
    )?;
    *adapter_request.upstream_body_mut() = prepared;
    Ok(original)
}

fn stop_for_adapter_error(
    error: AdapterError,
    request_id: &str,
    last_adapter_error: &mut Option<AdapterError>,
) -> RequestPrepare {
    if error.is_route_incompatible() {
        *last_adapter_error = Some(error);
        RequestPrepare::Continue
    } else {
        RequestPrepare::Respond(adapter_error_response(error, Some(request_id)))
    }
}
