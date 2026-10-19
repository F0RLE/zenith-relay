use std::sync::{Arc, Mutex};

use super::upstream_usage::UpstreamUsage;
use super::*;
use crate::runtime::CandidateLease;
use crate::TranslatedResponse;
use axum::http::HeaderMap;
use serde_json::Value;

pub(super) struct StreamCompletionSettlement {
    pub(super) discovery_progress:
        Option<Arc<Mutex<super::basis_points::discovery::DiscoveryProgress>>>,
    // Keep the request-local comparison alive until terminal usage is emitted.
    pub(super) _cache_context_observation:
        Option<crate::runtime::cache_context::CacheContextObservation>,
    pub(super) upstream_usage: Option<Arc<Mutex<UpstreamUsage>>>,
    pub(super) lease: CandidateLease,
    pub(super) runtime: std::sync::Arc<crate::GatewayRuntime>,
    pub(super) candidate_id: String,
    pub(super) route: crate::runtime::ExecutorRoute,
    pub(super) model: String,
    pub(super) headers: HeaderMap,
    pub(super) prompt_affinity: Option<String>,
    pub(super) uses_response_affinity: bool,
    pub(super) bridge_state: Option<Arc<Mutex<Option<TranslatedResponse>>>>,
    pub(super) native_response: Option<Arc<Mutex<Option<Value>>>>,
    pub(super) tool_call_capture: Arc<Mutex<crate::gateway::request::ToolCallCapture>>,
    pub(super) native_template: Value,
    pub(super) local_key: String,
}

impl StreamCompletionSettlement {
    pub(super) fn settle(
        &self,
        event: &mut crate::UsageEvent,
        response_id: Option<&str>,
        hint: crate::gateway::errors::RateLimitBodyHint,
    ) {
        if let Some(capture) = &self.upstream_usage {
            crate::poison::mutex(capture).apply_to(event);
        }
        let mut discovery_headers = None;
        if let Some(progress) = &self.discovery_progress {
            let progress = crate::poison::mutex(progress);
            event.attempt = u16::from(progress.budget.dispatches());
            event
                .tool_use
                .basis_points_catalog
                .clone_from(&progress.tool_use.basis_points_catalog);
            event
                .tool_use
                .upstream_context
                .clone_from(&progress.tool_use.upstream_context);
            discovery_headers.clone_from(&progress.headers);
        }
        if self.route.client_wire_api == crate::WireApi::Gemini
            && self.route.adapter.is_passthrough()
            && event.error_category.as_deref() == Some(error_codes::RESPONSE_INCOMPLETE)
        {
            if let Some(shared) = &self.native_response {
                if let Some(response) = crate::poison::mutex(shared).take() {
                    crate::gateway::continuation::gemini::bind_response(
                        &self.runtime,
                        &self.local_key,
                        &self.route,
                        &self.native_template,
                        &response,
                        now_ms(),
                    );
                }
            }
        }
        if event.success {
            let recovered = self
                .lease
                .settle_rotation_success_with_latency(now_ms(), event.ttft_ms);
            event.consecutive_failures = recovered.then_some(0);
        } else {
            // This callback belongs to an already returned response body.
            // A terminal failure may affect health, but can never replay it.
            let health = if event.error_category.as_deref().is_some_and(|category| {
                matches!(
                    category,
                    error_codes::UPSTREAM_SERVER_ERROR
                        | error_codes::UPSTREAM_OVERLOADED
                        | error_codes::UPSTREAM_UNAVAILABLE
                )
            }) {
                crate::scheduler::rotation::HealthObservation::CountableTransient {
                    provider_not_before_ms: None,
                }
            } else {
                crate::scheduler::rotation::HealthObservation::Unknown
            };
            let now = std::time::SystemTime::now();
            let cooldown = event.error_category.as_deref().and_then(|category| {
                failure_cooldown(CooldownInput {
                    runtime: &self.runtime,
                    candidate_id: &self.candidate_id,
                    model: &self.model,
                    status: StatusCode::from_u16(event.http_status)
                        .unwrap_or(StatusCode::BAD_GATEWAY),
                    category,
                    headers: discovery_headers.as_ref().unwrap_or(&self.headers),
                    hint,
                    now,
                })
            });
            self.runtime.settle_rotation_failure(
                &self.lease,
                crate::scheduler::rotation::AttemptObservation {
                    execution: crate::scheduler::rotation::ExecutionObservation::committed(),
                    health,
                },
                cooldown,
                crate::unix_time_ms_at(now),
            );
        }
        // A response is healthy only after the upstream has emitted its
        // successful terminal event. An incomplete response may have
        // delivered bytes to the client, but it must not warm affinity or
        // reset the selected slot's failure state.
        if event.success {
            crate::poison::mutex(&self.tool_call_capture).bind(
                &self.runtime,
                &self.local_key,
                &self.route,
                now_ms(),
            );
            self.runtime.bind_prompt_affinity(
                self.prompt_affinity.as_deref(),
                &self.candidate_id,
                now_ms(),
            );
            if self.uses_response_affinity {
                self.runtime
                    .bind_response_route_affinity(response_id, &self.route, now_ms());
            }
            if let Some(shared) = self.bridge_state.as_ref() {
                if let Some(messages_bridge_response) = crate::poison::mutex(shared).take() {
                    self.runtime.save_adapter_response_for_route(
                        &self.local_key,
                        &self.route,
                        &messages_bridge_response,
                        now_ms(),
                    );
                }
            }
            if let Some(shared) = self.native_response.as_ref() {
                if let Some(native_response_payload) = crate::poison::mutex(shared).take() {
                    for call_id in response_tool_call_ids(&native_response_payload) {
                        self.runtime.bind_tool_call_route_affinity(
                            &self.local_key,
                            &call_id,
                            &self.route,
                            now_ms(),
                        );
                    }
                    self.runtime.capture_native_responses_replay(
                        &self.local_key,
                        &self.candidate_id,
                        &self.native_template,
                        &self.model,
                        &native_response_payload,
                        now_ms(),
                    );
                }
            }
        } else if event
            .error_category
            .as_deref()
            .is_some_and(failure_category_requires_cooldown)
        {
            let failure_state =
                current_failure_state(&self.runtime, &self.candidate_id, &self.model);
            apply_failure_state(event, failure_state);
        }
    }
}
