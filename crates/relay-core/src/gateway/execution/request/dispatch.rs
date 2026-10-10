use super::prelude::*;
use crate::runtime::ExecutorRoute;
use crate::usage::ToolUseDiagnostics;

pub(super) enum RequestDispatch {
    Continue,
    Respond(Response<Body>),
    Ready(Box<DispatchedRequestAttempt>),
}

pub(super) struct DispatchedRequestAttempt {
    pub(super) route: ExecutorRoute,
    pub(super) upstream: reqwest::Response,
    pub(super) status: StatusCode,
    pub(super) response_headers: HeaderMap,
    pub(super) started: Instant,
}

pub(super) struct RequestDispatchInput<'a> {
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) lease: &'a CandidateLease,
    pub(super) budget: &'a SharedRequestBudget,
    pub(super) route: ExecutorRoute,
    pub(super) client_wire_api: WireApi,
    pub(super) stream: bool,
    pub(super) account_route: bool,
    pub(super) basis_points_route: bool,
    pub(super) route_responses_lite: Option<HeaderValue>,
    pub(super) adapter_request: &'a PreparedAdapterRequest,
    pub(super) request_body: Vec<u8>,
    pub(super) reasoning_effort: &'a ReasoningEffortDiagnostics,
    pub(super) tool_use: &'a ToolUseDiagnostics,
    pub(super) request_id: &'a str,
    pub(super) requested_model: &'a str,
    pub(super) forwarded_headers: &'a HeaderMap,
    pub(super) selected_error_origin: ErrorOrigin,
    pub(super) attempt: &'a mut u16,
    pub(super) last_failure: &'a mut Option<AttemptFailure>,
    pub(super) last_failure_origin: &'a mut ErrorOrigin,
}

/// Send one prepared attempt. A transport failure stays retryable until the
/// provider may already have accepted the request.
pub(super) async fn dispatch_request_attempt(
    request_dispatch_input: RequestDispatchInput<'_>,
) -> RequestDispatch {
    let RequestDispatchInput {
        runtime,
        key,
        lease,
        budget,
        mut route,
        client_wire_api,
        stream,
        account_route,
        basis_points_route,
        route_responses_lite,
        adapter_request,
        request_body,
        reasoning_effort,
        tool_use,
        request_id,
        requested_model,
        forwarded_headers,
        selected_error_origin,
        attempt,
        last_failure,
        last_failure_origin,
    } = request_dispatch_input;
    let request_body = if basis_points_route {
        match super::super::basis_points::attach_input_images(
            runtime,
            &route.candidate_id,
            &route.upstream_url,
            &route.upstream_headers,
            request_body,
        )
        .await
        {
            Ok(upstream_response_body) => upstream_response_body,
            Err(super::super::basis_points::AttachmentFailure::Reject(failure)) => {
                return RequestDispatch::Respond(attempt_error_response(
                    failure,
                    None,
                    selected_error_origin,
                    request_id,
                ));
            }
            Err(super::super::basis_points::AttachmentFailure::Retry(failure)) => {
                *last_failure = Some(failure);
                *last_failure_origin = selected_error_origin;
                return RequestDispatch::Continue;
            }
        }
    } else {
        request_body
    };
    let upstream_stream = stream || (account_route && !basis_points_route);
    let started = Instant::now();
    let request_client = runtime.request_client(&route.candidate_id);
    let mut upstream_headers = upstream_headers_for_route(
        account_route,
        basis_points_route,
        adapter_request.requires_bridge_headers(),
        route.adapter.upstream_protocol(client_wire_api),
        forwarded_headers,
    );
    for (name, header_value) in &route.upstream_headers {
        upstream_headers.insert(name.clone(), header_value.clone());
    }
    let turn_scope = (account_route
        && !basis_points_route
        && client_wire_api == WireApi::Responses
        && route.adapter.is_passthrough())
    .then(|| {
        request_scope(
            &key.id,
            forwarded_headers,
            route.account_id.as_deref(),
            &route.source_model,
        )
    })
    .flatten();
    if turn_scope.is_none() {
        upstream_headers.remove(CODEX_TURN_STATE_HEADER);
    }
    if account_route && !basis_points_route {
        apply_codex_routing_hint(
            &mut upstream_headers,
            &route.source_model,
            route.service_tier,
        );
    }
    let mut upstream_request = request_client
        .post(route.upstream_url.clone())
        .header(CONTENT_TYPE, "application/json")
        .headers(upstream_headers);
    if upstream_stream {
        upstream_request = upstream_request.header(ACCEPT, "text/event-stream");
    } else if basis_points_route {
        // Basis Points mirrors the native client contract and expects an
        // explicit JSON response preference for buffered requests.
        upstream_request = upstream_request.header(ACCEPT, "application/json");
    }
    if account_route && !basis_points_route {
        if let Some(responses_lite_header) = route_responses_lite.as_ref() {
            upstream_request =
                upstream_request.header(CODEX_RESPONSES_LITE_HEADER, responses_lite_header);
        }
    }
    let upstream = runtime
        .send_authorized_request(
            &route.candidate_id,
            upstream_request.body(request_body),
            crate::runtime::AuthorizationDispatch {
                client_version: (!basis_points_route)
                    .then(|| codex_client_version(forwarded_headers))
                    .flatten(),
                identity_policy: if basis_points_route {
                    AuthorizationIdentityPolicy::PreserveUpstream
                } else {
                    AuthorizationIdentityPolicy::RelayCodex
                },
                turn_scope: turn_scope.as_ref(),
                budget: Some(budget),
                lease: Some(lease),
            },
        )
        .await;
    // Includes internal auth replay; repair and recovery cannot refund a
    // real upstream dispatch just by changing their visible attempt count.
    *attempt = u16::from(budget.dispatches());
    let upstream = match upstream {
        Ok(upstream) => {
            route.account_token_generation = upstream.account_token_generation;
            upstream.response
        }
        Err(error) => {
            let uncertain = error.execution_certainty() == ExecutionCertainty::Unknown;
            let exhausted = matches!(error, AuthorizedRequestError::DispatchBudgetExhausted);
            let failure = AttemptFailure::authorized_request(error);
            let mut event = usage_event(
                UsageAttempt {
                    request_id,
                    attempt: *attempt,
                    local_key_id: &key.id,
                    route: &route,
                    reasoning_effort: Some(reasoning_effort),
                    requested_model,
                    tool_use: tool_use.clone(),
                },
                false,
                failure.status.as_u16(),
                Some(failure.category.to_string()),
                started.elapsed().as_millis() as u64,
            );
            // Unknown remote acceptance is neither a health vote nor a
            // replayable error. A local exhausted budget is not a source
            // failure either.
            if uncertain || exhausted {
                if uncertain {
                    lease.settle_rotation_unknown(now_ms());
                }
                emit_usage(runtime, event);
                return RequestDispatch::Respond(attempt_error_response(
                    failure,
                    None,
                    selected_error_origin,
                    request_id,
                ));
            }
            let failure_state =
                settle_route_failure(runtime, lease, &route, &failure, &HeaderMap::new());
            apply_failure_state(&mut event, failure_state);
            emit_usage(runtime, event);
            *last_failure = Some(failure);
            *last_failure_origin = selected_error_origin;
            return RequestDispatch::Continue;
        }
    };
    let upstream_status = upstream.status();
    let response_headers = upstream.headers().clone();
    RequestDispatch::Ready(Box::new(DispatchedRequestAttempt {
        route,
        upstream,
        status: upstream_status,
        response_headers,
        started,
    }))
}

/// Forward client metadata only within the selected protocol. Source
/// credentials are added by the selected route; Codex/OpenAI headers must not
/// be copied to an unrelated Messages or Gemini provider.
/// Account routes retain their existing forwarded-header behavior.
fn upstream_headers_for_route(
    is_account_route: bool,
    is_basis_points_route: bool,
    needs_bridge_headers: bool,
    provider_protocol: crate::UpstreamProtocol,
    client_headers: &HeaderMap,
) -> HeaderMap {
    if is_basis_points_route {
        return HeaderMap::new();
    }

    if needs_bridge_headers {
        // A translated request is already a complete upstream contract. Keep
        // only metadata that belongs to that contract; never copy incoming
        // credentials or OpenAI/Codex-only headers to another provider.
        return match provider_protocol {
            crate::UpstreamProtocol::Messages => forwarded_bridge_messages_headers(client_headers),
            crate::UpstreamProtocol::GeminiGenerateContent => {
                forwarded_bridge_gemini_headers(client_headers)
            }
            crate::UpstreamProtocol::Responses | crate::UpstreamProtocol::ChatCompletions => {
                HeaderMap::new()
            }
        };
    }

    if is_account_route {
        client_headers.clone()
    } else {
        // Filter again at the source boundary. Native Responses and Messages
        // keep their own client metadata without forwarding credentials or
        // synthesizing a session for an API source.
        match provider_protocol {
            crate::UpstreamProtocol::Messages => forwarded_messages_headers(client_headers),
            crate::UpstreamProtocol::Responses => forwarded_responses_headers(client_headers),
            crate::UpstreamProtocol::ChatCompletions
            | crate::UpstreamProtocol::GeminiGenerateContent => HeaderMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::upstream_headers_for_route;
    use crate::UpstreamProtocol;
    use axum::http::{HeaderMap, HeaderValue};

    const CLAUDE_CODE_SESSION_HEADER: &str = "x-claude-code-session-id";

    fn codex_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("Codex Desktop/1.0"));
        headers.insert(
            "x-codex-session-id",
            HeaderValue::from_static("codex-session"),
        );
        headers.insert(
            CLAUDE_CODE_SESSION_HEADER,
            HeaderValue::from_static("codex-session"),
        );
        headers.insert("openai-beta", HeaderValue::from_static("responses=v1"));
        headers
    }

    #[test]
    fn source_bridge_keeps_only_upstream_metadata() {
        let messages = upstream_headers_for_route(
            false,
            false,
            true,
            UpstreamProtocol::Messages,
            &codex_headers(),
        );
        assert_eq!(
            messages.get(CLAUDE_CODE_SESSION_HEADER),
            Some(&HeaderValue::from_static("codex-session"))
        );
        assert_eq!(
            messages.get("user-agent"),
            Some(&HeaderValue::from_static("Codex Desktop/1.0"))
        );
        assert!(!messages.contains_key("openai-beta"));
        assert!(!messages.contains_key("x-codex-session-id"));

        let gemini = upstream_headers_for_route(
            false,
            false,
            true,
            UpstreamProtocol::GeminiGenerateContent,
            &codex_headers(),
        );
        assert_eq!(
            gemini.get("user-agent"),
            Some(&HeaderValue::from_static("Codex Desktop/1.0"))
        );
        assert!(!gemini.contains_key(CLAUDE_CODE_SESSION_HEADER));
        assert!(!gemini.contains_key("openai-beta"));
    }

    #[test]
    fn account_headers_keep_existing_bridge_policy() {
        let forwarded = upstream_headers_for_route(
            true,
            false,
            true,
            UpstreamProtocol::Messages,
            &codex_headers(),
        );
        assert_eq!(
            forwarded.get(CLAUDE_CODE_SESSION_HEADER),
            Some(&HeaderValue::from_static("codex-session"))
        );
        assert!(!forwarded.contains_key("openai-beta"));
        assert!(!forwarded.contains_key("x-codex-session-id"));
    }

    #[test]
    fn native_messages_source_keeps_filtered_messages_metadata() {
        let forwarded = upstream_headers_for_route(
            false,
            false,
            false,
            UpstreamProtocol::Messages,
            &codex_headers(),
        );
        assert_eq!(
            forwarded.get(CLAUDE_CODE_SESSION_HEADER),
            Some(&HeaderValue::from_static("codex-session"))
        );
        assert!(!forwarded.contains_key("openai-beta"));
        assert!(!forwarded.contains_key("x-codex-session-id"));
    }
}
