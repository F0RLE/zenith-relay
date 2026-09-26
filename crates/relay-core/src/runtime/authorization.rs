use super::{
    runtime_now_ms, AuthorizationIncarnation, AuthorizedRequestError, AuthorizedResponse,
    CandidateLease, ChatGptAccountExecutor, CodexTurnStateScope, ExecutorPrepareError,
    GatewayRuntime, PreparedAuthorization,
};
use crate::accounts::{TokenAuthorityError, TokenDispatchRevisionGuard};
use crate::providers::chatgpt::{
    is_agent_identity_task_invalid_response, AgentIdentityCredential, AgentIdentityError,
};
use crate::scheduler::rotation::{ExecutionCertainty, SharedRequestBudget};
use crate::CandidateHealth;
use reqwest::header::{HeaderValue, AUTHORIZATION};
use reqwest::StatusCode;
use std::sync::atomic::Ordering;
use std::sync::RwLockReadGuard;

pub(super) struct PreparedAuthorizationDispatchGuard<'a> {
    _token: Option<TokenDispatchRevisionGuard<'a>>,
    _agent: Option<RwLockReadGuard<'a, Option<AgentIdentityCredential>>>,
}

impl AuthorizedRequestError {
    /// A transport error after execute() starts does not prove that the
    /// provider did not accept the generation. Only a connection failure is
    /// known to be pre-send; never transparently replay an unknown outcome.
    pub(crate) fn execution_certainty(&self) -> ExecutionCertainty {
        match self {
            Self::Transport(error) if !error.is_connect() => ExecutionCertainty::Unknown,
            _ => ExecutionCertainty::NotSent,
        }
    }
}

impl GatewayRuntime {
    pub(crate) async fn prepare_authorization(
        &self,
        candidate_id: &str,
        now_ms: u64,
    ) -> std::result::Result<PreparedAuthorization, ExecutorPrepareError> {
        if let Some(binding) = self.source_candidate_bindings.get(candidate_id) {
            let source = self
                .sources
                .get(&binding.source_id)
                .ok_or(ExecutorPrepareError::Authentication)?;
            let source_binding = source
                .binding_for(binding.binding_key)
                .ok_or(ExecutorPrepareError::Authentication)?;
            let (header_name, authorization) = source.authorization_for_binding(source_binding);
            return Ok(PreparedAuthorization {
                header_name,
                authorization,
                identity: None,
                token_generation: None,
                token_revision: None,
                agent_task_id: None,
                agent_credential_fingerprint: None,
                agent_identity_revision: None,
            });
        }
        let account = self
            .chatgpt_accounts
            .get(candidate_id)
            .ok_or(ExecutorPrepareError::Authentication)?;
        if !account.active.load(Ordering::Acquire) {
            return Err(ExecutorPrepareError::Authentication);
        }
        if account
            .agent_identity
            .read()
            .map_err(|_| ExecutorPrepareError::Transient)?
            .is_some()
        {
            match account.ensure_agent_identity_task(None).await {
                Ok(agent) => {
                    return Ok(PreparedAuthorization {
                        header_name: AUTHORIZATION,
                        authorization: agent
                            .authorization(now_ms)
                            .map_err(|_| ExecutorPrepareError::InvalidCredential)?,
                        identity: Some(
                            account
                                .identity
                                .with_configured_client_version()
                                .map_err(|_| ExecutorPrepareError::InvalidCredential)?,
                        ),
                        token_generation: None,
                        token_revision: None,
                        agent_task_id: agent.task_id().map(str::to_string),
                        agent_credential_fingerprint: Some(agent_credential_fingerprint(&agent)),
                        agent_identity_revision: Some(
                            account.agent_identity_revision.load(Ordering::Acquire),
                        ),
                    });
                }
                Err(error) if account.token_authority.tokens(&account.id).await.is_none() => {
                    return Err(error);
                }
                Err(_) => {}
            }
        }
        self.prepare_oauth_authorization(candidate_id, account, now_ms)
            .await
    }

    async fn prepare_oauth_authorization(
        &self,
        candidate_id: &str,
        account: &ChatGptAccountExecutor,
        now_ms: u64,
    ) -> std::result::Result<PreparedAuthorization, ExecutorPrepareError> {
        let prepared = match account
            .token_authority
            .prepare_and_persist(
                &account.id,
                now_ms,
                account.refresh_skew_ms,
                account.refresh_adapter.as_ref(),
                account.persistence_adapter.as_ref(),
            )
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let health = match &error {
                    TokenAuthorityError::RequiresReauth(_) => Some(CandidateHealth::ReauthRequired),
                    TokenAuthorityError::AccessTokenExpired
                    | TokenAuthorityError::AccountNotFound
                    | TokenAuthorityError::InvalidAccountId => Some(CandidateHealth::Unhealthy),
                    _ => None,
                };
                if let Some(health) = health {
                    self.set_candidate_health(candidate_id, health);
                }
                return Err(classify_token_authority_error(error));
            }
        };
        let Ok(mut authorization) =
            HeaderValue::from_str(&format!("Bearer {}", prepared.tokens.access_token()))
        else {
            self.set_candidate_health(candidate_id, CandidateHealth::Unhealthy);
            return Err(ExecutorPrepareError::InvalidCredential);
        };
        authorization.set_sensitive(true);
        Ok(PreparedAuthorization {
            header_name: AUTHORIZATION,
            authorization,
            identity: Some(
                account
                    .identity
                    .with_configured_client_version()
                    .map_err(|_| ExecutorPrepareError::InvalidCredential)?,
            ),
            token_generation: Some(prepared.tokens.generation()),
            token_revision: Some(prepared.dispatch_revision),
            agent_task_id: None,
            agent_credential_fingerprint: None,
            agent_identity_revision: None,
        })
    }

    pub(crate) async fn refresh_authorization_after_unauthorized(
        &self,
        candidate_id: &str,
        failed_generation: Option<u64>,
        now_ms: u64,
    ) -> std::result::Result<PreparedAuthorization, ExecutorPrepareError> {
        let account = self
            .chatgpt_accounts
            .get(candidate_id)
            .ok_or(ExecutorPrepareError::Authentication)?;
        if !account.active.load(Ordering::Acquire) {
            return Err(ExecutorPrepareError::Authentication);
        }
        account
            .token_authority
            .invalidate_access_generation_and_persist(
                &account.id,
                failed_generation,
                now_ms,
                account.persistence_adapter.as_ref(),
            )
            .await
            .map_err(classify_token_authority_error)?;
        self.prepare_authorization(candidate_id, now_ms).await
    }

    pub(crate) async fn refresh_agent_identity_task_after_unauthorized(
        &self,
        candidate_id: &str,
        expected_task_id: &str,
        now_ms: u64,
    ) -> std::result::Result<PreparedAuthorization, ExecutorPrepareError> {
        let account = self
            .chatgpt_accounts
            .get(candidate_id)
            .ok_or(ExecutorPrepareError::Authentication)?;
        if !account.active.load(Ordering::Acquire) {
            return Err(ExecutorPrepareError::Authentication);
        }
        match account
            .ensure_agent_identity_task(Some(expected_task_id))
            .await
        {
            Ok(_) => self.prepare_authorization(candidate_id, now_ms).await,
            Err(error) if account.token_authority.tokens(&account.id).await.is_none() => Err(error),
            Err(_) => {
                self.prepare_oauth_authorization(candidate_id, account, now_ms)
                    .await
            }
        }
    }

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
                    self.observe_codex_quota_headers(
                        candidate_id,
                        response.status(),
                        response.headers(),
                        runtime_now_ms(),
                    );
                    return Ok(AuthorizedResponse {
                        response,
                        account_token_generation: prepared.token_generation,
                    });
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
                self.observe_codex_quota_headers(
                    candidate_id,
                    response.status(),
                    response.headers(),
                    runtime_now_ms(),
                );
                return Ok(AuthorizedResponse {
                    response,
                    account_token_generation: refreshed.token_generation,
                });
            }
        }
        if response.status() != StatusCode::UNAUTHORIZED || prepared.token_generation.is_none() {
            self.observe_codex_quota_headers(
                candidate_id,
                response.status(),
                response.headers(),
                runtime_now_ms(),
            );
            return Ok(AuthorizedResponse {
                response,
                account_token_generation: prepared.token_generation,
            });
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
        self.observe_codex_quota_headers(
            candidate_id,
            response.status(),
            response.headers(),
            runtime_now_ms(),
        );
        Ok(AuthorizedResponse {
            response,
            account_token_generation: refreshed.token_generation,
        })
    }
}

impl ChatGptAccountExecutor {
    async fn ensure_agent_identity_task(
        &self,
        expected_task_id: Option<&str>,
    ) -> std::result::Result<AgentIdentityCredential, ExecutorPrepareError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(ExecutorPrepareError::Authentication);
        }
        let current = self
            .agent_identity
            .read()
            .map_err(|_| ExecutorPrepareError::Transient)?
            .clone()
            .ok_or(ExecutorPrepareError::Authentication)?;
        if current.task_id().is_some()
            && expected_task_id.is_none_or(|expected| current.task_id() != Some(expected))
        {
            return Ok(current);
        }

        let _guard = self.agent_task_lock.lock().await;
        let current = self
            .agent_identity
            .read()
            .map_err(|_| ExecutorPrepareError::Transient)?
            .clone()
            .ok_or(ExecutorPrepareError::Authentication)?;
        if current.task_id().is_some()
            && expected_task_id.is_none_or(|expected| current.task_id() != Some(expected))
        {
            return Ok(current);
        }
        let task_id = current
            .register_task(&self.clients.http)
            .await
            .map_err(classify_agent_identity_error)?;
        let task_id = self
            .persistence_adapter
            .persist_agent_task_id_for_identity(&self.id, &current, &task_id)
            .await
            .map_err(|_| ExecutorPrepareError::Persistence)?;
        let updated = current
            .with_task_id(task_id)
            .map_err(|_| ExecutorPrepareError::InvalidCredential)?;
        if !self.active.load(Ordering::Acquire) {
            return Err(ExecutorPrepareError::Authentication);
        }
        let mut identity = self
            .agent_identity
            .write()
            .map_err(|_| ExecutorPrepareError::Transient)?;
        *identity = Some(updated.clone());
        self.agent_identity_revision.fetch_add(1, Ordering::Release);
        Ok(updated)
    }
}

fn classify_agent_identity_error(error: AgentIdentityError) -> ExecutorPrepareError {
    match error {
        AgentIdentityError::RegistrationTransport => ExecutorPrepareError::Transient,
        AgentIdentityError::RegistrationRejected => ExecutorPrepareError::Authentication,
        _ => ExecutorPrepareError::InvalidCredential,
    }
}

async fn inspect_agent_identity_unauthorized(
    response: reqwest::Response,
) -> std::result::Result<(reqwest::Response, bool), AuthorizedRequestError> {
    let status = response.status();
    let version = response.version();
    let headers = response.headers().clone();
    let body = response
        .bytes()
        .await
        .map_err(AuthorizedRequestError::Transport)?;
    let invalid = is_agent_identity_task_invalid_response(status.as_u16(), &body);
    let mut restored = axum::http::Response::builder()
        .status(status)
        .version(version)
        .body(reqwest::Body::from(body))
        .map_err(|_| AuthorizedRequestError::NotReplayable)?;
    *restored.headers_mut() = headers;
    Ok((reqwest::Response::from(restored), invalid))
}

impl GatewayRuntime {
    pub(crate) fn routing_cookies(
        &self,
        candidate_id: &str,
        prepared: &PreparedAuthorization,
    ) -> Option<std::sync::Arc<super::routing_cookies::RoutingCookieJar>> {
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

// Agent assertions are signed anew each second. Bind state to their credential
// and task, not the timestamp/signature of an individual request.
fn agent_credential_fingerprint(agent: &AgentIdentityCredential) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"relay-agent-turn-state-v1");
    for part in [
        agent.private_key(),
        agent.runtime_id(),
        agent.task_id().unwrap_or_default(),
    ] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    digest.finalize().into()
}

impl PreparedAuthorization {
    pub(crate) fn incarnation(&self) -> AuthorizationIncarnation {
        if let Some(revision) = &self.token_revision {
            AuthorizationIncarnation::OAuth(revision.clone())
        } else if let Some(revision) = self.agent_identity_revision {
            AuthorizationIncarnation::Agent(revision)
        } else {
            AuthorizationIncarnation::Source
        }
    }

    /// Hold the credential's incarnation through the same budget/scheduler
    /// transaction that starts a generation. No async lock or provider I/O is
    /// performed under this guard.
    pub(super) fn dispatch_guard<'a>(
        &'a self,
        runtime: &'a GatewayRuntime,
        candidate_id: &str,
    ) -> Option<PreparedAuthorizationDispatchGuard<'a>> {
        if let Some(revision) = &self.token_revision {
            let account = runtime.chatgpt_accounts.get(candidate_id)?;
            if !account.active.load(Ordering::Acquire) {
                return None;
            }
            return Some(PreparedAuthorizationDispatchGuard {
                _token: Some(revision.guard()?),
                _agent: None,
            });
        }
        if let Some(expected) = self.agent_identity_revision {
            let account = runtime.chatgpt_accounts.get(candidate_id)?;
            let identity = account.agent_identity.read().ok()?;
            if !account.active.load(Ordering::Acquire)
                || account.agent_identity_revision.load(Ordering::Acquire) != expected
                || identity.as_ref().map(agent_credential_fingerprint)
                    != self.agent_credential_fingerprint
            {
                return None;
            }
            return Some(PreparedAuthorizationDispatchGuard {
                _token: None,
                _agent: Some(identity),
            });
        }
        runtime.source_candidate_bindings.get(candidate_id)?;
        Some(PreparedAuthorizationDispatchGuard {
            _token: None,
            _agent: None,
        })
    }

    pub(crate) fn credential_fingerprint(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        Sha256::digest(self.turn_state_credential()).into()
    }

    fn turn_state_credential(&self) -> &[u8] {
        self.agent_credential_fingerprint.as_ref().map_or_else(
            || self.authorization.as_bytes(),
            |fingerprint| fingerprint.as_slice(),
        )
    }
}

/// Build the request before adding account authorization so the existing
/// downstream headers can be inspected and preserved. `RequestBuilder::headers`
/// cannot express that distinction: applying a second header map replaces the
/// client's identity even when it was already valid.
fn apply_prepared_authorization(
    request: reqwest::RequestBuilder,
    prepared: &PreparedAuthorization,
    client_version: Option<&str>,
) -> std::result::Result<(reqwest::Client, reqwest::Request), AuthorizedRequestError> {
    let (client, request) = request.build_split();
    let mut request = request.map_err(AuthorizedRequestError::Transport)?;
    request
        .headers_mut()
        .insert(prepared.header_name.clone(), prepared.authorization.clone());
    if let Some(identity) = prepared.identity.as_ref() {
        // A model-catalog request has no forwarded client headers, so its
        // requested version is a useful fallback. For normal routed requests
        // the explicit downstream identity remains authoritative.
        let identity = match client_version {
            Some(version) => identity
                .with_client_version(version)
                .map_err(|_| AuthorizedRequestError::NotReplayable)?,
            None => identity.clone(),
        };
        identity.insert(request.headers_mut());
    }
    Ok((client, request))
}

fn classify_token_authority_error(error: TokenAuthorityError) -> ExecutorPrepareError {
    match error {
        TokenAuthorityError::AccessTokenExpired
        | TokenAuthorityError::RequiresReauth(_)
        | TokenAuthorityError::AccountNotFound
        | TokenAuthorityError::InvalidAccountId => ExecutorPrepareError::Authentication,
        TokenAuthorityError::PersistenceRequired | TokenAuthorityError::PersistenceFailed(_) => {
            ExecutorPrepareError::Persistence
        }
        TokenAuthorityError::RefreshFailed(_)
        | TokenAuthorityError::InvalidCapacity
        | TokenAuthorityError::CapacityReached => ExecutorPrepareError::Transient,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_turn_state_identity_ignores_signature_timestamp_but_tracks_credentials() {
        let agent = AgentIdentityCredential::new(
            "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g".into(),
            "synthetic-runtime".into(),
            "synthetic-task".into(),
        )
        .unwrap();
        let prepare = |agent: &AgentIdentityCredential, timestamp| PreparedAuthorization {
            header_name: AUTHORIZATION,
            authorization: agent.authorization(timestamp).unwrap(),
            identity: None,
            token_generation: None,
            token_revision: None,
            agent_task_id: agent.task_id().map(str::to_string),
            agent_credential_fingerprint: Some(agent_credential_fingerprint(agent)),
            agent_identity_revision: Some(0),
        };
        let first = prepare(&agent, 1_000);
        let next = prepare(&agent, 2_000);
        assert_ne!(first.authorization, next.authorization);
        assert_eq!(first.turn_state_credential(), next.turn_state_credential());
        let changed_task = prepare(&agent.with_task_id("another-task".into()).unwrap(), 2_000);
        assert_ne!(
            first.turn_state_credential(),
            changed_task.turn_state_credential()
        );
        let changed_runtime = AgentIdentityCredential::new(
            agent.private_key().into(),
            "another-runtime".into(),
            "synthetic-task".into(),
        )
        .unwrap();
        assert_ne!(
            first.turn_state_credential(),
            prepare(&changed_runtime, 2_000).turn_state_credential()
        );
    }
}
