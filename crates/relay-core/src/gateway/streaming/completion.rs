use std::sync::{Arc, Mutex};

use super::upstream_usage::UpstreamUsage;
use super::*;
use crate::runtime::CandidateLease;
use crate::MessagesBridgeResponse;
use axum::http::HeaderMap;
use serde_json::Value;

pub(super) struct StreamCompletionSettlement {
    pub(super) upstream_usage: Option<Arc<Mutex<UpstreamUsage>>>,
    pub(super) lease: CandidateLease,
    pub(super) runtime: std::sync::Arc<crate::GatewayRuntime>,
    pub(super) source: String,
    pub(super) model: String,
    pub(super) headers: HeaderMap,
    pub(super) prompt_affinity: Option<String>,
    pub(super) uses_response_affinity: bool,
    pub(super) bridge_state: Option<Arc<Mutex<Option<MessagesBridgeResponse>>>>,
    pub(super) native_response: Option<Arc<Mutex<Option<Value>>>>,
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
        if event.success {
            self.lease.settle_rotation_success(now_ms());
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
                    candidate_id: &self.source,
                    model: &self.model,
                    status: StatusCode::from_u16(event.http_status)
                        .unwrap_or(StatusCode::BAD_GATEWAY),
                    category,
                    headers: &self.headers,
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
            let recovered = self.runtime.record_success_with_metrics(
                &self.source,
                &self.model,
                now_ms(),
                event.output_tokens,
                event.generation_ms.unwrap_or(event.latency_ms),
            );
            event.consecutive_failures = recovered.then_some(0);
            self.runtime.bind_prompt_affinity(
                self.prompt_affinity.as_deref(),
                &self.source,
                now_ms(),
            );
            if self.uses_response_affinity {
                self.runtime
                    .bind_response_affinity(response_id, &self.source, now_ms());
            }
            if let Some(shared) = self.bridge_state.as_ref() {
                if let Some(response) = crate::poison::mutex(shared).take() {
                    self.runtime.save_messages_bridge_response(
                        &self.local_key,
                        &self.source,
                        &response,
                        now_ms(),
                    );
                }
            }
            if let Some(shared) = self.native_response.as_ref() {
                if let Some(response) = crate::poison::mutex(shared).take() {
                    for call_id in response_tool_call_ids(&response) {
                        self.runtime.bind_tool_call_affinity(
                            &self.local_key,
                            &call_id,
                            &self.source,
                            now_ms(),
                        );
                    }
                    self.runtime.capture_native_responses_replay(
                        &self.local_key,
                        &self.source,
                        &self.native_template,
                        &self.model,
                        &response,
                        now_ms(),
                    );
                }
            }
        } else if event
            .error_category
            .as_deref()
            .is_some_and(failure_category_requires_cooldown)
        {
            let state = current_failure_state(&self.runtime, &self.source, &self.model);
            apply_failure_state(event, state);
        }
    }
}
