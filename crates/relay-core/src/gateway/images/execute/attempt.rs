use super::super::super::errors::{
    AttemptFailure, api_error, apply_failure_state, current_failure_state, settle_attempt_failure,
};
use super::super::super::now_ms;
use super::super::super::response::emit_usage;
use super::super::account::{
    ImageAttempt, direct_account_request_body, direct_request_body, image_endpoint_url,
};
use super::super::{ImageEndpoint, PreparedImageRequest};
use super::ImageAttemptStep;
use super::response::handle_collected_image;
use crate::GatewayRuntime;
use crate::runtime::{
    AccountTransport, AuthenticatedKey, AuthorizationIdentityPolicy, CandidateLease, ExecutorRoute,
};
use crate::scheduler::rotation::SharedRequestBudget;
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use std::sync::Arc;
use std::time::Instant;

pub(super) struct SelectedImageRoute<'a> {
    pub(super) runtime: &'a Arc<GatewayRuntime>,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) prepared: &'a PreparedImageRequest,
    pub(super) endpoint: ImageEndpoint,
    pub(super) request_id: &'a str,
    pub(super) budget: &'a SharedRequestBudget,
    pub(super) lease: &'a Arc<CandidateLease>,
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
    let upstream_url = image_endpoint_url(route.upstream_url.clone(), endpoint);
    let Some(upstream_url) = upstream_url else {
        return ImageAttemptStep::Retry(AttemptFailure::invalid_request());
    };
    let request_body = if account_route {
        direct_account_request_body(prepared, endpoint, &route.source_model)
    } else if prepared.content_type.to_str().is_ok_and(|value| {
        value
            .to_ascii_lowercase()
            .starts_with("multipart/form-data")
    }) {
        match super::super::multipart::source_body(prepared, &route.source_model).await {
            Ok(body) => body,
            Err(()) => {
                return ImageAttemptStep::Respond(api_error(
                    StatusCode::BAD_REQUEST,
                    "multipart image upload is invalid",
                    crate::error_codes::INVALID_REQUEST,
                ));
            }
        }
    } else {
        direct_request_body(prepared, &route.source_model)
    };

    let started = Instant::now();
    let mut upstream = runtime
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
            if prepared.stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .body(request_body);
    if account_route {
        // Match the native Codex Images transport. Keep the same turn identity
        // across authorization recovery instead of generating one per send.
        upstream = upstream.header("x-codex-image-turn-id", request_id);
    }
    let authorization_event = ImageAttempt {
        request_id,
        attempt: u16::from(budget.dispatches()),
        key,
        route: &route,
        prepared,
        started,
    }
    .event(false, StatusCode::UNAUTHORIZED, None);
    let upstream_result = runtime
        .send_authorized_request_for_route(
            &route,
            upstream,
            crate::runtime::AuthorizationDispatch {
                client_version: None,
                identity_policy: if account_route
                    && route.account_transport == AccountTransport::ExcelBasisPoints
                {
                    AuthorizationIdentityPolicy::PreserveUpstream
                } else {
                    AuthorizationIdentityPolicy::RelayCodex
                },
                turn_scope: None,
                budget: Some(budget),
                lease: Some(lease),
                usage_event: Some((&authorization_event, started)),
            },
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
    if status.is_success() && prepared.stream {
        let event = observed.event(false, status, None);
        return ImageAttemptStep::Respond(super::stream::image_stream_response(
            runtime.clone(),
            lease.clone(),
            upstream,
            event,
            started,
            endpoint,
            prepared
                .fields
                .get("n")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(1),
        ));
    }
    let collected = crate::transport::collect_with_progress_limit(
        upstream,
        None,
        super::super::MAX_IMAGE_BODY_BYTES,
        |_| false,
    )
    .await;
    let bytes = match collected {
        Ok(bytes) => bytes,
        Err(partial) => {
            lease.settle_rotation_unknown(now_ms());
            let failure = AttemptFailure::upstream_response_body_failure();
            let failure_state =
                current_failure_state(runtime, &route.candidate_id, &prepared.resolved_model);
            let mut event =
                observed.event(false, failure.status, Some(failure.category.to_string()));
            super::super::super::response::populate_tokens(&mut event, &partial.bytes);
            if let Ok(envelope) =
                serde_json::from_slice::<super::super::native::ImageEnvelope<'_>>(&partial.bytes)
            {
                envelope.observe_usage(&mut event);
            }
            apply_failure_state(&mut event, failure_state);
            if let Some(routing) = event.routing.as_mut() {
                routing.attempt_outcome = Some(crate::AttemptOutcome::ExecutionUncertain);
            }
            emit_usage(runtime, event);
            return ImageAttemptStep::Respond(api_error(
                failure.status,
                failure.message,
                failure.category,
            ));
        }
    };
    handle_collected_image(
        runtime,
        lease,
        &route,
        prepared,
        observed,
        status,
        response_headers,
        bytes,
    )
}
