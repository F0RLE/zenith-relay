use super::super::{now_ms, ExecutorRoute, GatewayFailure};
use crate::error_codes;
use crate::gateway::continuation;
use crate::gateway::request::repair_legacy_responses_call_ids;
use crate::GatewayRuntime;
use serde_json::Value;

impl super::ClientRequest {
    pub(in crate::gateway::websocket) fn native_replay_value(&self) -> Value {
        self.value.clone()
    }

    /// Replace an owner-bound opaque continuation with materialized native
    /// history before selecting a replacement candidate.
    pub(in crate::gateway::websocket) fn replay_native_continuation(
        &mut self,
        runtime: &GatewayRuntime,
        local_key_id: &str,
        owner_candidate_id: &str,
        owner_model: &str,
    ) -> Result<bool, GatewayFailure> {
        let Some(previous_response_id) = self.previous_response_id() else {
            return Ok(false);
        };
        let Some(replay) = runtime.load_native_responses_replay(
            local_key_id,
            previous_response_id,
            owner_candidate_id,
            now_ms(),
        ) else {
            return Ok(false);
        };
        let replayed = match replay.replay_request(&self.value, owner_model, true) {
            Ok(value) => value,
            Err(error) if error.code() == error_codes::ADAPTER_CONTINUATION_MISMATCH => {
                return Ok(false)
            }
            Err(_) => {
                return Err(GatewayFailure::invalid_request(
                    "native continuation state is invalid",
                ))
            }
        };
        self.value = replayed;
        continuation::clear_materialized_continuation(
            &mut self.response_affinity_key,
            &mut self.requires_affinity_owner,
            &mut self.has_unpaired_tool_output,
        );
        Ok(true)
    }

    pub(in crate::gateway::websocket) fn replay_missing_response(
        &mut self,
        runtime: &GatewayRuntime,
        local_key_id: &str,
        route: &ExecutorRoute,
        attempted: &mut bool,
    ) -> Result<bool, GatewayFailure> {
        if *attempted || !self.requires_affinity_owner {
            return Ok(false);
        }
        if !self.replay_native_continuation(
            runtime,
            local_key_id,
            &route.candidate_id,
            &route.source_model,
        )? {
            return Ok(false);
        }
        *attempted = true;
        Ok(true)
    }

    pub(in crate::gateway::websocket) fn has_previous_response_id(&self) -> bool {
        self.previous_response_id().is_some()
    }

    pub(in crate::gateway::websocket) fn previous_response_id(&self) -> Option<&str> {
        continuation::previous_response_id(&self.value)
    }

    pub(in crate::gateway::websocket) const fn has_unpaired_tool_output(&self) -> bool {
        self.has_unpaired_tool_output
    }

    pub(in crate::gateway::websocket) fn drop_previous_response_id(
        &mut self,
        runtime: &GatewayRuntime,
        local_key_id: &str,
    ) -> bool {
        if continuation::drop_materialized_previous_response_id(
            runtime,
            local_key_id,
            &mut self.value,
            &self.resolved_model,
            now_ms(),
        ) {
            continuation::clear_materialized_continuation(
                &mut self.response_affinity_key,
                &mut self.requires_affinity_owner,
                &mut self.has_unpaired_tool_output,
            );
            true
        } else {
            false
        }
    }

    pub(in crate::gateway::websocket) fn recover_stale_tool_history(
        &mut self,
        runtime: &GatewayRuntime,
        local_key_id: &str,
        upstream_error: &[u8],
    ) -> bool {
        let mut materialized = self.value.clone();
        if !continuation::recover_stale_tool_history(
            runtime,
            local_key_id,
            &mut materialized,
            &self.resolved_model,
            now_ms(),
            true,
            upstream_error,
        ) {
            return false;
        }
        self.value = materialized;
        continuation::clear_materialized_continuation(
            &mut self.response_affinity_key,
            &mut self.requires_affinity_owner,
            &mut self.has_unpaired_tool_output,
        );
        true
    }

    pub(in crate::gateway::websocket) fn value_mut(&mut self) -> &mut Value {
        &mut self.value
    }

    #[cfg(test)]
    pub(in crate::gateway::websocket) fn repair_message_item_ids(&mut self) -> bool {
        crate::protocol::remove_item_prefixed_message_ids(&mut self.value)
    }

    pub(in crate::gateway::websocket) fn repair_legacy_call_ids(&mut self) -> bool {
        if !repair_legacy_responses_call_ids(&mut self.value) {
            return false;
        }
        self.has_unpaired_tool_output =
            !crate::gateway::request::unpaired_tool_output_ids(&self.value).is_empty();
        self.requires_affinity_owner =
            self.has_previous_response_id() || self.has_unpaired_tool_output;
        true
    }
}
