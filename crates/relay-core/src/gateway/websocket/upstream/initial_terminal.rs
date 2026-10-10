use super::super::*;
use super::messages::first_message_terminal;
use super::telemetry::{
    record_connect_affinity_miss, record_connect_failure_with_hint, record_connect_rejection,
};
use super::{ConnectProgress, ConnectScope};

pub(super) enum TerminalAction {
    Continue,
    Break,
    Proceed,
}

pub(super) fn handle_initial_terminal(
    initial_messages: &[UpstreamMessage],
    scope: &mut ConnectScope<'_>,
    progress: &mut ConnectProgress<'_>,
) -> Result<TerminalAction, GatewayFailure> {
    let Some(terminal) = initial_messages.last().and_then(first_message_terminal) else {
        return Ok(TerminalAction::Proceed);
    };
    if terminal.outcome != Some(EventTerminalOutcome::Failure) {
        return Ok(TerminalAction::Proceed);
    }
    if initial_messages
        .iter()
        .any(|message| super::messages::initial_message_state(message).0)
    {
        // A failed response can still contain generated output. Let the bridge
        // deliver and settle it; body repair must not execute that turn again.
        return Ok(TerminalAction::Proceed);
    }
    let terminal_body = initial_messages.last().and_then(|message| match message {
        UpstreamMessage::Text(text) => Some(text.as_bytes()),
        UpstreamMessage::Binary(bytes) => Some(bytes.as_ref()),
        _ => None,
    });
    if continue_after_item_prefix_repair(terminal_body, scope, progress) {
        return Ok(TerminalAction::Continue);
    }
    let (status, category) = resolved_terminal_failure(&terminal);
    if continue_after_legacy_call_links(terminal_body, scope, progress) {
        return Ok(TerminalAction::Continue);
    }
    let affinity_miss = super::super::super::errors::recoverable_response_affinity_miss(
        status,
        scope.request.has_previous_response_id(),
        scope.response_affinity_hit,
        terminal.previous_response_not_found,
    );
    if continue_after_missing_response_replay(
        status,
        category,
        affinity_miss,
        &terminal,
        scope,
        progress,
    )? {
        return Ok(TerminalAction::Continue);
    }
    if continue_after_stale_tool_history(
        status,
        category,
        terminal_body,
        &terminal,
        scope,
        progress,
    ) {
        return Ok(TerminalAction::Continue);
    }
    if continue_after_model_switch(status, category, terminal_body, &terminal, scope, progress) {
        return Ok(TerminalAction::Continue);
    }
    settle_initial_terminal_failure(&terminal, scope);
    if continue_after_cache_write_rejection(
        status,
        category,
        terminal_body,
        &terminal,
        scope,
        progress,
    ) {
        return Ok(TerminalAction::Continue);
    }
    if let Some(action) =
        retry_initial_terminal(status, category, affinity_miss, &terminal, scope, progress)
    {
        return Ok(action);
    }
    Ok(TerminalAction::Proceed)
}

fn continue_after_item_prefix_repair(
    terminal_body: Option<&[u8]>,
    scope: &mut ConnectScope<'_>,
    progress: &mut ConnectProgress<'_>,
) -> bool {
    let route = scope.route;
    let lease = scope.lease;
    let tried = &mut *progress.tried;
    let repairs = &mut *progress.repairs;
    let repaired = terminal_body.is_some_and(|terminal_body_bytes| {
        repair_responses_item_prefixes(
            scope.request.request_body_mut(),
            terminal_body_bytes,
            true,
            &mut ResponsesItemPrefixRepairs {
                function_ids: &mut repairs.function_item_id,
                custom_tool_ids: &mut repairs.custom_tool_item_id,
                message_ids: &mut repairs.message_item_id,
            },
            tried,
            &route.candidate_id,
            lease,
        )
    });
    if !repaired {
        return false;
    }
    // A missing item prefix is a compatible body repair, not an upstream
    // failure. Settling the terminal first would mark the outcome unknown
    // and stop this retry.
    lease.settle_rotation_repair(now_ms());
    *progress.last_failure = None;
    true
}

fn continue_after_legacy_call_links(
    terminal_body: Option<&[u8]>,
    scope: &mut ConnectScope<'_>,
    progress: &mut ConnectProgress<'_>,
) -> bool {
    if progress.repairs.legacy_call_id
        || !terminal_body
            .is_some_and(super::super::super::errors::responses_tool_call_links_rejected)
        || !scope.request.repair_legacy_call_ids()
    {
        return false;
    }
    progress.repairs.legacy_call_id = true;
    progress.tried.remove(&scope.route.candidate_id);
    scope.lease.settle_rotation_repair(now_ms());
    true
}

fn continue_after_missing_response_replay(
    status: StatusCode,
    category: &'static str,
    affinity_miss: bool,
    terminal: &EventTerminal,
    scope: &mut ConnectScope<'_>,
    progress: &mut ConnectProgress<'_>,
) -> Result<bool, GatewayFailure> {
    let runtime = scope.runtime;
    let key = scope.key;
    let route = scope.route;
    if !(affinity_miss
        && scope.response_affinity_hit
        && scope.request.replay_missing_response(
            runtime,
            &key.id,
            route,
            &mut progress.repairs.native_replay,
        )?)
    {
        return Ok(false);
    }
    record_connect_affinity_miss(&scope.trace(), status);
    progress.tried.remove(&route.candidate_id);
    scope.lease.settle_rotation_repair(now_ms());
    *progress.last_failure = Some(classified_terminal_failure(
        status,
        category,
        scope.source_error_origin,
        terminal,
    ));
    Ok(true)
}

fn continue_after_stale_tool_history(
    status: StatusCode,
    category: &'static str,
    terminal_body: Option<&[u8]>,
    terminal: &EventTerminal,
    scope: &mut ConnectScope<'_>,
    progress: &mut ConnectProgress<'_>,
) -> bool {
    let runtime = scope.runtime;
    let key = scope.key;
    let recovered = !progress.repairs.stale_tool_history
        && scope.request.has_previous_response_id()
        && terminal_body.is_some_and(|terminal_body_bytes| {
            super::super::super::errors::responses_tool_call_is_missing_output(terminal_body_bytes)
                && scope
                    .request
                    .recover_stale_tool_history(runtime, &key.id, terminal_body_bytes)
        });
    if !recovered {
        return false;
    }
    progress.repairs.stale_tool_history = true;
    remember_terminal_rejection(status, category, terminal, scope, progress);
    true
}

fn continue_after_model_switch(
    status: StatusCode,
    category: &'static str,
    terminal_body: Option<&[u8]>,
    terminal: &EventTerminal,
    scope: &mut ConnectScope<'_>,
    progress: &mut ConnectProgress<'_>,
) -> bool {
    let runtime = scope.runtime;
    let key = scope.key;
    let switched = !progress.repairs.model_switch_reset
        && super::super::super::errors::recoverable_response_model_switch(
            status,
            category,
            scope.request.has_previous_response_id(),
            scope.request.has_unpaired_tool_output(),
            terminal_body.unwrap_or_default(),
        )
        && scope.request.drop_previous_response_id(runtime, &key.id);
    if !switched {
        return false;
    }
    progress.repairs.model_switch_reset = true;
    remember_terminal_rejection(status, category, terminal, scope, progress);
    true
}

fn settle_initial_terminal_failure(terminal: &EventTerminal, scope: &ConnectScope<'_>) {
    let failure = super::super::super::errors::AttemptFailure::classified_with_hint(
        terminal_failure_status(terminal.status),
        terminal
            .error_category
            .unwrap_or(error_codes::UPSTREAM_TERMINAL),
        terminal.body_hint,
    );
    super::super::super::errors::settle_attempt_failure(
        scope.runtime,
        scope.lease,
        &scope.route.source_model,
        &failure,
        &terminal.headers,
    );
}

fn continue_after_cache_write_rejection(
    status: StatusCode,
    category: &'static str,
    terminal_body: Option<&[u8]>,
    terminal: &EventTerminal,
    scope: &mut ConnectScope<'_>,
    progress: &mut ConnectProgress<'_>,
) -> bool {
    if !terminal_body.is_some_and(super::super::super::errors::prompt_cache_write_rejected) {
        return false;
    }
    scope
        .runtime
        .invalidate_prompt_affinity(scope.request.prompt_affinity_key.as_deref());
    let failure =
        classified_terminal_failure(status, category, scope.source_error_origin, terminal);
    record_connect_failure_with_hint(
        &scope.trace(),
        &failure,
        Some(&terminal.headers),
        terminal.body_hint,
    );
    *progress.last_failure = Some(failure);
    true
}

fn retry_initial_terminal(
    status: StatusCode,
    category: &'static str,
    affinity_miss: bool,
    terminal: &EventTerminal,
    scope: &mut ConnectScope<'_>,
    progress: &mut ConnectProgress<'_>,
) -> Option<TerminalAction> {
    if !(affinity_miss
        || super::super::super::errors::retryable_failure(
            status,
            category,
            scope.request.has_previous_response_id(),
        ))
    {
        return None;
    }
    let failure =
        classified_terminal_failure(status, category, scope.source_error_origin, terminal);
    if affinity_miss {
        *progress.confirmed_response_missing |= terminal.previous_response_not_found;
        scope
            .runtime
            .invalidate_response_affinity(scope.request.response_affinity_key.as_deref());
        record_connect_affinity_miss(&scope.trace(), status);
    } else {
        record_connect_failure_with_hint(
            &scope.trace(),
            &failure,
            Some(&terminal.headers),
            terminal.body_hint,
        );
        if scope.response_affinity_hit && !scope.request.requires_affinity_owner {
            scope.request.response_affinity_key = None;
        }
    }
    *progress.last_failure = Some(failure);
    Some(
        if affinity_miss && terminal.previous_response_not_found && scope.response_affinity_hit {
            TerminalAction::Break
        } else {
            TerminalAction::Continue
        },
    )
}

fn classified_terminal_failure(
    status: StatusCode,
    category: &'static str,
    origin: ErrorOrigin,
    terminal: &EventTerminal,
) -> GatewayFailure {
    GatewayFailure::classified(status, category, origin)
        .with_upstream_error(terminal.upstream_error.clone())
}

fn remember_terminal_rejection(
    status: StatusCode,
    category: &'static str,
    terminal: &EventTerminal,
    scope: &mut ConnectScope<'_>,
    progress: &mut ConnectProgress<'_>,
) {
    let failure =
        classified_terminal_failure(status, category, scope.source_error_origin, terminal);
    record_connect_rejection(&scope.trace(), &failure);
    scope.lease.settle_rotation_repair(now_ms());
    *progress.last_failure = Some(failure);
}
