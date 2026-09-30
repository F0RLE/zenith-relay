use super::super::{
    runtime_now_ms, AuthorizationIncarnation, AuthorizedRequestError, AuthorizedResponse,
    CandidateLease, CodexTurnStateScope, ExecutorPrepareError, GatewayRuntime,
    PreparedAuthorization,
};
use super::prepare::{agent_credential_fingerprint, inspect_agent_identity_unauthorized};
use crate::accounts::TokenDispatchRevisionGuard;
use crate::providers::chatgpt::AgentIdentityCredential;
use crate::scheduler::rotation::SharedRequestBudget;
use reqwest::StatusCode;
use std::sync::atomic::Ordering;
use std::sync::RwLockReadGuard;

pub(in crate::runtime) struct PreparedAuthorizationDispatchGuard<'a> {
    _token: Option<TokenDispatchRevisionGuard<'a>>,
    _agent: Option<RwLockReadGuard<'a, Option<AgentIdentityCredential>>>,
}

impl GatewayRuntime {
    pub(crate) async fn send_authorized_request(
        &self,
        candidate_id: &str,
        request: reqwest::RequestBuilder,
        client_version: Option<&str>,
        turn_scope: Option<&CodexTurnStateScope<'_>>,
        budget: Option<&SharedRequestBudget>,
        lease: Option<&CandidateLease>,
    ) -> std::result::Result<AuthorizedResponse, AuthorizedRequestError> {
        let first_request = request
            .try_clone()
            .ok_or(AuthorizedRequestError::NotReplayable)?;
        let prepared = self
            .prepare_authorization(candidate_id, runtime_now_ms())
            .await
            .map_err(AuthorizedRequestError::Prepare)?;
        let response = self
            .send_prepared_authorization(
                candidate_id,
                first_request,
                &prepared,
                client_version,
                turn_scope,
                budget,
                lease,
            )
            .await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            if let Some(task_id) = prepared.agent_task_id.as_deref() {
                let (response, invalid_task) =
                    inspect_agent_identity_unauthorized(response).await?;
                if !invalid_task {
                    return Ok(self.accept_authorized_response(
                        candidate_id,
                        response,
                        prepared.token_generation,
                    ));
                }
                // Preserve the real 401 when there is no room for a second
                // generation. A hidden auth retry must never bypass the
                // outer request's dispatch budget.
                if budget.is_some_and(|budget| !budget.can_dispatch()) {
                    return Ok(AuthorizedResponse {
                        response,
                        account_token_generation: prepared.token_generation,
                    });
                }
                drop(response);
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
                let response = self
                    .send_prepared_authorization(
                        candidate_id,
                        request,
                        &refreshed,
                        client_version,
                        turn_scope,
                        budget,
                        lease,
                    )
                    .await?;
                return Ok(self.accept_authorized_response(
                    candidate_id,
                    response,
                    refreshed.token_generation,
                ));
            }
        }
        if response.status() != StatusCode::UNAUTHORIZED || prepared.token_generation.is_none() {
            return Ok(self.accept_authorized_response(
                candidate_id,
                response,
                prepared.token_generation,
            ));
        }

        if budget.is_some_and(|budget| !budget.can_dispatch()) {
            return Ok(AuthorizedResponse {
                response,
                account_token_generation: prepared.token_generation,
            });
        }
        drop(response);
        if let Some(budget) = budget {
            budget.observe_rejection();
        }
        let fence = self.fence_execution(candidate_id);
        let refreshed = self
            .refresh_authorization_after_unauthorized(
                candidate_id,
                prepared.token_generation,
                runtime_now_ms(),
            )
            .await
            .map_err(AuthorizedRequestError::Prepare)?;
        // The owner retains its lease for this proven 401 repair. Once the
        // token authority has produced the replacement, release the Auth
        // admission fence before that same lease passes final dispatch.
        drop(fence);
        let response = self
            .send_prepared_authorization(
                candidate_id,
                request,
                &refreshed,
                client_version,
                turn_scope,
                budget,
                lease,
            )
            .await?;
        Ok(self.accept_authorized_response(candidate_id, response, refreshed.token_generation))
    }

    fn accept_authorized_response(
        &self,
        candidate_id: &str,
        response: reqwest::Response,
        account_token_generation: Option<u64>,
    ) -> AuthorizedResponse {
        self.observe_codex_quota_headers(
            candidate_id,
            response.status(),
            response.headers(),
            runtime_now_ms(),
        );
        AuthorizedResponse {
            response,
            account_token_generation,
        }
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
        if let Some(state) = headers.get("x-codex-turn-state") {
            if !scope.is_some_and(|scope| {
                self.codex_turn_state_matches(
                    scope,
                    state.as_bytes(),
                    prepared.turn_state_credential(),
                    runtime_now_ms(),
                )
            }) {
                headers.remove("x-codex-turn-state");
            }
        }
    }

    pub(crate) fn observe_turn_state(
        &self,
        headers: &reqwest::header::HeaderMap,
        scope: Option<&CodexTurnStateScope<'_>>,
        prepared: &PreparedAuthorization,
    ) {
        if let (Some(scope), Some(state)) = (scope, headers.get("x-codex-turn-state")) {
            self.note_codex_turn_state(
                scope,
                state.as_bytes(),
                prepared.turn_state_credential(),
                runtime_now_ms(),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_prepared_authorization(
        &self,
        candidate_id: &str,
        request: reqwest::RequestBuilder,
        prepared: &PreparedAuthorization,
        client_version: Option<&str>,
        scope: Option<&CodexTurnStateScope<'_>>,
        budget: Option<&SharedRequestBudget>,
        lease: Option<&CandidateLease>,
    ) -> std::result::Result<reqwest::Response, AuthorizedRequestError> {
        let (client, mut request) =
            apply_prepared_authorization(request, prepared, client_version)?;
        self.guard_turn_state(request.headers_mut(), scope, prepared);
        let url = request.url().clone();
        let cookies = self.routing_cookies(candidate_id, prepared);
        if let Some(cookies) = &cookies {
            cookies.apply(&url, request.headers_mut());
        }
        let stale_authorization =
            || AuthorizedRequestError::Prepare(ExecutorPrepareError::Transient);
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
        let response = client
            .execute(request)
            .await
            .map_err(AuthorizedRequestError::Transport)?;
        if let Some(cookies) = cookies {
            cookies.observe(&url, response.headers());
        }
        if response.status().is_success() {
            self.observe_turn_state(response.headers(), scope, prepared);
        }
        Ok(response)
    }
}

mod prepared;
use prepared::apply_prepared_authorization;
