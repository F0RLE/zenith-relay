use super::*;

impl TokenAuthority {
    pub async fn register(
        &self,
        account_id: &str,
        tokens: TokenSet,
        auth_state: AccountAuthState,
    ) -> Result<(), TokenAuthorityError> {
        match self.prepare_slot(account_id, TokenSlot::fresh(tokens, auth_state))? {
            PreparedTokenSlot::Inserted => Ok(()),
            PreparedTokenSlot::Existing { slot, candidate } => {
                let mut slot_state = slot.lock().await;
                let _slots = self.current_slots(account_id, &slot)?;
                slot.bump();
                *slot_state = candidate;
                Ok(())
            }
        }
    }

    /// Registers a credential snapshot only when it is newer than the
    /// authority's current token generation. Request preparation uses this to
    /// initialize a missing slot without clearing an in-flight refresh,
    /// pending persistence, or a terminal authentication state from a newer
    /// in-memory result.
    pub async fn register_if_newer(
        &self,
        account_id: &str,
        tokens: TokenSet,
        auth_state: AccountAuthState,
    ) -> Result<bool, TokenAuthorityError> {
        match self.prepare_slot(account_id, TokenSlot::fresh(tokens, auth_state))? {
            PreparedTokenSlot::Inserted => Ok(true),
            PreparedTokenSlot::Existing { slot, candidate } => {
                let mut existing = slot.lock().await;
                let _slots = self.current_slots(account_id, &slot)?;
                if !token_set_is_newer(&candidate.tokens, &existing.tokens) {
                    return Ok(false);
                }
                slot.bump();
                *existing = candidate;
                Ok(true)
            }
        }
    }

    /// Registers a token snapshot unless an in-memory refresh has already
    /// produced an unmistakably newer generation. Desktop-profile import uses
    /// this after releasing its cross-process credential lock, so it cannot
    /// roll back a concurrent automatic refresh.
    pub async fn register_if_not_stale(
        &self,
        account_id: &str,
        tokens: TokenSet,
        auth_state: AccountAuthState,
    ) -> Result<bool, TokenAuthorityError> {
        match self.prepare_slot(account_id, TokenSlot::fresh(tokens, auth_state))? {
            PreparedTokenSlot::Inserted => Ok(true),
            PreparedTokenSlot::Existing { slot, candidate } => {
                let mut existing = slot.lock().await;
                let _slots = self.current_slots(account_id, &slot)?;
                if token_set_is_newer(&existing.tokens, &candidate.tokens) {
                    return Ok(false);
                }
                slot.bump();
                *existing = candidate;
                Ok(true)
            }
        }
    }

    /// Replaces an authority slot only while it still contains the exact
    /// token/authentication state installed by the caller. Compensating
    /// account mutations use this instead of an unconditional `register` so a
    /// delayed rollback cannot replace a login or refresh that completed
    /// meanwhile.
    pub async fn replace_if_current(
        &self,
        account_id: &str,
        expected_tokens: &TokenSet,
        expected_auth_state: AccountAuthState,
        replacement_tokens: TokenSet,
        replacement_auth_state: AccountAuthState,
    ) -> Result<bool, TokenAuthorityError> {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return Err(TokenAuthorityError::InvalidAccountId);
        }
        let slot_handle = { lock(&self.slots).get(account_id).cloned() };
        let Some(slot_handle) = slot_handle else {
            return Ok(false);
        };
        let mut slot = slot_handle.lock().await;
        if slot.auth_state != expected_auth_state || slot.tokens != *expected_tokens {
            return Ok(false);
        }
        // A failed mutation's rollback is a new authorization incarnation,
        // even when it restores the same persisted generation and bearer.
        {
            let slots = lock(&self.slots);
            if slots
                .get(account_id)
                .is_none_or(|registered_slot| !Arc::ptr_eq(registered_slot, &slot_handle))
            {
                return Ok(false);
            }
            slot_handle.bump();
        }
        *slot = TokenSlot {
            tokens: replacement_tokens,
            auth_state: replacement_auth_state,
            persistence_pending: false,
            auth_state_persistence_pending: false,
        };
        Ok(true)
    }

    /// Removes a newly-created authority slot only while it still belongs to
    /// the failed mutation. A concurrent login/refresh may reuse the same
    /// account id, so removing by id alone is not safe.
    pub async fn remove_if_current(
        &self,
        account_id: &str,
        expected_tokens: &TokenSet,
        expected_auth_state: AccountAuthState,
    ) -> Result<bool, TokenAuthorityError> {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return Err(TokenAuthorityError::InvalidAccountId);
        }
        let slot = { lock(&self.slots).get(account_id).cloned() };
        let Some(slot) = slot else {
            return Ok(false);
        };
        let slot_guard = slot.lock().await;
        if slot_guard.auth_state != expected_auth_state || slot_guard.tokens != *expected_tokens {
            return Ok(false);
        }
        // All authority operations take the map lock only long enough to copy
        // an Arc, then await the slot lock. Holding this slot lock while
        // checking the Arc therefore prevents a remove/re-register race.
        let mut slots = lock(&self.slots);
        if slots
            .get(account_id)
            .is_none_or(|registered_slot| !Arc::ptr_eq(registered_slot, &slot))
        {
            return Ok(false);
        }
        slot.retire();
        slots.remove(account_id);
        Ok(true)
    }

    pub fn register_if_absent(
        &self,
        account_id: &str,
        tokens: TokenSet,
        auth_state: AccountAuthState,
    ) -> Result<bool, TokenAuthorityError> {
        Ok(matches!(
            self.prepare_slot(account_id, TokenSlot::fresh(tokens, auth_state))?,
            PreparedTokenSlot::Inserted
        ))
    }

    pub fn remove(&self, account_id: &str) -> bool {
        let mut slots = lock(&self.slots);
        if let Some(slot) = slots.get(account_id) {
            slot.retire();
        }
        slots.remove(account_id).is_some()
    }

    pub fn len(&self) -> usize {
        lock(&self.slots).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub async fn auth_state(&self, account_id: &str) -> Option<AccountAuthState> {
        let slot = lock(&self.slots).get(account_id).cloned()?;
        let slot_state = slot.lock().await;
        let _slots = self.current_slots(account_id, &slot).ok()?;
        Some(slot_state.auth_state)
    }

    pub async fn tokens(&self, account_id: &str) -> Option<TokenSet> {
        let slot = lock(&self.slots).get(account_id).cloned()?;
        let slot_state = slot.lock().await;
        let _slots = self.current_slots(account_id, &slot).ok()?;
        Some(slot_state.tokens.clone())
    }

    pub async fn invalidate_access_and_persist(
        &self,
        account_id: &str,
        now_ms: u64,
        persistence: &dyn TokenPersistenceAdapter,
    ) -> Result<(), TokenAuthorityError> {
        self.invalidate_access_generation_and_persist(account_id, None, now_ms, persistence)
            .await
            .map(|_| ())
    }

    pub async fn invalidate_access_generation_and_persist(
        &self,
        account_id: &str,
        failed_generation: Option<u64>,
        now_ms: u64,
        persistence: &dyn TokenPersistenceAdapter,
    ) -> Result<bool, TokenAuthorityError> {
        self.invalidate_access_with_fence(account_id, failed_generation, None, now_ms, persistence)
            .await
    }

    /// A remote 401 belongs to the exact bearer that was sent. Generation
    /// alone can be reused by a replacement login with the same account id.
    pub async fn invalidate_access_if_current_and_persist(
        &self,
        account_id: &str,
        rejected_tokens: &TokenSet,
        now_ms: u64,
        persistence: &dyn TokenPersistenceAdapter,
    ) -> Result<bool, TokenAuthorityError> {
        self.invalidate_access_with_fence(
            account_id,
            None,
            Some(rejected_tokens),
            now_ms,
            persistence,
        )
        .await
    }

    async fn invalidate_access_with_fence(
        &self,
        account_id: &str,
        failed_generation: Option<u64>,
        rejected_tokens: Option<&TokenSet>,
        now_ms: u64,
        persistence: &dyn TokenPersistenceAdapter,
    ) -> Result<bool, TokenAuthorityError> {
        let slot_handle = lock(&self.slots)
            .get(account_id)
            .cloned()
            .ok_or(TokenAuthorityError::AccountNotFound)?;
        let mut slot = slot_handle.lock().await;
        if failed_generation.is_some_and(|generation| slot.tokens.generation != generation)
            || rejected_tokens.is_some_and(|tokens| slot.tokens != *tokens)
        {
            return Ok(false);
        }
        {
            let slots = lock(&self.slots);
            if slots
                .get(account_id)
                .is_none_or(|registered_slot| !Arc::ptr_eq(registered_slot, &slot_handle))
            {
                return Ok(false);
            }
            slot_handle.bump();
        }
        slot.tokens.expires_at_ms = Some(now_ms);
        slot.tokens.issued_at_ms = now_ms;
        slot.tokens.generation = slot.tokens.generation.saturating_add(1);
        slot.persistence_pending = true;
        let revision = slot_handle.snapshot();
        persistence
            .persist_fenced(account_id, &slot.tokens, &revision)
            .await
            .map_err(|failure| TokenAuthorityError::PersistenceFailed(failure.code))?;
        self.ensure_current_slot(account_id, &slot_handle)?;
        slot.persistence_pending = false;
        Ok(true)
    }
}
