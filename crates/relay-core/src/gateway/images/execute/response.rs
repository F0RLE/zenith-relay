use super::super::super::errors::{
    AttemptFailure, TRANSIENT_COOLDOWN_MS, api_error, apply_failure_state, retryable_route_failure,
    route_forbids_fallback, settle_image_capability_failure, settle_status_failure,
};
use super::super::super::now_ms;
use super::super::super::response::{
    emit_usage, populate_tokens, proxy_json_response, route_error_origin,
};
use super::super::account::{ImageAttempt, image_capability_unavailable};
use super::super::{
    PreparedImageRequest,
    native::{ImageEnvelope, normalize_response},
};
use super::ImageAttemptStep;
use crate::GatewayRuntime;
use crate::UsageEvent;
use crate::error_codes;
use crate::runtime::{CandidateLease, ExecutorRoute};
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_collected_image(
    runtime: &GatewayRuntime,
    lease: &CandidateLease,
    route: &ExecutorRoute,
    prepared: &PreparedImageRequest,
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
        if (retryable_route_failure(route, status, failure.category, false) || capability_failure)
            && !route_forbids_fallback(route, status, failure.category)
        {
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
            populate_tokens(&mut event, &bytes);
            if let Ok(envelope) = serde_json::from_slice::<ImageEnvelope<'_>>(&bytes) {
                envelope.observe_usage(&mut event);
            }
            apply_failure_state(&mut event, failure_state);
            emit_usage(runtime, event);
            return ImageAttemptStep::Retry(failure);
        }
        let mut event = observed.event(false, status, Some(failure.category.to_string()));
        populate_tokens(&mut event, &bytes);
        if let Ok(envelope) = serde_json::from_slice::<ImageEnvelope<'_>>(&bytes) {
            envelope.observe_usage(&mut event);
        }
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

    let mut event = observed.event(false, status, None);
    let error = match serde_json::from_slice::<ImageEnvelope<'_>>(&bytes) {
        Ok(envelope) => {
            envelope.observe_usage(&mut event);
            if let Some(image_usage) = event.image_usage.as_mut() {
                image_usage.generated_images = envelope.output_count() as u64;
            }
            if envelope.error.is_some() {
                event.upstream_error =
                    Some(crate::usage::UpstreamErrorDetails::from_response_body(
                        Some(status.as_u16()),
                        &bytes,
                    ));
                Some(error_codes::STREAM_ERROR)
            } else if (envelope.output_count() as u64)
                < prepared
                    .fields
                    .get("n")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(1)
            {
                Some(error_codes::IMAGE_OUTPUT_MISSING)
            } else {
                None
            }
        }
        Err(_) => Some(error_codes::STREAM_INVALID),
    };
    if let Some(category) = error {
        // An accepted but missing/invalid result is never safe to replay.
        lease.settle_rotation_unknown(now_ms());
        event.http_status = 502;
        event.error_category = Some(category.to_owned());
        if let Some(routing) = event.routing.as_mut() {
            routing.attempt_outcome = Some(crate::AttemptOutcome::ExecutionUncertain);
        }
        emit_usage(runtime, event);
        return ImageAttemptStep::Respond(api_error(
            StatusCode::BAD_GATEWAY,
            "upstream did not return a completed image",
            category,
        ));
    }
    event.success = true;
    event.latency_ms = observed.started.elapsed().as_millis() as u64;
    finish_image_success(runtime, lease, event);
    ImageAttemptStep::Respond(proxy_json_response(
        status,
        &response_headers,
        Body::from(normalize_response(bytes, &prepared.response_format)),
    ))
}

fn finish_image_success(runtime: &GatewayRuntime, lease: &CandidateLease, mut event: UsageEvent) {
    let recovered = lease.settle_rotation_success_with_latency(now_ms(), event.ttft_ms);
    event.consecutive_failures = recovered.then_some(0);
    emit_usage(runtime, event);
}
