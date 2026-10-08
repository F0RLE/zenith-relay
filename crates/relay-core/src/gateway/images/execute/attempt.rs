use super::super::super::errors::{
    api_error, apply_failure_state, current_failure_state, settle_attempt_failure, AttemptFailure,
};
use super::super::super::now_ms;
use super::super::super::response::emit_usage;
use super::super::account::{
    build_account_request, direct_request_body, image_endpoint_url, ImageAttempt,
};
use super::super::{ImageEndpoint, PreparedImageRequest};
use super::response::handle_collected_image;
use super::ImageAttemptStep;
use crate::error_codes;
use crate::runtime::{AuthenticatedKey, CandidateLease, ExecutorRoute};
use crate::scheduler::rotation::SharedRequestBudget;
use crate::GatewayRuntime;
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use std::time::Instant;

pub(super) struct SelectedImageRoute<'a> {
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) prepared: &'a PreparedImageRequest,
    pub(super) endpoint: ImageEndpoint,
    pub(super) request_id: &'a str,
    pub(super) budget: &'a SharedRequestBudget,
    pub(super) lease: &'a CandidateLease,
    pub(super) route: ExecutorRoute,
}

pub(super) async fn run_selected_attempt(
    selected_image_route: SelectedImageRoute<'_>,
) -> ImageAttemptStep {
    let SelectedImageRoute {
        runtime,
        key,
        prepared,
        endpoint,
        request_id,
        budget,
        lease,
        mut route,
    } = selected_image_route;
    let account_route = route.account_id.is_some();
    let upstream_url = if account_route {
        Some(route.upstream_url.clone())
    } else {
        image_endpoint_url(route.upstream_url.clone(), endpoint)
    };
    let Some(upstream_url) = upstream_url else {
        return ImageAttemptStep::Retry(AttemptFailure::invalid_request());
    };
    let request_body = if account_route {
        match serde_json::to_vec(&build_account_request(
            prepared,
            endpoint,
            &route.source_model,
        )) {
            Ok(body) => body,
            Err(_) => {
                return ImageAttemptStep::Respond(api_error(
                    StatusCode::BAD_REQUEST,
                    "image request could not be serialized",
                    error_codes::INVALID_REQUEST,
                ));
            }
        }
    } else {
        direct_request_body(prepared, &route.source_model)
    };

    let started = Instant::now();
    let upstream = runtime
        .request_client(&route.candidate_id)
        .post(upstream_url)
        .header(
            CONTENT_TYPE,
            if account_route {
                HeaderValue::from_static("application/json")
            } else {
                prepared.content_type.clone()
            },
        )
        .header(
            ACCEPT,
            if account_route || prepared.stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .body(request_body);
    let upstream_result = runtime
        .send_authorized_request(
            &route.candidate_id,
            upstream,
            None,
            None,
            Some(budget),
            Some(lease),
        )
        .await;
    let attempt = u16::from(budget.dispatches());
    let upstream = match upstream_result {
        Ok(upstream) => {
            route.account_token_generation = upstream.account_token_generation;
            upstream.response
        }
        Err(error) => {
            let observed = ImageAttempt {
                request_id,
                attempt,
                key,
                route: &route,
                prepared,
                started,
            };
            let uncertain = error.execution_certainty()
                == crate::scheduler::rotation::ExecutionCertainty::Unknown;
            let exhausted = matches!(
                error,
                crate::runtime::AuthorizedRequestError::DispatchBudgetExhausted
            );
            let failure = AttemptFailure::authorized_request(error);
            if uncertain || exhausted {
                if uncertain {
                    lease.settle_rotation_unknown(now_ms());
                }
                emit_usage(
                    runtime,
                    observed.event(false, failure.status, Some(failure.category.to_string())),
                );
                return ImageAttemptStep::Respond(api_error(
                    failure.status,
                    failure.message,
                    failure.category,
                ));
            }
            let failure_state = settle_attempt_failure(
                runtime,
                lease,
                &prepared.resolved_model,
                &failure,
                &HeaderMap::new(),
            );
            let mut event =
                observed.event(false, failure.status, Some(failure.category.to_string()));
            apply_failure_state(&mut event, failure_state);
            emit_usage(runtime, event);
            return ImageAttemptStep::Retry(failure);
        }
    };
    let observed = ImageAttempt {
        request_id,
        attempt,
        key,
        route: &route,
        prepared,
        started,
    };

    let status = upstream.status();
    let response_headers = upstream.headers().clone();
    let Ok(bytes) = crate::transport::collect(upstream).await else {
        lease.settle_rotation_unknown(now_ms());
        let failure = AttemptFailure::upstream_response_body_failure();
        let failure_state =
            current_failure_state(runtime, &route.candidate_id, &prepared.resolved_model);
        let mut event = observed.event(false, failure.status, Some(failure.category.to_string()));
        apply_failure_state(&mut event, failure_state);
        emit_usage(runtime, event);
        return ImageAttemptStep::Retry(failure);
    };
    handle_collected_image(
        runtime,
        lease,
        &route,
        prepared,
        endpoint,
        account_route,
        observed,
        status,
        response_headers,
        bytes,
    )
}
