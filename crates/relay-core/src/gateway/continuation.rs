use crate::error_codes;
mod compacted;
mod replay;

pub(super) use replay::{drop_materialized_previous_response_id, recover_stale_tool_history};

use super::request::{tool_call_output_ids, unpaired_tool_output_ids};
use crate::GatewayRuntime;
use serde_json::Value;

pub(super) const RESPONSE_CONTINUATION_UNAVAILABLE_CODE: &str =
    error_codes::RESPONSE_CONTINUATION_UNAVAILABLE;
pub(super) const RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE: &str =
    "response continuation is unavailable; resend complete history without previous_response_id or start a new conversation";

/// Trimmed Responses `previous_response_id`, or `None` when the field is absent or blank.
///
/// Callers that decide whether a request is a continuation use this form.
/// Recovery still reads the raw field so an untrimmed stored identifier is unchanged.
pub(super) fn previous_response_id(request: &Value) -> Option<&str> {
    request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|response_id| !response_id.is_empty())
}

/// Drops the opaque continuation binding after its history has been materialized.
///
/// Retry bookkeeping stays with the caller. Account-only execution does not
/// settle a rotation repair on the same paths as an ordinary request.
pub(super) fn clear_materialized_continuation(
    response_affinity_key: &mut Option<String>,
    requires_affinity_owner: &mut bool,
    has_unpaired_tool_output: &mut bool,
) {
    *response_affinity_key = None;
    *requires_affinity_owner = false;
    *has_unpaired_tool_output = false;
}

/// Drops the opaque continuation requirement after replay while retaining the
/// saved owner binding for one same-provider repair attempt.
pub(super) fn retain_materialized_continuation_owner(
    requires_affinity_owner: &mut bool,
    has_unpaired_tool_output: &mut bool,
) {
    *requires_affinity_owner = false;
    *has_unpaired_tool_output = false;
}

/// Shared ownership facts for HTTP, WebSocket, and account-only execution.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct ContinuationState {
    pub(super) response_affinity_key: Option<String>,
    pub(super) requires_affinity_owner: bool,
    pub(super) has_unpaired_tool_output: bool,
}

/// Classifies an incoming Responses continuation before candidate selection.
///
/// An opaque ID created outside this Relay instance has no known owner. It
/// must never be forwarded to an arbitrary candidate. The shape of incoming
/// messages cannot establish that the client included all earlier context.
pub(super) fn prepare_response_continuation(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    request: &mut Value,
    now_ms: u64,
    connection_affinity_key: Option<&str>,
) -> Result<ContinuationState, ()> {
    if compacted::reset_compacted_history(request) {
        return Ok(ContinuationState {
            response_affinity_key: None,
            requires_affinity_owner: false,
            has_unpaired_tool_output: false,
        });
    }
    let response_affinity_key = connection_affinity_key.map(str::to_string).or_else(|| {
        runtime.response_affinity_key(request.get("previous_response_id").and_then(Value::as_str))
    });
    let response_binding_known = response_affinity_key
        .as_deref()
        .is_some_and(|key| runtime.has_response_affinity_binding(key, now_ms));
    let tool_output_ids = tool_call_output_ids(request);
    let unpaired_output_ids = unpaired_tool_output_ids(request);
    let has_unpaired_tool_output = !unpaired_output_ids.is_empty();
    let tool_affinity_key = if !response_binding_known && has_unpaired_tool_output {
        let mut owner = None;
        let mut owner_key = None;
        // Completed historic calls do not establish ownership of new results.
        // Every unpaired output must resolve to the same known candidate.
        for call_id in unpaired_output_ids {
            let affinity_key = runtime
                .tool_call_affinity_key(local_key_id, &call_id)
                .ok_or(())?;
            let candidate = runtime
                .response_affinity_candidate(&affinity_key, now_ms)
                .ok_or(())?;
            if owner.as_ref().is_some_and(|owner| owner != &candidate) {
                return Err(());
            }
            owner = Some(candidate);
            owner_key = Some(affinity_key);
        }
        owner_key
    } else {
        tool_output_ids.iter().find_map(|call_id| {
            let affinity_key = runtime.tool_call_affinity_key(local_key_id, call_id)?;
            runtime
                .has_response_affinity_binding(&affinity_key, now_ms)
                .then_some(affinity_key)
        })
    };
    let has_previous_response_id = response_affinity_key.is_some();
    if has_previous_response_id && !response_binding_known {
        return Err(());
    }
    if !response_binding_known && has_unpaired_tool_output && tool_affinity_key.is_none() {
        return Err(());
    }

    let response_affinity_key = if response_binding_known {
        response_affinity_key
    } else {
        tool_affinity_key.or(response_affinity_key)
    };
    Ok(ContinuationState {
        // A complete tool call plus its matching output is self-contained:
        // it can be sent to another compatible candidate without an opaque
        // response reference. Only an actual previous response or an
        // unpaired tool output still needs its creating owner.
        requires_affinity_owner: has_previous_response_id || has_unpaired_tool_output,
        response_affinity_key,
        has_unpaired_tool_output,
    })
}

/// Materializes the saved predecessor before releasing its owner. Plaintext
/// replay can change models; tool and encrypted state use the stricter native
/// replay path. Missing, evicted or differently scoped state stays owner-bound.
#[cfg(test)]
mod tests;
