use super::super::{
    runtime_now_ms, AuthorizationIdentityPolicy, AuthorizationIncarnation, AuthorizedRequestError,
    AuthorizedResponse, CodexTurnStateScope, ExecutorPrepareError, GatewayRuntime,
    PreparedAuthorization,
};
use super::prepare::{agent_credential_fingerprint, inspect_agent_identity_unauthorized};
use super::AuthorizationDispatch;
use crate::accounts::TokenDispatchRevisionGuard;
use crate::providers::chatgpt::AgentIdentityCredential;
use reqwest::StatusCode;
use std::sync::atomic::Ordering;
use std::sync::RwLockReadGuard;

pub(in crate::runtime) struct PreparedAuthorizationDispatchGuard<'a> {
    _token: Option<TokenDispatchRevisionGuard<'a>>,
    _agent: Option<RwLockReadGuard<'a, Option<AgentIdentityCredential>>>,
    _plan: Option<RwLockReadGuard<'a, Option<String>>>,
}

impl GatewayRuntime {
    pub(crate) async fn send_authorized_request(
        &self,
        candidate_id: &str,
        request: reqwest::RequestBuilder,
        dispatch: AuthorizationDispatch<'_>,
    ) -> std::result::Result<AuthorizedResponse, AuthorizedRequestError> {
        self.send_authorized_request_with_binding(candidate_id, request, dispatch, None, None)
            .await
    }

    pub(crate) async fn send_authorized_request_for_route(
        &self,
        route: &crate::runtime::ExecutorRoute,
        request: reqwest::RequestBuilder,
        dispatch: AuthorizationDispatch<'_>,
    ) -> std::result::Result<AuthorizedResponse, AuthorizedRequestError> {
        self.send_authorized_request_with_binding(
            &route.candidate_id,
            request,
            dispatch,
            route.account_id.is_none().then(|| route.binding_key()),
            None,
        )
        .await
    }

    /// Continue a completed read-only catalog lookup on its exact OAuth
    /// incarnation. No fallback, owner switch, or authorization replay occurs.
    pub(crate) async fn send_catalog_continuation(
        &self,
        route: &crate::runtime::ExecutorRoute,
        request: reqwest::RequestBuilder,
        dispatch: AuthorizationDispatch<'_>,
        expected: &AuthorizationIncarnation,
    ) -> std::result::Result<AuthorizedResponse, AuthorizedRequestError> {
        if !self.is_basis_points_account(&route.candidate_id)
            && !self.is_siwc_account(&route.candidate_id)
        {
            return Err(AuthorizedRequestError::NotReplayable);
        }
        self.send_authorized_request_with_binding(
            &route.candidate_id,
            request,
            dispatch,
            None,
            Some(expected),
        )
        .await
    }

    async fn send_authorized_request_with_binding(
        &self,
        candidate_id: &str,
        request: reqwest::RequestBuilder,
        dispatch: AuthorizationDispatch<'_>,
        binding: Option<crate::SourceProtocolBindingKey>,
        expected: Option<&AuthorizationIncarnation>,
    ) -> std::result::Result<AuthorizedResponse, AuthorizedRequestError> {
        let budget = dispatch.budget;
        let first_request = request
            .try_clone()
            .ok_or(AuthorizedRequestError::NotReplayable)?;
        let prepared = if let Some(binding) = binding {
            self.prepare_source_authorization(candidate_id, binding)
                .map_err(AuthorizedRequestError::Prepare)?
        } else if budget.is_some() && self.is_basis_points_account(candidate_id) {
            self.prepare_basis_points_authorization(candidate_id)
                .await?
        } else {
            self.prepare_authorization(candidate_id, runtime_now_ms())
                .await
                .map_err(AuthorizedRequestError::Prepare)?
        };
        if expected.is_some_and(|expected| *expected != prepared.incarnation()) {
            return Err(AuthorizedRequestError::NotReplayable);
        }
        let upstream_response = self
            .send_prepared_authorization(candidate_id, first_request, &prepared, dispatch)
            .await?;
        if upstream_response.status() == StatusCode::UNAUTHORIZED {
            if let Some(task_id) = prepared.agent_task_id.as_deref() {
                let (unauthorized_response, invalid_task) =
                    inspect_agent_identity_unauthorized(upstream_response).await?;
                if !invalid_task {
                    return Ok(self.accept_authorized_response(
                        candidate_id,
                        unauthorized_response,
                        &prepared,
                    ));
                }
                // Preserve the real 401 when there is no room for a second
                // generation. A hidden auth retry must never bypass the
                // outer request's dispatch budget.
                if budget.is_some_and(|budget| !budget.can_dispatch()) {
                    return Ok(AuthorizedResponse {
                        response: unauthorized_response,
                        account_token_generation: prepared.token_generation,
                        authorization_incarnation: prepared.incarnation(),
                    });
                }
                self.record_authorization_replay(
                    dispatch,
                    prepared.token_generation,
                    &unauthorized_response,
                );
                drop(unauthorized_response);
                if let Some(budget) = budget {
                    budget.observe_rejection();
                }
                let refreshed = self
                    .refresh_agent_identity_task_after_unauthorized(
                        candidate_id,
                        task_id,
                        runtime_now_ms(),
                    )
                    .await
                    .map_err(AuthorizedRequestError::Prepare)?;
                let refreshed_response = self
                    .send_prepared_authorization(candidate_id, request, &refreshed, dispatch)
                    .await?;
                return Ok(self.accept_authorized_response(
                    candidate_id,
                    refreshed_response,
                    &refreshed,
                ));
            }
        }
        if upstream_response.status() != StatusCode::UNAUTHORIZED
            || prepared.token_generation.is_none()
        {
            return Ok(self.accept_authorized_response(candidate_id, upstream_response, &prepared));
        }

        if self.is_siwc_account(candidate_id) {
            // Direct-plan 401 can mean missing permission: do not replay or change billing.
            return Ok(self.accept_authorized_response(candidate_id, upstream_response, &prepared));
        }
        if self.is_basis_points_account(candidate_id) {
            let account =
                self.chatgpt_accounts
                    .get(candidate_id)
                    .ok_or(AuthorizedRequestError::Prepare(
                        ExecutorPrepareError::Authentication,
                    ))?;
            let revision =
                prepared
                    .token_revision
                    .as_ref()
                    .ok_or(AuthorizedRequestError::Prepare(
                        ExecutorPrepareError::InvalidCredential,
                    ))?;
            account
                .token_authority
                .invalidate_access_revision_and_persist(
                    candidate_id,
                    revision,
                    runtime_now_ms(),
                    account.persistence_adapter.as_ref(),
                )
                .await
                .map_err(|_| AuthorizedRequestError::Prepare(ExecutorPrepareError::Persistence))?;
            // Keep the Excel refresh token and the original refusal. The next
            // request prepares a fresh bearer; this response is never replayed.
            return Ok(self.accept_authorized_response(candidate_id, upstream_response, &prepared));
        }

        if budget.is_some_and(|budget| !budget.can_dispatch()) {
            return Ok(AuthorizedResponse {
                response: upstream_response,
                account_token_generation: prepared.token_generation,
                authorization_incarnation: prepared.incarnation(),
            });
        }
        self.record_authorization_replay(dispatch, prepared.token_generation, &upstream_response);
        drop(upstream_response);
        if let Some(budget) = budget {
            budget.observe_rejection();
        }
        let fence = self.fence_execution(candidate_id);
        let refreshed = self
            .refresh_authorization_after_unauthorized(candidate_id, &prepared, runtime_now_ms())
            .await
            .map_err(AuthorizedRequestError::Prepare)?;
        // The owner retains its lease for this proven 401 repair. Once the
        // token authority has produced the replacement, release the Auth
        // admission fence before that same lease passes final dispatch.
        drop(fence);
        let refreshed_response = self
            .send_prepared_authorization(candidate_id, request, &refreshed, dispatch)
            .await?;
        Ok(self.accept_authorized_response(candidate_id, refreshed_response, &refreshed))
    }

    fn accept_authorized_response(
        &self,
        candidate_id: &str,
        response: reqwest::Response,
        prepared: &PreparedAuthorization,
    ) -> AuthorizedResponse {
        self.observe_codex_quota_headers(
            candidate_id,
            response.status(),
            response.headers(),
            runtime_now_ms(),
        );
        AuthorizedResponse {
            response,
            account_token_generation: prepared.token_generation,
            authorization_incarnation: prepared.incarnation(),
        }
    }

    fn record_authorization_replay(
        &self,
        dispatch: AuthorizationDispatch<'_>,
        generation: Option<u64>,
        response: &reqwest::Response,
    ) {
        let Some((template, started)) = dispatch.usage_event else {
            return;
        };
        if template
            .account_id
            .as_deref()
            .is_some_and(|id| !self.account_candidate_is_active(id))
        {
            return;
        }
        let mut event = template.clone();
        event.latency_ms = started.elapsed().as_millis() as u64;
        event.attempt = dispatch
            .budget
            .map_or(event.attempt, |budget| u16::from(budget.dispatches()));
        event.account_token_generation = generation;
        event.error_category = Some(crate::error_codes::UPSTREAM_UNAUTHORIZED.to_string());
        if let Some(routing) = event.routing.as_mut() {
            routing.attempt_outcome = Some(crate::AttemptOutcome::RetryAuthorization);
            if let Some(transport) = routing.transport.as_mut() {
                transport.request_payload = response
                    .extensions()
                    .get::<crate::usage::RequestPayloadDiagnostics>()
                    .cloned();
            }
        }
        // Audit only: the authorization owner is already repairing this 401.
        // Do not submit a second health/auth vote or reset cache-context state.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.usage)(event)));
    }

    pub(crate) fn routing_cookies(
        &self,
        candidate_id: &str,
        prepared: &PreparedAuthorization,
    ) -> Option<std::sync::Arc<super::super::routing_cookies::RoutingCookieJar>> {
        self.chatgpt_accounts.get(candidate_id).map(|account| {
            account
                .routing_cookies
                .for_credential(prepared.turn_state_credential())
        })
    }

    pub(crate) fn guard_turn_state(
        &self,
        headers: &mut reqwest::header::HeaderMap,
        scope: Option<&CodexTurnStateScope<'_>>,
        prepared: &PreparedAuthorization,
    ) {
        if prepared.identity.is_none() {
            return;
        }
        let now_ms = runtime_now_ms();
        let authorization = prepared.incarnation();
        super::super::session_state::request::guard_headers(headers, &|state| {
            scope.is_some_and(|scope| {
                self.codex_turn_state_matches(
                    scope,
                    state,
                    prepared.turn_state_credential(),
                    &authorization,
                    now_ms,
                )
            })
        });
    }

    pub(crate) fn guard_codex_payload(
        &self,
        payload: &mut serde_json::Value,
        scope: Option<&CodexTurnStateScope<'_>>,
        prepared: &PreparedAuthorization,
    ) -> bool {
        if prepared.identity.is_none() {
            return false;
        }
        let now_ms = runtime_now_ms();
        let authorization = prepared.incarnation();
        super::super::session_state::request::guard_payload(payload, &|state| {
            scope.is_some_and(|scope| {
                self.codex_turn_state_matches(
                    scope,
                    state,
                    prepared.turn_state_credential(),
                    &authorization,
                    now_ms,
                )
            })
        })
    }

    pub(crate) fn observe_turn_state(
        &self,
        headers: &reqwest::header::HeaderMap,
        scope: Option<&CodexTurnStateScope<'_>>,
        prepared: &PreparedAuthorization,
    ) {
        if let (Some(scope), Some(turn_state_header)) = (scope, headers.get("x-codex-turn-state")) {
            self.note_codex_turn_state(
                scope,
                turn_state_header.as_bytes(),
                prepared.turn_state_credential(),
                &prepared.incarnation(),
                runtime_now_ms(),
            );
        }
    }

    async fn send_prepared_authorization(
        &self,
        candidate_id: &str,
        request: reqwest::RequestBuilder,
        prepared: &PreparedAuthorization,
        dispatch: AuthorizationDispatch<'_>,
    ) -> std::result::Result<reqwest::Response, AuthorizedRequestError> {
        let AuthorizationDispatch {
            client_version,
            identity_policy,
            turn_scope: scope,
            budget,
            lease,
            usage_event: _,
        } = dispatch;
        let (client, mut authorized_request) =
            apply_prepared_authorization(request, prepared, client_version, identity_policy)?;
        if self.is_siwc_account(candidate_id) {
            crate::providers::chatgpt::sanitize_siwc_headers(authorized_request.headers_mut());
        }
        if budget.is_some() {
            self.verify_basis_points_request(candidate_id, prepared, &authorized_request)
                .await?;
        }
        if identity_policy == AuthorizationIdentityPolicy::RelayCodex && prepared.identity.is_some()
        {
            self.guard_turn_state(authorized_request.headers_mut(), scope, prepared);
            let json_body = authorized_request
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
            if let Some(body) = authorized_request.body().filter(|_| json_body) {
                let bytes = body
                    .as_bytes()
                    .ok_or(AuthorizedRequestError::NotReplayable)?;
                let mut payload: serde_json::Value = serde_json::from_slice(bytes)
                    .map_err(|_| AuthorizedRequestError::NotReplayable)?;
                if self.guard_codex_payload(&mut payload, scope, prepared) {
                    let scoped_body = serde_json::to_vec(&payload)
                        .map_err(|_| AuthorizedRequestError::NotReplayable)?;
                    *authorized_request.body_mut() = Some(scoped_body.into());
                    authorized_request
                        .headers_mut()
                        .remove(reqwest::header::CONTENT_LENGTH);
                }
            }
        }
        let native_codex_account = self.chatgpt_accounts.contains_key(candidate_id)
            && !self.is_siwc_account(candidate_id)
            && !self.is_basis_points_account(candidate_id)
            && identity_policy == AuthorizationIdentityPolicy::RelayCodex
            && prepared.identity.is_some();
        let diagnostics_mode = dispatch
            .usage_event
            .and_then(|(event, _)| event.routing.as_ref())
            .and_then(|routing| routing.transport.as_ref())
            .map(|transport| transport.mode);
        let (mut authorized_request, payload_diagnostics) =
            super::payload::encode(authorized_request, native_codex_account, diagnostics_mode)
                .await;
        let url = authorized_request.url().clone();
        let cookies = if self.is_siwc_account(candidate_id) {
            None
        } else {
            self.routing_cookies(candidate_id, prepared)
        };
        if let Some(cookies) = &cookies {
            cookies.apply(&url, authorized_request.headers_mut());
        }
        let stale_authorization =
            || AuthorizedRequestError::Prepare(ExecutorPrepareError::Transient);
        let headers_deadline = budget
            .filter(|_| !self.is_basis_points_account(candidate_id))
            .and_then(|budget| {
                crate::transport::http_headers_timeout()
                    .map(|timeout| budget.http_headers_deadline(timeout))
            });
        if headers_deadline.is_some_and(|deadline| deadline <= tokio::time::Instant::now()) {
            return Err(AuthorizedRequestError::HeadersTimeout);
        }
        if let Some(budget) = budget {
            if let Some(lease) = lease {
                lease
                    .begin_rotation_http_dispatch_for(prepared, self)
                    .map_err(|error| {
                        if error == crate::scheduler::rotation::DispatchStartError::BudgetExhausted
                        {
                            AuthorizedRequestError::DispatchBudgetExhausted
                        } else {
                            AuthorizedRequestError::Prepare(ExecutorPrepareError::Transient)
                        }
                    })?;
            } else {
                budget.with_budget(|request_budget| {
                    let _guard = prepared
                        .dispatch_guard(self, candidate_id)
                        .ok_or_else(stale_authorization)?;
                    if !request_budget.can_start_wire() || !request_budget.can_dispatch() {
                        return Err(AuthorizedRequestError::DispatchBudgetExhausted);
                    }
                    request_budget
                        .start_dispatch()
                        .expect("dispatch capacity was checked under the budget lock");
                    request_budget
                        .start_wire_attempt()
                        .expect("wire capacity was checked under the budget lock");
                    Ok(())
                })?;
            }
        } else {
            let _guard = prepared
                .dispatch_guard(self, candidate_id)
                .ok_or_else(stale_authorization)?;
        }
        // This boundary deliberately excludes authorization and body encoding.
        // reqwest does not expose upload completion: do not label this upload time.
        let send_started = payload_diagnostics
            .as_ref()
            .map(|_| std::time::Instant::now());
        let mut upstream_response = if let Some(deadline) = headers_deadline {
            crate::transport::execute_with_headers_deadline(&client, authorized_request, deadline)
                .await?
        } else {
            match self
                .is_basis_points_account(candidate_id)
                .then(crate::transport::basis_points_progress_timeout)
                .flatten()
            {
                Some(timeout) => tokio::time::timeout(timeout, client.execute(authorized_request))
                    .await
                    .map_err(|_| AuthorizedRequestError::ProgressTimeout)?,
                None => client.execute(authorized_request).await,
            }
            .map_err(AuthorizedRequestError::Transport)?
        };
        let send_to_headers_ms = send_started.map(|start| start.elapsed().as_millis() as u64);
        if let Some(cookies) = cookies {
            cookies.observe(&url, upstream_response.headers());
        }
        if let Some(mut payload_diagnostics) = payload_diagnostics {
            payload_diagnostics.http_send_to_headers_ms = send_to_headers_ms;
            upstream_response
                .extensions_mut()
                .insert(payload_diagnostics);
        }
        if upstream_response.status().is_success() {
            self.observe_turn_state(upstream_response.headers(), scope, prepared);
            self.observe_codex_models_etag(
                candidate_id,
                &url,
                upstream_response.headers(),
                prepared,
            );
        }
        Ok(upstream_response)
    }
}

mod prepared;
use prepared::apply_prepared_authorization;
