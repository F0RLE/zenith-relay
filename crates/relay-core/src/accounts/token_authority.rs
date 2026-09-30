use super::{AccountAuthState, ReauthReason};
use crate::providers::chatgpt::AgentIdentityCredential;
use futures_util::future::BoxFuture;
use futures_util::lock::Mutex as AsyncMutex;
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

mod failure;
mod revision;

pub use failure::{
    TokenAuthorityError, TokenPersistenceFailure, TokenRefreshFailure, TokenRefreshFailureKind,
};
use revision::DispatchRevisionState;
pub use revision::{TokenDispatchRevision, TokenDispatchRevisionGuard};

pub use set::{access_token_is_usable, TokenRefresh, TokenSet, TOKEN_REFRESH_SKEW_MS};

pub trait TokenRefreshAdapter: Send + Sync {
    fn refresh<'a>(
        &'a self,
        account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>>;

    /// Adapters that write credentials as part of refresh must override this
    /// method and hold the revision guard over their final synchronous write.
    /// Adapters that only perform the provider exchange can use the default.
    fn refresh_fenced<'a>(
        &'a self,
        account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
        revision: &'a TokenDispatchRevision,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        let _ = revision;
        self.refresh(account_id, refresh_token, now_ms)
    }
}

pub trait TokenPersistenceAdapter: Send + Sync {
    fn persist<'a>(
        &'a self,
        account_id: &'a str,
        tokens: &'a TokenSet,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>>;

    /// Secret stores with reusable account ids must override this and hold the
    /// revision guard over the final write after any asynchronous lock wait.
    fn persist_fenced<'a>(
        &'a self,
        account_id: &'a str,
        tokens: &'a TokenSet,
        revision: &'a TokenDispatchRevision,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        let _ = revision;
        self.persist(account_id, tokens)
    }

    fn persist_auth_state<'a>(
        &'a self,
        account_id: &'a str,
        auth_state: AccountAuthState,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>>;

    fn persist_auth_state_fenced<'a>(
        &'a self,
        account_id: &'a str,
        auth_state: AccountAuthState,
        revision: &'a TokenDispatchRevision,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        let _ = revision;
        self.persist_auth_state(account_id, auth_state)
    }

    fn persist_agent_task_id<'a>(
        &'a self,
        account_id: &'a str,
        expected_task_id: Option<&'a str>,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<String, TokenPersistenceFailure>>;

    /// Host adapters compare the entire identity after the provider call and
    /// before the durable task write, not only a possibly empty task id.
    fn persist_agent_task_id_for_identity<'a>(
        &'a self,
        account_id: &'a str,
        expected: &'a AgentIdentityCredential,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<String, TokenPersistenceFailure>> {
        self.persist_agent_task_id(account_id, expected.task_id(), task_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrepareStatus {
    Ready,
    Refreshed,
}

#[derive(Clone, Debug)]
pub struct PreparedToken {
    pub status: PrepareStatus,
    pub tokens: TokenSet,
    pub(crate) dispatch_revision: TokenDispatchRevision,
}

struct TokenSlot {
    tokens: TokenSet,
    auth_state: AccountAuthState,
    persistence_pending: bool,
    auth_state_persistence_pending: bool,
}

struct TokenSlotEntry {
    slot: AsyncMutex<TokenSlot>,
    revision: Arc<RwLock<DispatchRevisionState>>,
}

impl TokenSlotEntry {
    fn new(slot: TokenSlot) -> Self {
        Self {
            slot: AsyncMutex::new(slot),
            revision: Arc::new(RwLock::new(DispatchRevisionState {
                value: 0,
                active: true,
            })),
        }
    }

    fn bump(&self) {
        let mut revision = self
            .revision
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        revision.value = revision
            .value
            .checked_add(1)
            .expect("token revision exhausted");
    }

    fn retire(&self) {
        let mut revision = self
            .revision
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        revision.active = false;
    }

    fn snapshot(&self) -> TokenDispatchRevision {
        let expected = self
            .revision
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .value;
        TokenDispatchRevision {
            state: self.revision.clone(),
            expected,
        }
    }
}

impl Deref for TokenSlotEntry {
    type Target = AsyncMutex<TokenSlot>;

    fn deref(&self) -> &Self::Target {
        &self.slot
    }
}

impl TokenSlot {
    fn fresh(tokens: TokenSet, auth_state: AccountAuthState) -> Self {
        Self {
            tokens,
            auth_state,
            persistence_pending: false,
            auth_state_persistence_pending: false,
        }
    }
}

enum PreparedTokenSlot {
    Existing {
        slot: Arc<TokenSlotEntry>,
        candidate: TokenSlot,
    },
    Inserted,
}

pub struct TokenAuthority {
    slots: Mutex<HashMap<String, Arc<TokenSlotEntry>>>,
    max_accounts: usize,
}

impl TokenAuthority {
    pub fn new(max_accounts: usize) -> Result<Self, TokenAuthorityError> {
        if max_accounts == 0 {
            return Err(TokenAuthorityError::InvalidCapacity);
        }
        Ok(Self {
            slots: Mutex::new(HashMap::new()),
            max_accounts,
        })
    }

    /// Validates an account identifier and atomically either creates its
    /// initial slot or returns the existing slot with the caller's candidate.
    /// The standard mutex is released before any async slot lock is awaited.
    fn prepare_slot(
        &self,
        account_id: &str,
        candidate: TokenSlot,
    ) -> Result<PreparedTokenSlot, TokenAuthorityError> {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return Err(TokenAuthorityError::InvalidAccountId);
        }

        let mut slots = lock(&self.slots);
        if let Some(slot) = slots.get(account_id) {
            return Ok(PreparedTokenSlot::Existing {
                slot: slot.clone(),
                candidate,
            });
        }
        if slots.len() >= self.max_accounts {
            return Err(TokenAuthorityError::CapacityReached);
        }
        slots.insert(
            account_id.to_string(),
            Arc::new(TokenSlotEntry::new(candidate)),
        );
        Ok(PreparedTokenSlot::Inserted)
    }

    fn current_slots<'a>(
        &'a self,
        account_id: &str,
        entry: &Arc<TokenSlotEntry>,
    ) -> Result<MutexGuard<'a, HashMap<String, Arc<TokenSlotEntry>>>, TokenAuthorityError> {
        let slots = lock(&self.slots);
        if slots
            .get(account_id.trim())
            .is_none_or(|registered| !Arc::ptr_eq(registered, entry))
        {
            return Err(TokenAuthorityError::AccountNotFound);
        }
        Ok(slots)
    }

    fn ensure_current_slot(
        &self,
        account_id: &str,
        entry: &Arc<TokenSlotEntry>,
    ) -> Result<(), TokenAuthorityError> {
        self.current_slots(account_id, entry).map(|_| ())
    }
}

async fn persist_auth_state(
    account_id: &str,
    slot: &mut TokenSlot,
    persistence: Option<&dyn TokenPersistenceAdapter>,
    revision: &TokenDispatchRevision,
) -> Result<(), TokenAuthorityError> {
    let Some(persistence) = persistence else {
        return Ok(());
    };
    slot.auth_state_persistence_pending = true;
    persistence
        .persist_auth_state_fenced(account_id, slot.auth_state, revision)
        .await
        .map_err(|failure| TokenAuthorityError::PersistenceFailed(failure.code))?;
    slot.auth_state_persistence_pending = false;
    Ok(())
}

fn token_set_is_newer(current: &TokenSet, candidate: &TokenSet) -> bool {
    current.generation() > candidate.generation()
        || (current.generation() == candidate.generation()
            && current.issued_at_ms() > candidate.issued_at_ms())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

mod prepare;
mod registry;
mod set;

#[cfg(test)]
mod tests;
