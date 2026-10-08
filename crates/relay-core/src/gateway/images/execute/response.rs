use super::super::super::errors::{
    apply_failure_state, retryable_failure, settle_classified_failure,
    settle_image_capability_failure, settle_status_failure, AttemptFailure, TRANSIENT_COOLDOWN_MS,
};
use super::super::super::now_ms;
use super::super::super::response::{
    apply_usage, emit_usage, populate_tokens, proxy_json_response, proxy_response,
    proxy_sse_response, route_error_origin,
};
use super::super::account::{
    image_capability_unavailable, image_error_response, translate_account_response, ImageAttempt,
};
use super::super::{ImageEndpoint, PreparedImageRequest};
use super::ImageAttemptStep;
use crate::error_codes;
use crate::runtime::{CandidateLease, ExecutorRoute};
use crate::GatewayRuntime;
use crate::UsageEvent;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_collected_image(
    runtime: &GatewayRuntime,
    lease: &CandidateLease,
    route: &ExecutorRoute,
    prepared: &PreparedImageRequest,
    endpoint: ImageEndpoint,
    account_route: bool,
    observed: ImageAttempt<'_>,
    status: StatusCode,
    response_headers: HeaderMap,
    bytes: Vec<u8>,
) -> ImageAttemptStep {
    if !status.is_success() {
        let upstream_error =
            crate::usage::UpstreamErrorDetails::from_response_body(Some(status.as_u16()), &bytes);
        let mut failure = AttemptFailure::status_with_body(status, Some(&bytes));
        super::super::super::errors::apply_degraded_route_policy(runtime, &mut failure);
        let capability_failure = image_capability_unavailable(&bytes);
        if retryable_failure(status, failure.category, false) || capability_failure {
            let failure_state = if capability_failure {
                settle_image_capability_failure(
                    runtime,
                    lease,
                    &prepared.resolved_model,
                    TRANSIENT_COOLDOWN_MS,
                )
            } else {
                settle_status_failure(
                    runtime,
                    lease,
                    &prepared.resolved_model,
                    status,
                    failure.category,
                    &response_headers,
                    Some(&bytes),
                )
            };
            let mut event = observed.event(
                false,
                status,
                Some(if capability_failure {
                    error_codes::IMAGE_GENERATION_NOT_ENABLED.to_string()
                } else {
                    failure.category.to_string()
                }),
            );
            event.upstream_error = Some(upstream_error);
            apply_failure_state(&mut event, failure_state);
            emit_usage(runtime, event);
            return ImageAttemptStep::Retry(failure);
        }
        let mut event = observed.event(false, status, Some(failure.category.to_string()));
        populate_tokens(&mut event, &bytes);
        event.upstream_error = Some(upstream_error);
        emit_usage(runtime, event);
        let origin = route_error_origin(route).for_category(failure.category);
        let error_response = super::super::super::response::proxy_error_response(
            status,
            &response_headers,
            &bytes,
            origin,
            failure.category,
            Some(observed.request_id),
        );
        return ImageAttemptStep::Respond(error_response);
    }

    if !account_route {
        let mut event = observed.event(true, status, None);
        populate_tokens(&mut event, &bytes);
        finish_image_success(
            runtime,
            lease,
            &route.candidate_id,
            &prepared.resolved_model,
            event,
        );
        return ImageAttemptStep::Respond(if prepared.stream {
            proxy_sse_response(status, &response_headers, Body::from(bytes))
        } else {
            proxy_response(status, &response_headers, Body::from(bytes))
        });
    }

    let translated = match translate_account_response(
        &bytes,
        &prepared.response_format,
        endpoint.stream_prefix(),
    ) {
        Ok(translated) => translated,
        Err(failure) if failure.retryable => {
            if matches!(
                failure.category,
                error_codes::STREAM_INCOMPLETE
                    | error_codes::IMAGE_OUTPUT_MISSING
                    | error_codes::STREAM_INVALID
            ) {
                lease.settle_rotation_unknown(now_ms());
            }
            let failure_state = if failure.category == error_codes::IMAGE_GENERATION_NOT_ENABLED {
                settle_image_capability_failure(
                    runtime,
                    lease,
                    &prepared.resolved_model,
                    TRANSIENT_COOLDOWN_MS,
                )
            } else {
                settle_classified_failure(
                    runtime,
                    lease,
                    &prepared.resolved_model,
                    failure.status,
                    failure.category,
                    &response_headers,
                    failure.cooldown_hint,
                )
            };
            let mut event =
                observed.event(false, failure.status, Some(failure.category.to_string()));
            event.upstream_error = failure.upstream_error.map(|mut details| {
                details.http_status = Some(status.as_u16());
                *details
            });
            apply_failure_state(&mut event, failure_state);
            emit_usage(runtime, event);
            return ImageAttemptStep::Retry(AttemptFailure::classified_with_hint(
                failure.status,
                failure.category,
                failure.cooldown_hint,
            ));
        }
        Err(failure) => {
            lease.settle_rotation_terminal(now_ms());
            let mut event =
                observed.event(false, failure.status, Some(failure.category.to_string()));
            event.upstream_error = failure.upstream_error.clone().map(|mut details| {
                details.http_status = Some(status.as_u16());
                *details
            });
            emit_usage(runtime, event);
            return ImageAttemptStep::Respond(image_error_response(
                failure,
                route_error_origin(route),
                observed.request_id,
            ));
        }
    };
    let mut event = observed.event(true, status, None);
    if let Some(usage) = translated.usage.as_ref() {
        apply_usage(&mut event, usage);
    }
    finish_image_success(
        runtime,
        lease,
        &route.candidate_id,
        &prepared.resolved_model,
        event,
    );
    ImageAttemptStep::Respond(if prepared.stream {
        proxy_sse_response(status, &response_headers, Body::from(translated.stream))
    } else {
        proxy_json_response(status, &response_headers, Body::from(translated.json))
    })
}

fn finish_image_success(
    runtime: &GatewayRuntime,
    lease: &CandidateLease,
    candidate_id: &str,
    model: &str,
    mut event: UsageEvent,
) {
    let recovered =
        runtime.record_success_with_metrics(candidate_id, model, now_ms(), None, event.latency_ms);
    event.consecutive_failures = recovered.then_some(0);
    emit_usage(runtime, event);
    lease.settle_rotation_success(now_ms());
}
