//! Route ownership shares the existing bounded affinity and persistence
//! infrastructure. Only fingerprints are durable; conversation state is not.
use crate::runtime::{ExecutorRoute, GatewayRuntime, ResponseAffinityBinding};
use crate::{AdapterError, AdapterResult, WireApi, RESPONSE_AFFINITY_TTL_MS};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

impl GatewayRuntime {
    /// Remember only hashed call ownership, even when full history cannot be
    /// materialized or retained. This is volatile and shares its bounded
    /// TTL store with route ownership; it never authorizes a different route.
    pub(super) fn bind_response_tool_ownership(
        &self,
        local_key_id: &str,
        candidate_id: &str,
        model: &str,
        upstream: &Value,
        now_ms: u64,
    ) {
        let response = upstream
            .pointer("/response/response")
            .or_else(|| upstream.get("response"))
            .unwrap_or(upstream);
        if crate::protocol::completion::ResponseCompletion::from_body(response)
            != crate::protocol::completion::ResponseCompletion::Completed
        {
            return;
        }
        let Some(response_id) = response.get("id").and_then(Value::as_str) else {
            return;
        };
        let Some(scope) =
            response_tool_ownership_scope(local_key_id, response_id, candidate_id, model)
        else {
            return;
        };
        let calls: Vec<_> = response
            .get("output")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| {
                matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("function_call" | "custom_tool_call")
                )
            })
            .filter_map(|item| item.get("call_id").and_then(Value::as_str))
            .filter(|id| !id.trim().is_empty() && id.len() <= 256)
            .take(1_024)
            .collect();
        let mut ownership = crate::poison::mutex(&self.response_route_affinity);
        if calls.is_empty() && ownership.get(&scope, now_ms).is_none() {
            return;
        }
        // A reused response ID replaces its call set instead of accumulating
        // stale permissions from an older response with the same identifier.
        let mut digest = Sha256::new();
        for call_id in &calls {
            digest.update((call_id.len() as u64).to_be_bytes());
            digest.update(call_id.as_bytes());
        }
        let generation = hex::encode(digest.finalize());
        ownership.bind(scope.clone(), generation.clone(), now_ms);
        for call_id in calls {
            let key = response_tool_ownership_key(&scope, &generation, call_id);
            ownership.bind(key, "observed", now_ms);
        }
    }

    fn continuation_route_fingerprint(&self, route: &ExecutorRoute) -> Option<String> {
        let mut digest = Sha256::new();
        let url = if route.account_id.is_none() {
            self.sources.get(&route.source_id)?.endpoint(
                route.binding_key(),
                &route.source_model,
                false,
            )?
        } else {
            route.upstream_url.clone()
        };
        for part in [
            route.candidate_id.as_bytes(),
            route.source_model.as_bytes(),
            url.as_str().as_bytes(),
            route.adapter.route_suffix(route.client_wire_api).as_bytes(),
        ] {
            digest.update((part.len() as u64).to_be_bytes());
            digest.update(part);
        }
        if route.account_id.is_none() {
            let authorization = self
                .prepare_source_authorization(&route.candidate_id, route.binding_key())
                .ok()?;
            digest.update(authorization.header_name.as_str().as_bytes());
            digest.update(authorization.authorization.as_bytes());
        }
        Some(hex::encode(digest.finalize()))
    }

    fn bind_route_affinity(&self, key: &str, route: &ExecutorRoute, now_ms: u64) {
        let Some(fingerprint) = self.continuation_route_fingerprint(route) else {
            return;
        };
        let route_key = format!("route:{key}");
        crate::poison::mutex(&self.response_route_affinity).bind(
            route_key.clone(),
            fingerprint.clone(),
            now_ms,
        );
        if let Some(store) = &self.response_affinity_store {
            let _ = store.upsert(&ResponseAffinityBinding {
                key: route_key,
                candidate_id: fingerprint,
                expires_at_ms: now_ms.saturating_add(RESPONSE_AFFINITY_TTL_MS),
            });
        }
    }

    fn route_affinity(&self, key: &str, now_ms: u64) -> Option<String> {
        let key = format!("route:{key}");
        let cached = crate::poison::mutex(&self.response_route_affinity)
            .get(&key, now_ms)
            .map(str::to_owned);
        if cached.is_some() {
            return cached;
        }
        let binding = self
            .response_affinity_store
            .as_ref()?
            .find(&key, now_ms)
            .ok()??;
        let fingerprint = binding.candidate_id.clone();
        crate::poison::mutex(&self.response_route_affinity).restore(
            key,
            binding.candidate_id,
            binding.expires_at_ms,
            now_ms,
        );
        Some(fingerprint)
    }

    pub(crate) fn bind_response_route_affinity(
        &self,
        response_id: Option<&str>,
        route: &ExecutorRoute,
        now_ms: u64,
    ) {
        self.bind_response_affinity(response_id, &route.candidate_id, now_ms);
        if let Some(key) = self.response_affinity_key(response_id) {
            self.bind_route_affinity(&key, route, now_ms);
        }
    }

    pub(crate) fn bind_tool_call_route_affinity(
        &self,
        local_key_id: &str,
        call_id: &str,
        route: &ExecutorRoute,
        now_ms: u64,
    ) {
        self.bind_tool_call_affinity(local_key_id, call_id, &route.candidate_id, now_ms);
        if let Some(key) = self.tool_call_affinity_key(local_key_id, call_id) {
            self.bind_route_affinity(&key, route, now_ms);
        }
    }

    pub(crate) fn save_adapter_response_for_route(
        &self,
        local_key_id: &str,
        route: &ExecutorRoute,
        response: &crate::TranslatedResponse,
        now_ms: u64,
    ) {
        self.save_adapter_response(local_key_id, &route.candidate_id, response, now_ms);
        if !response.continuation.is_reusable()
            || crate::protocol::completion::ResponseCompletion::from_body(&response.response_body)
                != crate::protocol::completion::ResponseCompletion::Completed
        {
            return;
        }
        self.bind_response_tool_ownership(
            local_key_id,
            &route.candidate_id,
            &route.source_model,
            &response.response_body,
            now_ms,
        );
        self.bind_response_route_affinity(Some(&response.response_id), route, now_ms);
        if let Some(key) = response.continuation.history_key() {
            self.bind_response_route_affinity(Some(key), route, now_ms);
        }
        for call_id in response
            .response_body
            .get("output")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| {
                matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("function_call" | "custom_tool_call")
                )
            })
            .filter_map(|item| item.get("call_id").and_then(Value::as_str))
        {
            self.bind_tool_call_route_affinity(local_key_id, call_id, route, now_ms);
        }
    }

    pub(crate) fn validate_continuation_route(
        &self,
        route: &ExecutorRoute,
        local_key_id: &str,
        request: &Value,
        now_ms: u64,
    ) -> AdapterResult<()> {
        let fingerprint = self
            .continuation_route_fingerprint(route)
            .ok_or_else(AdapterError::continuation_mismatch)?;
        if route.client_wire_api == WireApi::GeminiInteractions {
            if let Some(previous) = crate::protocol::interactions::previous_id(request)
                .map_err(|_| AdapterError::continuation_mismatch())?
            {
                let reference = crate::protocol::interactions::reference(local_key_id, previous)
                    .map_err(|_| AdapterError::continuation_mismatch())?;
                let key = self
                    .response_affinity_key(Some(&reference))
                    .ok_or_else(AdapterError::continuation_missing)?;
                if self.route_affinity(&key, now_ms).as_deref() != Some(fingerprint.as_str()) {
                    return Err(AdapterError::continuation_mismatch());
                }
            }
        }
        let previous = request
            .get("previous_response_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty());
        let mut predecessor_route_validated = false;
        if route.client_wire_api == WireApi::Responses {
            if let Some(previous) = previous {
                let key = self
                    .response_affinity_key(Some(previous))
                    .expect("nonempty id");
                if let Some(owner) = self.route_affinity(&key, now_ms) {
                    if owner != fingerprint {
                        return Err(AdapterError::continuation_mismatch());
                    }
                    predecessor_route_validated = true;
                } else if previous.starts_with("resp_bridge_")
                    || previous.starts_with("gemini_bridge_")
                    || previous.starts_with("chat_bridge_")
                {
                    let state = self.load_adapter_continuation_state(
                        local_key_id,
                        previous,
                        &route.candidate_id,
                        now_ms,
                    )?;
                    if state.upstream_wire_api()
                        != route
                            .adapter
                            .upstream_protocol(route.client_wire_api)
                            .wire_api()
                    {
                        return Err(AdapterError::continuation_mismatch());
                    }
                } else if self
                    .source_candidate_bindings
                    .get(&route.candidate_id)
                    .is_some_and(|binding| {
                        binding.routes.iter().any(|other| {
                            other.adapter.upstream_protocol(other.wire_api)
                                != route.adapter.upstream_protocol(route.client_wire_api)
                        })
                    })
                {
                    // Legacy opaque IDs on multi-endpoint sources have no
                    // exact owner metadata. They cannot follow a preference.
                    return Err(AdapterError::continuation_missing());
                }
            }
        }
        let paired_tool_results = paired_tool_result_ids(request, route.client_wire_api);
        let mut restored_pairs: Option<HashSet<String>> = None;
        for call_id in tool_result_ids(request, route.client_wire_api) {
            if let Some(key) = self.tool_call_affinity_key(local_key_id, call_id) {
                if self
                    .route_affinity(&key, now_ms)
                    .is_some_and(|owner| owner != fingerprint)
                    && !paired_tool_results.contains(call_id)
                {
                    if predecessor_route_validated
                        && previous
                            .and_then(|previous| {
                                response_tool_ownership_scope(
                                    local_key_id,
                                    previous,
                                    &route.candidate_id,
                                    &route.source_model,
                                )
                            })
                            .is_some_and(|scope| {
                                let mut ownership =
                                    crate::poison::mutex(&self.response_route_affinity);
                                let key = ownership.get(&scope, now_ms).map(|generation| {
                                    response_tool_ownership_key(&scope, generation, call_id)
                                });
                                key.is_some_and(|key| ownership.get(&key, now_ms).is_some())
                            })
                    {
                        continue;
                    }
                    // Provider call IDs can collide across conversations. A
                    // caller-scoped predecessor with this exact route remains
                    // authoritative only if its retained history proves the
                    // call/result pair. Recover lazily on a conflicting hint;
                    // ordinary turns do not clone the saved conversation.
                    let restored_pairs = restored_pairs.get_or_insert_with(|| {
                        if !predecessor_route_validated {
                            return HashSet::new();
                        }
                        previous
                            .and_then(|id| {
                                self.load_native_responses_replay(
                                    local_key_id,
                                    id,
                                    &route.candidate_id,
                                    now_ms,
                                )
                            })
                            .and_then(|state| {
                                state
                                    .replay_request(request, &route.source_model, false)
                                    .ok()
                            })
                            .map(|replayed| {
                                paired_tool_result_ids(&replayed, WireApi::Responses)
                                    .into_iter()
                                    .map(str::to_owned)
                                    .collect()
                            })
                            .unwrap_or_default()
                    });
                    if !restored_pairs.contains(call_id) {
                        return Err(AdapterError::continuation_mismatch());
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn validate_adapter_history_route(
        &self,
        route: &ExecutorRoute,
        history_key: &str,
        now_ms: u64,
    ) -> AdapterResult<()> {
        let key = self
            .response_affinity_key(Some(history_key))
            .ok_or_else(AdapterError::continuation_mismatch)?;
        if let Some(owner) = self.route_affinity(&key, now_ms) {
            if self.continuation_route_fingerprint(route).as_deref() != Some(&owner) {
                return Err(AdapterError::continuation_mismatch());
            }
        }
        Ok(())
    }
}

fn response_tool_ownership_scope(
    local_key_id: &str,
    response_id: &str,
    candidate_id: &str,
    model: &str,
) -> Option<String> {
    if local_key_id.is_empty() || response_id.trim().is_empty() {
        return None;
    }
    let mut digest = Sha256::new();
    digest.update(b"relay-response-tools-v1");
    for part in [local_key_id, response_id, candidate_id, model] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    Some(format!("response-tools:{}", hex::encode(digest.finalize())))
}

fn response_tool_ownership_key(scope: &str, generation: &str, call_id: &str) -> String {
    let mut digest = Sha256::new();
    for part in [scope, generation, call_id] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("response-tool:{}", hex::encode(digest.finalize()))
}

fn tool_result_ids(request: &Value, client: WireApi) -> Vec<&str> {
    match client {
        WireApi::GeminiInteractions => request
            .get("input")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_result"))
            .filter_map(|item| item.get("call_id").and_then(Value::as_str))
            .collect(),
        WireApi::Responses => request
            .get("input")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| {
                matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("function_call_output" | "custom_tool_call_output")
                )
            })
            .filter_map(|item| item.get("call_id").and_then(Value::as_str))
            .collect(),
        WireApi::ChatCompletions => request
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| item.get("role").and_then(Value::as_str) == Some("tool"))
            .filter_map(|item| item.get("tool_call_id").and_then(Value::as_str))
            .collect(),
        WireApi::Messages => request
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|message| {
                message
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_result"))
            .filter_map(|item| item.get("tool_use_id").and_then(Value::as_str))
            .collect(),
        WireApi::Gemini => request
            .get("contents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|message| {
                message
                    .get("parts")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(|item| item.pointer("/functionResponse/id").and_then(Value::as_str))
            .collect(),
    }
}

/// Result IDs whose matching tool call is present in this same request.
///
/// A complete pair carries its own history. An opaque predecessor is checked
/// separately; orphan results remain bound to the route that created the call.
fn paired_tool_result_ids(request: &Value, client: WireApi) -> HashSet<&str> {
    match client {
        // Native state is never synthesized or moved from a text/tool summary.
        WireApi::GeminiInteractions => HashSet::new(),
        WireApi::Responses => {
            let calls: HashSet<&str> = request
                .get("input")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|item| {
                    matches!(
                        item.get("type").and_then(Value::as_str),
                        Some("function_call" | "custom_tool_call")
                    )
                })
                .filter_map(|item| {
                    item.get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                })
                .collect();
            tool_result_ids(request, client)
                .into_iter()
                .filter(|call_id| calls.contains(call_id))
                .collect()
        }
        WireApi::ChatCompletions => {
            let calls: HashSet<&str> = request
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
                .flat_map(|message| {
                    message
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                })
                .filter_map(|call| call.get("id").and_then(Value::as_str))
                .collect();
            tool_result_ids(request, client)
                .into_iter()
                .filter(|call_id| calls.contains(call_id))
                .collect()
        }
        WireApi::Messages => {
            let calls: HashSet<&str> = request
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(|message| {
                    message
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                })
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"))
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .collect();
            tool_result_ids(request, client)
                .into_iter()
                .filter(|call_id| calls.contains(call_id))
                .collect()
        }
        WireApi::Gemini => {
            let calls: HashSet<&str> = request
                .get("contents")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(|message| {
                    message
                        .get("parts")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                })
                .filter_map(|item| item.pointer("/functionCall/id").and_then(Value::as_str))
                .collect();
            tool_result_ids(request, client)
                .into_iter()
                .filter(|call_id| calls.contains(call_id))
                .collect()
        }
    }
}
