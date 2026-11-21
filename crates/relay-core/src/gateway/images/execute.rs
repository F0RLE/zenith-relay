use super::super::errors::{AttemptFailure, api_error_with_origin_and_category, cooldown_error};
use super::super::now_ms;
use super::super::request::request_id;
use super::{IMAGE_PROTOCOLS, ImageEndpoint, PreparedImageRequest};
use crate::runtime::AuthenticatedKey;
use crate::{ErrorOrigin, GatewayRuntime};
use attempt::SelectedImageRoute;
use attempt::run_selected_attempt;
use axum::body::Body;
use axum::http::{Response, StatusCode};
use std::collections::HashSet;
use std::sync::Arc;

mod attempt;
mod response;
mod stream;

pub(super) enum ImageAttemptStep {
    Retry(AttemptFailure),
    Respond(Response<Body>),
}

pub(super) async fn execute_prepared(
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    prepared: PreparedImageRequest,
    endpoint: ImageEndpoint,
    started: tokio::time::Instant,
    ingress: crate::runtime::RequestIngressTiming,
) -> Response<Body> {
    let request_id = request_id();
    let budget = crate::scheduler::rotation::SharedRequestBudget::for_incoming_request(
        runtime.request_dispatch_budget(),
    );
    let scope = runtime.track_request_usage_from(&request_id, &budget, started, ingress);
    let response =
        execute_attempts(&runtime, &key, &prepared, endpoint, &request_id, &budget).await;
    let streaming = response.status().is_success()
        && response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
    scope.finish(response.status().as_u16(), streaming);
    response
}

async fn execute_attempts(
    runtime: &Arc<GatewayRuntime>,
    key: &AuthenticatedKey,
    prepared: &PreparedImageRequest,
    endpoint: ImageEndpoint,
    request_id: &str,
    budget: &crate::scheduler::rotation::SharedRequestBudget,
) -> Response<Body> {
    budget.retain_input_bytes(
        prepared
            .input_images
            .iter()
            .map(String::capacity)
            .fold(
                prepared.raw_body.len().saturating_add(
                    crate::gateway::request_body::retained_object_bytes(&prepared.fields),
                ),
                usize::saturating_add,
            )
            .saturating_add(prepared.mask_image.as_ref().map_or(0, String::capacity))
            .saturating_mul(3)
            .saturating_add(16 * 1024),
    );
    let mut tried = HashSet::new();
    let mut last_failure = None;
    let mut last_failure_origin = ErrorOrigin::Relay;

    loop {
        budget.configure_retry_window(runtime.route_recovery_window_ms(), false);
        if !budget.can_dispatch() {
            break;
        }
        let selection_started = runtime.diagnostics_enabled().then(std::time::Instant::now);
        let Some((selected, lease)) = runtime
            .select_and_reserve_image_with_budget(
                key,
                &prepared.resolved_model,
                IMAGE_PROTOCOLS,
                &tried,
                now_ms(),
                budget,
            )
            .await
        else {
            break;
        };
        tried.insert(selected.candidate_id.clone());
        let Some(mut route) = runtime.image_executor_route(
            &selected.candidate_id,
            &prepared.resolved_model,
            &key.scope_snapshot(),
            IMAGE_PROTOCOLS,
        ) else {
            continue;
        };
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.client_context_id = prepared.client_context_id.clone();
        if let Some(started) = selection_started {
            runtime.record_request_selection(request_id, started);
        }
        let lease = Arc::new(lease);
        let route_origin = super::super::response::route_error_origin(&route);
        match run_selected_attempt(SelectedImageRoute {
            runtime,
            key,
            prepared,
            endpoint,
            request_id,
            budget,
            lease: &lease,
            route,
        })
        .await
        {
            ImageAttemptStep::Retry(failure) => {
                last_failure = Some(failure);
                last_failure_origin = route_origin;
            }
            ImageAttemptStep::Respond(response) => return response,
        }
    }

    if let Some(error) = super::super::errors::admission_error(budget) {
        return error;
    }
    let failure = last_failure.unwrap_or_else(AttemptFailure::no_candidate);
    if failure.status == StatusCode::TOO_MANY_REQUESTS
        && let Some((retry_at, reason)) = runtime.all_applicable_cooldown(
            key,
            &prepared.resolved_model,
            IMAGE_PROTOCOLS,
            &HashSet::new(),
            None,
            now_ms(),
            crate::scheduler::rotation::RotationOperation::Image,
        )
    {
        return cooldown_error(
            retry_at,
            Some(&failure),
            reason.is_rate_limit(),
            last_failure_origin,
        );
    }
    let origin = last_failure_origin.for_category(failure.category);
    api_error_with_origin_and_category(
        failure.status,
        failure.message,
        failure.category,
        failure.category,
        origin,
        Some(request_id),
    )
}
