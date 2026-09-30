use super::super::{credentials::CredentialStore, import_session::SecretBackend};
use super::lock::{ProcessAccountLocks, ProcessLockConfig, ProcessLockError};
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};
use zenith_relay_core::accounts::{
    AccountAuthState, TokenDispatchRevision, TokenPersistenceAdapter, TokenPersistenceFailure,
    TokenSet,
};
use zenith_relay_core::error_codes;
use zenith_relay_core::providers::chatgpt::AgentIdentityCredential;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MetadataSinkError;

pub trait AccountMetadataSink: Send + Sync {
    fn persist_generation<'a>(
        &'a self,
        local_account_id: &'a str,
        generation: u64,
        updated_at_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), MetadataSinkError>> + Send + 'a>>;

    fn persist_auth_state<'a>(
        &'a self,
        local_account_id: &'a str,
        auth_state: AccountAuthState,
    ) -> Pin<Box<dyn Future<Output = Result<(), MetadataSinkError>> + Send + 'a>>;
}

pub struct CredentialPersistence<B, M> {
    credentials: CredentialStore<B>,
    metadata: Arc<M>,
    locks: ProcessAccountLocks,
}

impl<B, M> CredentialPersistence<B, M> {
    pub fn new(credentials: CredentialStore<B>, metadata: Arc<M>, root: PathBuf) -> Self {
        Self {
            credentials,
            metadata,
            locks: ProcessAccountLocks::with_config(root, ProcessLockConfig::default())
                .expect("default credential lock config is valid"),
        }
    }
}

impl<B, M> CredentialPersistence<B, M>
where
    B: SecretBackend + Send + Sync,
    M: AccountMetadataSink,
{
    async fn persist_inner(
        &self,
        account_id: &str,
        tokens: &TokenSet,
        revision: Option<&TokenDispatchRevision>,
    ) -> Result<(), TokenPersistenceFailure> {
        let _lock = self
            .locks
            .acquire(account_id)
            .await
            .map_err(persistence_lock_failure)?;
        let stored = {
            // No await between the incarnation check, credential read and
            // write. Deletion and import also wait for the process lock.
            let _fence = revision
                .map(|revision| revision.guard().ok_or_else(superseded_persistence))
                .transpose()?;
            let current = self
                .credentials
                .require(account_id)
                .map_err(|_| TokenPersistenceFailure::new(error_codes::CREDENTIAL_LOAD_FAILED))?;
            if current.generation() > tokens.generation()
                || (current.generation() == tokens.generation()
                    && current.issued_at_ms() >= tokens.issued_at_ms())
            {
                current
            } else {
                let updated = current
                    .with_token_set(tokens)
                    .map_err(|_| TokenPersistenceFailure::new(error_codes::INVALID_TOKEN_SET))?;
                self.credentials.save(&updated).map_err(|_| {
                    TokenPersistenceFailure::new(error_codes::CREDENTIAL_PERSIST_FAILED)
                })?;
                updated
            }
        };
        self.metadata
            .persist_generation(account_id, stored.generation(), stored.issued_at_ms())
            .await
            .map_err(|_| TokenPersistenceFailure::new(error_codes::METADATA_PERSIST_FAILED))
    }

    async fn persist_auth_state_inner(
        &self,
        account_id: &str,
        auth_state: AccountAuthState,
        revision: Option<&TokenDispatchRevision>,
    ) -> Result<(), TokenPersistenceFailure> {
        let _lock = self
            .locks
            .acquire(account_id)
            .await
            .map_err(persistence_lock_failure)?;
        if revision.is_some_and(|revision| revision.guard().is_none()) {
            return Err(superseded_persistence());
        }
        // The process lock is retained through the metadata sink's async
        // runtime update; delete/import cannot reuse this account id meanwhile.
        self.metadata
            .persist_auth_state(account_id, auth_state)
            .await
            .map_err(|_| TokenPersistenceFailure::new(error_codes::METADATA_PERSIST_FAILED))
    }

    async fn persist_agent_task_inner(
        &self,
        account_id: &str,
        expected_task_id: Option<&str>,
        expected_identity: Option<&AgentIdentityCredential>,
        task_id: &str,
    ) -> Result<String, TokenPersistenceFailure> {
        let _lock = self
            .locks
            .acquire(account_id)
            .await
            .map_err(persistence_lock_failure)?;
        let current = self
            .credentials
            .require(account_id)
            .map_err(|_| TokenPersistenceFailure::new(error_codes::CREDENTIAL_LOAD_FAILED))?;
        let agent = current
            .agent_identity()
            .ok_or_else(superseded_persistence)?;
        if expected_identity.is_some_and(|expected| {
            agent.private_key() != expected.private_key()
                || agent.runtime_id() != expected.runtime_id()
                || expected.task_id() != expected_task_id
        }) {
            return Err(superseded_persistence());
        }
        if let Some(current_task_id) = agent
            .task_id()
            .filter(|current_task_id| Some(*current_task_id) != expected_task_id)
        {
            return Ok(current_task_id.to_string());
        }
        let updated = current
            .with_agent_task_id(task_id.to_string())
            .map_err(|_| TokenPersistenceFailure::new(error_codes::INVALID_AGENT_TASK_ID))?;
        self.credentials
            .save(&updated)
            .map_err(|_| TokenPersistenceFailure::new(error_codes::CREDENTIAL_PERSIST_FAILED))?;
        Ok(task_id.to_string())
    }
}

fn persistence_lock_failure(_: ProcessLockError) -> TokenPersistenceFailure {
    TokenPersistenceFailure::new(error_codes::PERSISTENCE_FAILED)
}

fn superseded_persistence() -> TokenPersistenceFailure {
    TokenPersistenceFailure::new(error_codes::PERSISTENCE_FAILED)
}

impl<B, M> TokenPersistenceAdapter for CredentialPersistence<B, M>
where
    B: SecretBackend + Send + Sync,
    M: AccountMetadataSink,
{
    fn persist<'a>(
        &'a self,
        local_account_id: &'a str,
        tokens: &'a TokenSet,
    ) -> Pin<Box<dyn Future<Output = Result<(), TokenPersistenceFailure>> + Send + 'a>> {
        Box::pin(self.persist_inner(local_account_id, tokens, None))
    }

    fn persist_fenced<'a>(
        &'a self,
        local_account_id: &'a str,
        tokens: &'a TokenSet,
        revision: &'a TokenDispatchRevision,
    ) -> Pin<Box<dyn Future<Output = Result<(), TokenPersistenceFailure>> + Send + 'a>> {
        Box::pin(self.persist_inner(local_account_id, tokens, Some(revision)))
    }

    fn persist_auth_state<'a>(
        &'a self,
        local_account_id: &'a str,
        auth_state: AccountAuthState,
    ) -> Pin<Box<dyn Future<Output = Result<(), TokenPersistenceFailure>> + Send + 'a>> {
        Box::pin(self.persist_auth_state_inner(local_account_id, auth_state, None))
    }

    fn persist_auth_state_fenced<'a>(
        &'a self,
        local_account_id: &'a str,
        auth_state: AccountAuthState,
        revision: &'a TokenDispatchRevision,
    ) -> Pin<Box<dyn Future<Output = Result<(), TokenPersistenceFailure>> + Send + 'a>> {
        Box::pin(self.persist_auth_state_inner(local_account_id, auth_state, Some(revision)))
    }

    fn persist_agent_task_id<'a>(
        &'a self,
        local_account_id: &'a str,
        expected_task_id: Option<&'a str>,
        task_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, TokenPersistenceFailure>> + Send + 'a>> {
        Box::pin(self.persist_agent_task_inner(local_account_id, expected_task_id, None, task_id))
    }

    fn persist_agent_task_id_for_identity<'a>(
        &'a self,
        local_account_id: &'a str,
        expected: &'a AgentIdentityCredential,
        task_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, TokenPersistenceFailure>> + Send + 'a>> {
        Box::pin(self.persist_agent_task_inner(
            local_account_id,
            expected.task_id(),
            Some(expected),
            task_id,
        ))
    }
}
