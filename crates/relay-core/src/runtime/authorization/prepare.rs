use super::super::{
    AuthorizedRequestError, ChatGptAccountExecutor, ExecutorPrepareError, GatewayRuntime,
    PreparedAuthorization,
};
use crate::accounts::TokenAuthorityError;
use crate::providers::chatgpt::{
    is_agent_identity_task_invalid_response, AgentIdentityCredential, AgentIdentityError,
};
use crate::CandidateHealth;
use reqwest::header::{HeaderValue, AUTHORIZATION};
use std::sync::atomic::Ordering;

impl GatewayRuntime {
    pub(crate) async fn prepare_authorization(
        &self,
        candidate_id: &str,
        now_ms: u64,
    ) -> std::result::Result<PreparedAuthorization, ExecutorPrepareError> {
        if let Some(binding) = self.source_candidate_bindings.get(candidate_id) {
            let provider_source = self
                .sources
                .get(&binding.source_id)
                .ok_or(ExecutorPrepareError::Authentication)?;
            let source_binding = provider_source
                .binding_for(binding.binding_key)
                .ok_or(ExecutorPrepareError::Authentication)?;
            let (header_name, authorization) =
                provider_source.authorization_for_binding(source_binding);
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
}

impl ChatGptAccountExecutor {
    async fn ensure_agent_identity_task(
        &self,
        expected_task_id: Option<&str>,
    ) -> std::result::Result<AgentIdentityCredential, ExecutorPrepareError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(ExecutorPrepareError::Authentication);
        }
        let (agent_identity, ready) = self.current_agent_identity(expected_task_id)?;
        if ready {
            return Ok(agent_identity);
        }

        let _guard = self.agent_task_lock.lock().await;
        let (agent_identity, ready) = self.current_agent_identity(expected_task_id)?;
        if ready {
            return Ok(agent_identity);
        }
        let task_id = agent_identity
            .register_task(&self.clients.http)
            .await
            .map_err(classify_agent_identity_error)?;
        let task_id = self
            .persistence_adapter
            .persist_agent_task_id_for_identity(&self.id, &agent_identity, &task_id)
            .await
            .map_err(|_| ExecutorPrepareError::Persistence)?;
        let updated = agent_identity
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

    fn current_agent_identity(
        &self,
        expected_task_id: Option<&str>,
    ) -> std::result::Result<(AgentIdentityCredential, bool), ExecutorPrepareError> {
        let agent_identity = self
            .agent_identity
            .read()
            .map_err(|_| ExecutorPrepareError::Transient)?
            .clone()
            .ok_or(ExecutorPrepareError::Authentication)?;
        let ready = agent_identity.task_id().is_some()
            && expected_task_id.is_none_or(|expected| agent_identity.task_id() != Some(expected));
        Ok((agent_identity, ready))
    }
}

fn classify_agent_identity_error(error: AgentIdentityError) -> ExecutorPrepareError {
    match error {
        AgentIdentityError::RegistrationTransport => ExecutorPrepareError::Transient,
        AgentIdentityError::RegistrationRejected => ExecutorPrepareError::Authentication,
        _ => ExecutorPrepareError::InvalidCredential,
    }
}

pub(super) async fn inspect_agent_identity_unauthorized(
    response: reqwest::Response,
) -> std::result::Result<(reqwest::Response, bool), AuthorizedRequestError> {
    let status = response.status();
    let version = response.version();
    let headers = response.headers().clone();
    let response_body = response
        .bytes()
        .await
        .map_err(AuthorizedRequestError::Transport)?;
    let invalid = is_agent_identity_task_invalid_response(status.as_u16(), &response_body);
    let mut restored = axum::http::Response::builder()
        .status(status)
        .version(version)
        .body(reqwest::Body::from(response_body))
        .map_err(|_| AuthorizedRequestError::NotReplayable)?;
    *restored.headers_mut() = headers;
    Ok((reqwest::Response::from(restored), invalid))
}

// Agent assertions are signed anew each second. Bind state to their credential
// and task, not the timestamp/signature of an individual request.
pub(super) fn agent_credential_fingerprint(agent: &AgentIdentityCredential) -> [u8; 32] {
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
