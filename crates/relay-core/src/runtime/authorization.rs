use super::{
    runtime_now_ms, AuthorizedRequestError, ChatGptAccountExecutor, ExecutorPrepareError,
    GatewayRuntime, PreparedAuthorization,
};
use crate::accounts::TokenAuthorityError;
use crate::providers::chatgpt::{
    is_agent_identity_task_invalid_response, AgentIdentityCredential, AgentIdentityError,
};
use crate::CandidateHealth;
use reqwest::header::{HeaderValue, AUTHORIZATION};
use reqwest::StatusCode;
use std::sync::atomic::Ordering;

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
                agent_task_id: None,
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
                        identity: Some(account.identity.clone()),
                        token_generation: None,
                        agent_task_id: agent.task_id().map(str::to_string),
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
            identity: Some(account.identity.clone()),
            token_generation: Some(prepared.tokens.generation()),
            agent_task_id: None,
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
    ) -> std::result::Result<reqwest::Response, AuthorizedRequestError> {
        let first_request = request
            .try_clone()
            .ok_or(AuthorizedRequestError::NotReplayable)?;
        let prepared = self
            .prepare_authorization(candidate_id, runtime_now_ms())
            .await
            .map_err(AuthorizedRequestError::Prepare)?;
        let response = apply_prepared_authorization(first_request, &prepared, client_version)?
            .send()
            .await
            .map_err(AuthorizedRequestError::Transport)?;
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
                    return Ok(response);
                }
                drop(response);
                let refreshed = self
                    .refresh_agent_identity_task_after_unauthorized(
                        candidate_id,
                        task_id,
                        runtime_now_ms(),
                    )
                    .await
                    .map_err(AuthorizedRequestError::Prepare)?;
                let response = apply_prepared_authorization(request, &refreshed, client_version)?
                    .send()
                    .await
                    .map_err(AuthorizedRequestError::Transport)?;
                self.observe_codex_quota_headers(
                    candidate_id,
                    response.status(),
                    response.headers(),
                    runtime_now_ms(),
                );
                return Ok(response);
            }
        }
        if response.status() != StatusCode::UNAUTHORIZED || prepared.token_generation.is_none() {
            self.observe_codex_quota_headers(
                candidate_id,
                response.status(),
                response.headers(),
                runtime_now_ms(),
            );
            return Ok(response);
        }

        drop(response);
        let _fence = self.fence_execution(candidate_id);
        let refreshed = self
            .refresh_authorization_after_unauthorized(
                candidate_id,
                prepared.token_generation,
                runtime_now_ms(),
            )
            .await
            .map_err(AuthorizedRequestError::Prepare)?;
        let response = apply_prepared_authorization(request, &refreshed, client_version)?
            .send()
            .await
            .map_err(AuthorizedRequestError::Transport)?;
        self.observe_codex_quota_headers(
            candidate_id,
            response.status(),
            response.headers(),
            runtime_now_ms(),
        );
        Ok(response)
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
            .register_task(&self.clients.bounded)
            .await
            .map_err(classify_agent_identity_error)?;
        let task_id = self
            .persistence_adapter
            .persist_agent_task_id(&self.id, current.task_id(), &task_id)
            .await
            .map_err(|_| ExecutorPrepareError::Persistence)?;
        let updated = current
            .with_task_id(task_id)
            .map_err(|_| ExecutorPrepareError::InvalidCredential)?;
        if !self.active.load(Ordering::Acquire) {
            return Err(ExecutorPrepareError::Authentication);
        }
        *self
            .agent_identity
            .write()
            .map_err(|_| ExecutorPrepareError::Transient)? = Some(updated.clone());
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

fn apply_prepared_authorization(
    request: reqwest::RequestBuilder,
    prepared: &PreparedAuthorization,
    client_version: Option<&str>,
) -> std::result::Result<reqwest::RequestBuilder, AuthorizedRequestError> {
    let request = request.header(prepared.header_name.clone(), prepared.authorization.clone());
    let Some(identity) = prepared.identity.as_ref() else {
        return Ok(request);
    };
    let identity = match client_version {
        Some(version) => identity
            .with_client_version(version)
            .map_err(|_| AuthorizedRequestError::NotReplayable)?,
        None => identity.clone(),
    };
    Ok(identity.apply(request))
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
