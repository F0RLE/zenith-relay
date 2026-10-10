use super::super::{
    runtime_now_ms, AuthorizationIncarnation, AuthorizedRequestError, AuthorizedResponse,
    CodexTurnStateScope, ExecutorPrepareError, GatewayRuntime, PreparedAuthorization,
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
}

impl GatewayRuntime {
    pub(crate) async fn send_authorized_request(
        &self,
        candidate_id: &str,
        request: reqwest::RequestBuilder,
        dispatch: AuthorizationDispatch<'_>,
    ) -> std::result::Result<AuthorizedResponse, AuthorizedRequestError> {
        let budget = dispatch.budget;
        let first_request = request
            .try_clone()
            .ok_or(AuthorizedRequestError::NotReplayable)?;
        let prepared = if budget.is_some() && self.is_basis_points_account(candidate_id) {
            self.prepare_basis_points_authorization(candidate_id)
                .await?
        } else {
            self.prepare_authorization(candidate_id, runtime_now_ms())
                .await
                .map_err(AuthorizedRequestError::Prepare)?
        };
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
                        prepared.token_generation,
                    ));
                }
                // Preserve the real 401 when there is no room for a second
                // generation. A hidden auth retry must never bypass the
                // outer request's dispatch budget.
                if budget.is_some_and(|budget| !budget.can_dispatch()) {
                    return Ok(AuthorizedResponse {
                        response: unauthorized_response,
                        account_token_generation: prepared.token_generation,
                    });
                }
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
                    refreshed.token_generation,
                ));
            }
        }
        if upstream_response.status() != StatusCode::UNAUTHORIZED
            || prepared.token_generation.is_none()
        {
            return Ok(self.accept_authorized_response(
                candidate_id,
                upstream_response,
                prepared.token_generation,
            ));
        }

        if budget.is_some_and(|budget| !budget.can_dispatch()) {
            return Ok(AuthorizedResponse {
                response: upstream_response,
                account_token_generation: prepared.token_generation,
            });
        }
        drop(upstream_response);
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
        let refreshed_response = self
            .send_prepared_authorization(candidate_id, request, &refreshed, dispatch)
            .await?;
        Ok(self.accept_authorized_response(
            candidate_id,
            refreshed_response,
            refreshed.token_generation,
        ))
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
        if let Some(turn_state_header) = headers.get("x-codex-turn-state") {
            if !scope.is_some_and(|scope| {
                self.codex_turn_state_matches(
                    scope,
                    turn_state_header.as_bytes(),
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
        if let (Some(scope), Some(turn_state_header)) = (scope, headers.get("x-codex-turn-state")) {
            self.note_codex_turn_state(
                scope,
                turn_state_header.as_bytes(),
                prepared.turn_state_credential(),
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
        } = dispatch;
        let (client, mut authorized_request) =
            apply_prepared_authorization(request, prepared, client_version, identity_policy)?;
        if budget.is_some() {
            self.verify_basis_points_request(candidate_id, prepared, &authorized_request)
                .await?;
        }
        self.guard_turn_state(authorized_request.headers_mut(), scope, prepared);
        let url = authorized_request.url().clone();
        let cookies = self.routing_cookies(candidate_id, prepared);
        if let Some(cookies) = &cookies {
            cookies.apply(&url, authorized_request.headers_mut());
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
        let upstream_response = match self
            .is_basis_points_account(candidate_id)
            .then(crate::transport::basis_points_progress_timeout)
            .flatten()
        {
            Some(timeout) => tokio::time::timeout(timeout, client.execute(authorized_request))
                .await
                .map_err(|_| AuthorizedRequestError::ProgressTimeout)?,
            None => client.execute(authorized_request).await,
        }
        .map_err(AuthorizedRequestError::Transport)?;
        if let Some(cookies) = cookies {
            cookies.observe(&url, upstream_response.headers());
        }
        if upstream_response.status().is_success() {
            self.observe_turn_state(upstream_response.headers(), scope, prepared);
        }
        Ok(upstream_response)
    }
}

mod prepared;
use prepared::apply_prepared_authorization;
