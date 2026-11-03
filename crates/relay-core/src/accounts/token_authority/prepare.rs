use super::*;

impl TokenAuthority {
    pub async fn prepare(
        &self,
        account_id: &str,
        now_ms: u64,
        refresh_skew_ms: u64,
        adapter: &dyn TokenRefreshAdapter,
    ) -> Result<PreparedToken, TokenAuthorityError> {
        self.prepare_inner(account_id, now_ms, refresh_skew_ms, adapter, None)
            .await
    }

    pub async fn prepare_and_persist(
        &self,
        account_id: &str,
        now_ms: u64,
        refresh_skew_ms: u64,
        adapter: &dyn TokenRefreshAdapter,
        persistence: &dyn TokenPersistenceAdapter,
    ) -> Result<PreparedToken, TokenAuthorityError> {
        self.prepare_inner(
            account_id,
            now_ms,
            refresh_skew_ms,
            adapter,
            Some(persistence),
        )
        .await
    }

    async fn prepare_inner(
        &self,
        account_id: &str,
        now_ms: u64,
        refresh_skew_ms: u64,
        adapter: &dyn TokenRefreshAdapter,
        persistence: Option<&dyn TokenPersistenceAdapter>,
    ) -> Result<PreparedToken, TokenAuthorityError> {
        let slot_handle = lock(&self.slots)
            .get(account_id)
            .cloned()
            .ok_or(TokenAuthorityError::AccountNotFound)?;
        let mut slot = slot_handle.lock().await;
        self.ensure_current_slot(account_id, &slot_handle)?;
        if slot.persistence_pending {
            let persistence = persistence.ok_or(TokenAuthorityError::PersistenceRequired)?;
            let revision = slot_handle.snapshot();
            persistence
                .persist_fenced(account_id, &slot.tokens, &revision)
                .await
                .map_err(|failure| TokenAuthorityError::PersistenceFailed(failure.code))?;
            self.ensure_current_slot(account_id, &slot_handle)?;
            slot.persistence_pending = false;
            slot.auth_state_persistence_pending = true;
        }
        if slot.auth_state_persistence_pending {
            let persistence = persistence.ok_or(TokenAuthorityError::PersistenceRequired)?;
            let revision = slot_handle.snapshot();
            persistence
                .persist_auth_state_fenced(account_id, slot.auth_state, &revision)
                .await
                .map_err(|failure| TokenAuthorityError::PersistenceFailed(failure.code))?;
            self.ensure_current_slot(account_id, &slot_handle)?;
            slot.auth_state_persistence_pending = false;
        }
        if matches!(
            slot.auth_state,
            AccountAuthState::RequiresReauth(ReauthReason::ReusedRefreshToken)
        ) {
            // Older Relay versions persisted this transient OAuth race as a
            // hard reauthentication state. Heal the record before selecting a
            // token so an update can retry or use the still-valid access token.
            slot_handle.bump();
            slot.auth_state = AccountAuthState::Active;
            persist_auth_state(account_id, &mut slot, persistence, &slot_handle.snapshot()).await?;
            self.ensure_current_slot(account_id, &slot_handle)?;
        }
        if let AccountAuthState::RequiresReauth(reason) = slot.auth_state {
            return Err(TokenAuthorityError::RequiresReauth(reason));
        }
        if slot.tokens.is_access_usable(now_ms, refresh_skew_ms) {
            return Ok(PreparedToken {
                status: PrepareStatus::Ready,
                tokens: slot.tokens.clone(),
                dispatch_revision: slot_handle.snapshot(),
            });
        }
        let Some(refresh_token) = slot.tokens.refresh_token.clone() else {
            slot_handle.bump();
            slot.auth_state = AccountAuthState::DegradedAccessOnly;
            persist_auth_state(account_id, &mut slot, persistence, &slot_handle.snapshot()).await?;
            self.ensure_current_slot(account_id, &slot_handle)?;
            return Err(TokenAuthorityError::AccessTokenExpired);
        };

        let previous_auth_state = slot.auth_state;
        slot_handle.bump();
        slot.auth_state = AccountAuthState::Refreshing;
        let refresh_revision = slot_handle.snapshot();
        match adapter
            .refresh_fenced(account_id, &refresh_token, now_ms, &refresh_revision)
            .await
        {
            Ok(refreshed) => {
                let tokens = TokenSet {
                    access_token: refreshed.access_token,
                    refresh_token: refreshed.refresh_token.or(Some(refresh_token)),
                    id_token: refreshed.id_token.or_else(|| slot.tokens.id_token.clone()),
                    expires_at_ms: refreshed.expires_at_ms,
                    issued_at_ms: now_ms,
                    generation: slot.tokens.generation.saturating_add(1),
                };
                {
                    let slots = lock(&self.slots);
                    if slots
                        .get(account_id)
                        .is_none_or(|registered| !Arc::ptr_eq(registered, &slot_handle))
                    {
                        return Err(TokenAuthorityError::AccountNotFound);
                    }
                    slot_handle.bump();
                    slot.tokens = tokens.clone();
                    slot.auth_state = AccountAuthState::Active;
                }
                if let Some(persistence) = persistence {
                    slot.persistence_pending = true;
                    let revision = slot_handle.snapshot();
                    persistence
                        .persist_fenced(account_id, &slot.tokens, &revision)
                        .await
                        .map_err(|failure| TokenAuthorityError::PersistenceFailed(failure.code))?;
                    self.ensure_current_slot(account_id, &slot_handle)?;
                    slot.persistence_pending = false;
                    persist_auth_state(
                        account_id,
                        &mut slot,
                        Some(persistence),
                        &slot_handle.snapshot(),
                    )
                    .await?;
                    self.ensure_current_slot(account_id, &slot_handle)?;
                }
                Ok(PreparedToken {
                    status: PrepareStatus::Refreshed,
                    tokens,
                    dispatch_revision: slot_handle.snapshot(),
                })
            }
            Err(failure) => {
                self.ensure_current_slot(account_id, &slot_handle)?;
                if let Some(reason) = failure.reauth_reason() {
                    slot.auth_state = AccountAuthState::RequiresReauth(reason);
                    persist_auth_state(account_id, &mut slot, persistence, &slot_handle.snapshot())
                        .await?;
                    self.ensure_current_slot(account_id, &slot_handle)?;
                    Err(TokenAuthorityError::RequiresReauth(reason))
                } else {
                    // Network, timeout, lock, and temporary storage failures do
                    // not change whether the account credentials are valid.
                    // Keep the last durable auth state so an offline launch
                    // cannot turn a usable account into a permanent auth error.
                    slot.auth_state = previous_auth_state;
                    Err(TokenAuthorityError::RefreshFailed(failure.code))
                }
            }
        }
    }
}
