use super::*;

impl ServerTokenPersistence {
    pub(crate) fn for_account(state: Arc<AppState>, record: &ServerAccountRecord) -> Self {
        Self {
            state,
            secret_refs: HashMap::from([(record.id.clone(), record.secret_ref.clone())]),
        }
    }

    fn expected_ref(&self, account_id: &str) -> Result<&str, TokenPersistenceFailure> {
        self.secret_refs
            .get(account_id)
            .map(String::as_str)
            .ok_or_else(|| TokenPersistenceFailure::new(error_codes::PERSISTENCE_FAILED))
    }

    fn current_ref(&self, account_id: &str) -> Result<&str, TokenPersistenceFailure> {
        let expected = self.expected_ref(account_id)?;
        let record = find_account(&self.state, account_id).map_err(persistence_error)?;
        if record.secret_ref != expected {
            return Err(TokenPersistenceFailure::new(
                error_codes::PERSISTENCE_FAILED,
            ));
        }
        Ok(expected)
    }

    /// Call only under account_credential_lock so an import cannot change the
    /// record/reference between validation, vault load and the eventual save.
    fn load_current_credential(
        &self,
        account_id: &str,
    ) -> Result<(&str, AccountCredential), TokenPersistenceFailure> {
        let secret_ref = self.current_ref(account_id)?;
        let secret = self
            .state
            .vault
            .load(secret_ref)
            .map_err(persistence_error)?
            .ok_or_else(|| TokenPersistenceFailure::new(error_codes::SECRET_MISSING))?;
        let credential = serde_json::from_str(&secret)
            .map_err(|_| TokenPersistenceFailure::new(error_codes::SECRET_INVALID))?;
        Ok((secret_ref, credential))
    }

    async fn persist_agent_task_inner(
        &self,
        account_id: &str,
        expected_task_id: Option<&str>,
        expected_identity: Option<&AgentIdentityCredential>,
        task_id: &str,
    ) -> Result<String, TokenPersistenceFailure> {
        let _credential = self.state.account_credential_lock.lock().await;
        let (secret_ref, mut credential) = self.load_current_credential(account_id)?;
        let agent = credential
            .agent_identity()
            .map_err(persistence_error)?
            .ok_or_else(|| TokenPersistenceFailure::new(error_codes::NOT_AGENT_IDENTITY))?;
        if expected_identity.is_some_and(|expected| {
            agent.private_key() != expected.private_key()
                || agent.runtime_id() != expected.runtime_id()
                || expected.task_id() != expected_task_id
        }) {
            return Err(TokenPersistenceFailure::new(
                error_codes::PERSISTENCE_FAILED,
            ));
        }
        if let Some(current_task_id) = agent
            .task_id()
            .filter(|current_task_id| Some(*current_task_id) != expected_task_id)
        {
            return Ok(current_task_id.to_string());
        }
        credential.agent_task_id = Some(task_id.to_string());
        let encoded = serde_json::to_string(&credential)
            .map_err(|_| TokenPersistenceFailure::new(error_codes::SECRET_SERIALIZE))?;
        self.state
            .vault
            .save(secret_ref, &encoded)
            .map_err(persistence_error)?;
        Ok(task_id.to_string())
    }
}

impl TokenPersistenceAdapter for ServerTokenPersistence {
    fn persist<'a>(
        &'a self,
        account_id: &'a str,
        tokens: &'a TokenSet,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        Box::pin(async move {
            let _credential = self.state.account_credential_lock.lock().await;
            let (secret_ref, mut credential) = self.load_current_credential(account_id)?;
            credential.access_token = tokens.access_token().to_string();
            credential.refresh_token = tokens.refresh_token().map(str::to_string);
            credential.id_token = tokens.id_token().map(str::to_string);
            credential.expires_at_ms = tokens.expires_at_ms();
            credential.issued_at_ms = tokens.issued_at_ms();
            credential.generation = tokens.generation();
            let encoded = serde_json::to_string(&credential)
                .map_err(|_| TokenPersistenceFailure::new(error_codes::SECRET_SERIALIZE))?;
            self.state
                .vault
                .save(secret_ref, &encoded)
                .map_err(persistence_error)
        })
    }

    fn persist_auth_state<'a>(
        &'a self,
        account_id: &'a str,
        auth_state: AccountAuthState,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        Box::pin(async move {
            let _credential = self.state.account_credential_lock.lock().await;
            let expected = self.expected_ref(account_id)?;
            self.state
                .store
                .update_account(account_id, |record| {
                    if record.secret_ref != expected {
                        return Err("account login changed during token refresh".into());
                    }
                    record.auth_state = auth_state;
                    Ok(())
                })
                .map_err(persistence_error)?
                .ok_or_else(|| TokenPersistenceFailure::new(error_codes::ACCOUNT_MISSING))
        })
    }

    fn persist_agent_task_id<'a>(
        &'a self,
        account_id: &'a str,
        expected_task_id: Option<&'a str>,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<String, TokenPersistenceFailure>> {
        Box::pin(self.persist_agent_task_inner(account_id, expected_task_id, None, task_id))
    }

    fn persist_agent_task_id_for_identity<'a>(
        &'a self,
        account_id: &'a str,
        expected: &'a AgentIdentityCredential,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<String, TokenPersistenceFailure>> {
        Box::pin(self.persist_agent_task_inner(
            account_id,
            expected.task_id(),
            Some(expected),
            task_id,
        ))
    }
}

fn persistence_error(error: String) -> TokenPersistenceFailure {
    let _ = error;
    TokenPersistenceFailure::new(error_codes::PERSISTENCE_FAILED)
}
