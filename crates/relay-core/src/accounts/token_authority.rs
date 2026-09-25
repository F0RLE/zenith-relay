use super::{AccountAuthState, ReauthReason};
use crate::error::safe_error_code;
use crate::providers::chatgpt::AgentIdentityCredential;
use futures_util::future::BoxFuture;
use futures_util::lock::Mutex as AsyncMutex;
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard};

#[derive(Clone, Eq, PartialEq)]
pub struct TokenSet {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_at_ms: Option<u64>,
    issued_at_ms: u64,
    generation: u64,
}

/// Returns whether an access token remains usable after the refresh skew.
pub fn access_token_is_usable(
    expires_at_ms: Option<u64>,
    now_ms: u64,
    refresh_skew_ms: u64,
) -> bool {
    expires_at_ms.is_none_or(|expires_at| expires_at > now_ms.saturating_add(refresh_skew_ms))
}

impl TokenSet {
    pub fn new(
        access_token: impl Into<String>,
        refresh_token: Option<String>,
        id_token: Option<String>,
        expires_at_ms: Option<u64>,
        issued_at_ms: u64,
        generation: u64,
    ) -> Result<Self, &'static str> {
        let access_token = access_token.into();
        if access_token.trim().is_empty() {
            return Err("access token must not be empty");
        }
        Ok(Self {
            access_token,
            refresh_token: nonempty(refresh_token),
            id_token: nonempty(id_token),
            expires_at_ms,
            issued_at_ms,
            generation,
        })
    }

    pub fn access_only(
        access_token: impl Into<String>,
        expires_at_ms: Option<u64>,
        issued_at_ms: u64,
    ) -> Result<Self, &'static str> {
        Self::new(access_token, None, None, expires_at_ms, issued_at_ms, 0)
    }

    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    pub fn id_token(&self) -> Option<&str> {
        self.id_token.as_deref()
    }

    pub fn expires_at_ms(&self) -> Option<u64> {
        self.expires_at_ms
    }

    pub fn issued_at_ms(&self) -> u64 {
        self.issued_at_ms
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_access_usable(&self, now_ms: u64, refresh_skew_ms: u64) -> bool {
        access_token_is_usable(self.expires_at_ms, now_ms, refresh_skew_ms)
    }

    pub fn refresh_eligible(&self, now_ms: u64, refresh_skew_ms: u64) -> bool {
        self.refresh_token.is_some() && !self.is_access_usable(now_ms, refresh_skew_ms)
    }
}

impl fmt::Debug for TokenSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenSet")
            .field("access_token", &"[redacted]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "[redacted]"))
            .field("expires_at_ms", &self.expires_at_ms)
            .field("issued_at_ms", &self.issued_at_ms)
            .field("generation", &self.generation)
            .finish()
    }
}

#[derive(Clone)]
pub struct TokenRefresh {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_at_ms: Option<u64>,
}

impl TokenRefresh {
    pub fn new(
        access_token: impl Into<String>,
        refresh_token: Option<String>,
        id_token: Option<String>,
        expires_at_ms: Option<u64>,
    ) -> Result<Self, &'static str> {
        let access_token = access_token.into();
        if access_token.trim().is_empty() {
            return Err("refreshed access token must not be empty");
        }
        Ok(Self {
            access_token,
            refresh_token: nonempty(refresh_token),
            id_token: nonempty(id_token),
            expires_at_ms,
        })
    }
}

impl fmt::Debug for TokenRefresh {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenRefresh")
            .field("access_token", &"[redacted]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "[redacted]"))
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenPersistenceFailure {
    pub code: String,
}

impl TokenPersistenceFailure {
    pub fn new(code: &str) -> Self {
        Self {
            code: safe_error_code(code),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenRefreshFailureKind {
    InvalidGrant,
    ReusedRefreshToken,
    ExpiredRefreshToken,
    InvalidatedRefreshToken,
    Transient,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenRefreshFailure {
    pub kind: TokenRefreshFailureKind,
    pub code: String,
}

impl TokenRefreshFailure {
    pub fn new(kind: TokenRefreshFailureKind, code: &str) -> Self {
        Self {
            kind,
            code: safe_error_code(code),
        }
    }

    fn reauth_reason(&self) -> Option<ReauthReason> {
        match self.kind {
            TokenRefreshFailureKind::InvalidGrant => Some(ReauthReason::InvalidGrant),
            // Another concurrent refresh can rotate the token first. Preserve
            // the current state and retry normally instead of forcing login.
            TokenRefreshFailureKind::ReusedRefreshToken => None,
            TokenRefreshFailureKind::ExpiredRefreshToken => Some(ReauthReason::ExpiredRefreshToken),
            TokenRefreshFailureKind::InvalidatedRefreshToken => {
                Some(ReauthReason::InvalidatedRefreshToken)
            }
            TokenRefreshFailureKind::Transient => None,
        }
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

/// An in-memory incarnation, independent of persisted token generations and
/// visible token fields. Its read lock spans the final scheduler debit.
#[derive(Clone)]
pub struct TokenDispatchRevision {
    state: Arc<RwLock<DispatchRevisionState>>,
    expected: u64,
}

impl PartialEq for TokenDispatchRevision {
    fn eq(&self, other: &Self) -> bool {
        self.expected == other.expected && Arc::ptr_eq(&self.state, &other.state)
    }
}

impl Eq for TokenDispatchRevision {}

impl fmt::Debug for TokenDispatchRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenDispatchRevision")
            .finish_non_exhaustive()
    }
}

struct DispatchRevisionState {
    value: u64,
    active: bool,
}

#[must_use = "the guard must be held across the credential write"]
pub struct TokenDispatchRevisionGuard<'a> {
    _guard: RwLockReadGuard<'a, DispatchRevisionState>,
}

impl TokenDispatchRevision {
    /// Retains the slot's exact in-memory incarnation until the guard is
    /// dropped. Do not hold this synchronous guard across an async wait.
    pub fn guard(&self) -> Option<TokenDispatchRevisionGuard<'_>> {
        let guard = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (guard.active && guard.value == self.expected)
            .then_some(TokenDispatchRevisionGuard { _guard: guard })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenAuthorityError {
    InvalidCapacity,
    InvalidAccountId,
    CapacityReached,
    AccountNotFound,
    AccessTokenExpired,
    RequiresReauth(ReauthReason),
    RefreshFailed(String),
    PersistenceRequired,
    PersistenceFailed(String),
}

impl fmt::Display for TokenAuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCapacity => {
                formatter.write_str("token authority capacity must be positive")
            }
            Self::InvalidAccountId => formatter.write_str("account id must not be empty"),
            Self::CapacityReached => formatter.write_str("token authority capacity reached"),
            Self::AccountNotFound => formatter.write_str("account token state not found"),
            Self::AccessTokenExpired => {
                formatter.write_str("access token expired and cannot refresh")
            }
            Self::RequiresReauth(_) => formatter.write_str("account requires reauthentication"),
            Self::RefreshFailed(code) => write!(formatter, "token refresh failed: {code}"),
            Self::PersistenceRequired => {
                formatter.write_str("refreshed account tokens require persistence")
            }
            Self::PersistenceFailed(code) => {
                write!(formatter, "token persistence failed: {code}")
            }
        }
    }
}

impl std::error::Error for TokenAuthorityError {}

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

    pub async fn register(
        &self,
        account_id: &str,
        tokens: TokenSet,
        auth_state: AccountAuthState,
    ) -> Result<(), TokenAuthorityError> {
        match self.prepare_slot(account_id, TokenSlot::fresh(tokens, auth_state))? {
            PreparedTokenSlot::Inserted => Ok(()),
            PreparedTokenSlot::Existing { slot, candidate } => {
                let mut current = slot.lock().await;
                let _slots = self.current_slots(account_id, &slot)?;
                slot.bump();
                *current = candidate;
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
        let entry = { lock(&self.slots).get(account_id).cloned() };
        let Some(entry) = entry else {
            return Ok(false);
        };
        let mut slot = entry.lock().await;
        if slot.auth_state != expected_auth_state || slot.tokens != *expected_tokens {
            return Ok(false);
        }
        // A failed mutation's rollback is a new authorization incarnation,
        // even when it restores the same persisted generation and bearer.
        {
            let slots = lock(&self.slots);
            if slots
                .get(account_id)
                .is_none_or(|current| !Arc::ptr_eq(current, &entry))
            {
                return Ok(false);
            }
            entry.bump();
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
            .is_none_or(|current| !Arc::ptr_eq(current, &slot))
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
        let current = slot.lock().await;
        let _slots = self.current_slots(account_id, &slot).ok()?;
        Some(current.auth_state)
    }

    pub async fn tokens(&self, account_id: &str) -> Option<TokenSet> {
        let slot = lock(&self.slots).get(account_id).cloned()?;
        let current = slot.lock().await;
        let _slots = self.current_slots(account_id, &slot).ok()?;
        Some(current.tokens.clone())
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
        let entry = lock(&self.slots)
            .get(account_id)
            .cloned()
            .ok_or(TokenAuthorityError::AccountNotFound)?;
        let mut slot = entry.lock().await;
        if failed_generation.is_some_and(|generation| slot.tokens.generation != generation)
            || rejected_tokens.is_some_and(|tokens| slot.tokens != *tokens)
        {
            return Ok(false);
        }
        {
            let slots = lock(&self.slots);
            if slots
                .get(account_id)
                .is_none_or(|current| !Arc::ptr_eq(current, &entry))
            {
                return Ok(false);
            }
            entry.bump();
        }
        slot.tokens.expires_at_ms = Some(now_ms);
        slot.tokens.issued_at_ms = now_ms;
        slot.tokens.generation = slot.tokens.generation.saturating_add(1);
        slot.persistence_pending = true;
        let revision = entry.snapshot();
        persistence
            .persist_fenced(account_id, &slot.tokens, &revision)
            .await
            .map_err(|failure| TokenAuthorityError::PersistenceFailed(failure.code))?;
        self.ensure_current_slot(account_id, &entry)?;
        slot.persistence_pending = false;
        Ok(true)
    }

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
        let entry = lock(&self.slots)
            .get(account_id)
            .cloned()
            .ok_or(TokenAuthorityError::AccountNotFound)?;
        let mut slot = entry.lock().await;
        self.ensure_current_slot(account_id, &entry)?;
        if slot.persistence_pending {
            let persistence = persistence.ok_or(TokenAuthorityError::PersistenceRequired)?;
            let revision = entry.snapshot();
            persistence
                .persist_fenced(account_id, &slot.tokens, &revision)
                .await
                .map_err(|failure| TokenAuthorityError::PersistenceFailed(failure.code))?;
            self.ensure_current_slot(account_id, &entry)?;
            slot.persistence_pending = false;
            slot.auth_state_persistence_pending = true;
        }
        if slot.auth_state_persistence_pending {
            let persistence = persistence.ok_or(TokenAuthorityError::PersistenceRequired)?;
            let revision = entry.snapshot();
            persistence
                .persist_auth_state_fenced(account_id, slot.auth_state, &revision)
                .await
                .map_err(|failure| TokenAuthorityError::PersistenceFailed(failure.code))?;
            self.ensure_current_slot(account_id, &entry)?;
            slot.auth_state_persistence_pending = false;
        }
        if matches!(
            slot.auth_state,
            AccountAuthState::RequiresReauth(ReauthReason::ReusedRefreshToken)
        ) {
            // Older Relay versions persisted this transient OAuth race as a
            // hard reauthentication state. Heal the record before selecting a
            // token so an update can retry or use the still-valid access token.
            entry.bump();
            slot.auth_state = AccountAuthState::Active;
            persist_auth_state(account_id, &mut slot, persistence, &entry.snapshot()).await?;
            self.ensure_current_slot(account_id, &entry)?;
        }
        if let AccountAuthState::RequiresReauth(reason) = slot.auth_state {
            return Err(TokenAuthorityError::RequiresReauth(reason));
        }
        if slot.tokens.is_access_usable(now_ms, refresh_skew_ms) {
            return Ok(PreparedToken {
                status: PrepareStatus::Ready,
                tokens: slot.tokens.clone(),
                dispatch_revision: entry.snapshot(),
            });
        }
        let Some(refresh_token) = slot.tokens.refresh_token.clone() else {
            entry.bump();
            slot.auth_state = AccountAuthState::DegradedAccessOnly;
            persist_auth_state(account_id, &mut slot, persistence, &entry.snapshot()).await?;
            self.ensure_current_slot(account_id, &entry)?;
            return Err(TokenAuthorityError::AccessTokenExpired);
        };

        let previous_auth_state = slot.auth_state;
        entry.bump();
        slot.auth_state = AccountAuthState::Refreshing;
        let refresh_revision = entry.snapshot();
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
                        .is_none_or(|registered| !Arc::ptr_eq(registered, &entry))
                    {
                        return Err(TokenAuthorityError::AccountNotFound);
                    }
                    entry.bump();
                    slot.tokens = tokens.clone();
                    slot.auth_state = AccountAuthState::Active;
                }
                if let Some(persistence) = persistence {
                    slot.persistence_pending = true;
                    let revision = entry.snapshot();
                    persistence
                        .persist_fenced(account_id, &slot.tokens, &revision)
                        .await
                        .map_err(|failure| TokenAuthorityError::PersistenceFailed(failure.code))?;
                    self.ensure_current_slot(account_id, &entry)?;
                    slot.persistence_pending = false;
                    persist_auth_state(account_id, &mut slot, Some(persistence), &entry.snapshot())
                        .await?;
                    self.ensure_current_slot(account_id, &entry)?;
                }
                Ok(PreparedToken {
                    status: PrepareStatus::Refreshed,
                    tokens,
                    dispatch_revision: entry.snapshot(),
                })
            }
            Err(failure) => {
                self.ensure_current_slot(account_id, &entry)?;
                if let Some(reason) = failure.reauth_reason() {
                    slot.auth_state = AccountAuthState::RequiresReauth(reason);
                    persist_auth_state(account_id, &mut slot, persistence, &entry.snapshot())
                        .await?;
                    self.ensure_current_slot(account_id, &entry)?;
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

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct RefreshOnce {
        calls: AtomicUsize,
    }

    impl TokenRefreshAdapter for RefreshOnce {
        fn refresh<'a>(
            &'a self,
            _account_id: &'a str,
            refresh_token: &'a str,
            now_ms: u64,
        ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
            Box::pin(async move {
                assert_eq!(refresh_token, "refresh-secret");
                self.calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
                TokenRefresh::new("new-access-secret", None, None, Some(now_ms + 60_000)).map_err(
                    |_| TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid"),
                )
            })
        }
    }

    #[tokio::test]
    async fn twenty_concurrent_prepares_rotate_once() {
        let authority = Arc::new(TokenAuthority::new(4).unwrap());
        authority
            .register(
                "account",
                TokenSet::new(
                    "old-access-secret",
                    Some("refresh-secret".to_string()),
                    Some("id-secret".to_string()),
                    Some(1),
                    0,
                    7,
                )
                .unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        let adapter = Arc::new(RefreshOnce {
            calls: AtomicUsize::new(0),
        });
        let mut tasks = Vec::new();
        for _ in 0..20 {
            let authority = authority.clone();
            let adapter = adapter.clone();
            tasks.push(tokio::spawn(async move {
                authority.prepare("account", 10, 0, adapter.as_ref()).await
            }));
        }
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.unwrap().unwrap());
        }

        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result.status == PrepareStatus::Refreshed)
                .count(),
            1
        );
        let tokens = authority.tokens("account").await.unwrap();
        assert_eq!(tokens.generation(), 8);
        assert_eq!(tokens.refresh_token(), Some("refresh-secret"));
        let debug = format!("{tokens:?}");
        assert!(!debug.contains("new-access-secret"));
        assert!(!debug.contains("refresh-secret"));
        assert!(!debug.contains("id-secret"));
    }

    #[tokio::test]
    async fn prepared_token_revision_rejects_refresh_invalidation_and_readded_slot() {
        let authority = TokenAuthority::new(1).unwrap();
        let original = TokenSet::new(
            "old-access-secret",
            Some("refresh-secret".into()),
            None,
            Some(100),
            0,
            7,
        )
        .unwrap();
        authority
            .register("account", original.clone(), AccountAuthState::Active)
            .await
            .unwrap();
        let adapter = RefreshOnce {
            calls: AtomicUsize::new(0),
        };
        let before = authority.prepare("account", 10, 0, &adapter).await.unwrap();
        assert!(before.dispatch_revision.guard().is_some());
        let refreshed = authority
            .prepare("account", 101, 0, &adapter)
            .await
            .unwrap();
        assert!(before.dispatch_revision.guard().is_none());
        assert!(refreshed.dispatch_revision.guard().is_some());

        let persistence = CapturePersistence::default();
        authority
            .invalidate_access_and_persist("account", 102, &persistence)
            .await
            .unwrap();
        assert!(refreshed.dispatch_revision.guard().is_none());

        let after_invalidation = authority
            .tokens("account")
            .await
            .expect("invalidated slot remains stored");
        assert!(authority.remove("account"));
        authority
            .register("account", after_invalidation, AccountAuthState::Active)
            .await
            .unwrap();
        assert!(refreshed.dispatch_revision.guard().is_none());
        assert!(before.dispatch_revision.guard().is_none());
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn delayed_refresh_of_a_removed_slot_cannot_persist_or_authorize_the_readded_account() {
        struct PausedRefresh {
            entered: tokio::sync::Notify,
            release: tokio::sync::Notify,
        }

        impl TokenRefreshAdapter for PausedRefresh {
            fn refresh<'a>(
                &'a self,
                _account_id: &'a str,
                _refresh_token: &'a str,
                now_ms: u64,
            ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
                Box::pin(async move {
                    self.entered.notify_one();
                    self.release.notified().await;
                    Ok(TokenRefresh::new(
                        "stale-refreshed-access",
                        None,
                        None,
                        Some(now_ms + 60_000),
                    )
                    .unwrap())
                })
            }
        }

        let authority = Arc::new(TokenAuthority::new(1).unwrap());
        let adapter = Arc::new(PausedRefresh {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let persistence = Arc::new(CapturePersistence::default());
        let expired = TokenSet::new(
            "old-access",
            Some("old-refresh".into()),
            None,
            Some(1),
            0,
            1,
        )
        .unwrap();
        authority
            .register("account", expired, AccountAuthState::Active)
            .await
            .unwrap();
        let old = {
            let authority = authority.clone();
            let adapter = adapter.clone();
            let persistence = persistence.clone();
            tokio::spawn(async move {
                authority
                    .prepare_and_persist("account", 10, 0, adapter.as_ref(), persistence.as_ref())
                    .await
            })
        };
        adapter.entered.notified().await;
        assert!(authority.remove("account"));
        authority
            .register(
                "account",
                TokenSet::access_only("replacement-access", None, 10).unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        adapter.release.notify_one();
        assert!(matches!(
            old.await.unwrap(),
            Err(TokenAuthorityError::AccountNotFound)
        ));
        assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            authority.tokens("account").await.unwrap().access_token(),
            "replacement-access"
        );
    }

    #[tokio::test]
    async fn removed_slot_during_token_persistence_cannot_finish_preparation() {
        struct PausedPersistence {
            entered: tokio::sync::Notify,
            release: tokio::sync::Notify,
            capture: CapturePersistence,
        }

        impl TokenPersistenceAdapter for PausedPersistence {
            fn persist<'a>(
                &'a self,
                account_id: &'a str,
                tokens: &'a TokenSet,
            ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
                Box::pin(async move {
                    self.entered.notify_one();
                    self.release.notified().await;
                    self.capture.persist(account_id, tokens).await
                })
            }

            fn persist_auth_state<'a>(
                &'a self,
                account_id: &'a str,
                state: AccountAuthState,
            ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
                self.capture.persist_auth_state(account_id, state)
            }

            fn persist_agent_task_id<'a>(
                &'a self,
                account_id: &'a str,
                expected_task_id: Option<&'a str>,
                task_id: &'a str,
            ) -> BoxFuture<'a, Result<String, TokenPersistenceFailure>> {
                self.capture
                    .persist_agent_task_id(account_id, expected_task_id, task_id)
            }
        }

        let authority = Arc::new(TokenAuthority::new(1).unwrap());
        authority
            .register(
                "account",
                TokenSet::new(
                    "expired",
                    Some("refresh-secret".into()),
                    None,
                    Some(1),
                    0,
                    1,
                )
                .unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        let persistence = Arc::new(PausedPersistence {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            capture: CapturePersistence::default(),
        });
        let old = {
            let authority = authority.clone();
            let persistence = persistence.clone();
            tokio::spawn(async move {
                authority
                    .prepare_and_persist(
                        "account",
                        10,
                        0,
                        &RefreshOnce {
                            calls: AtomicUsize::new(0),
                        },
                        persistence.as_ref(),
                    )
                    .await
            })
        };
        persistence.entered.notified().await;
        assert!(authority.remove("account"));
        authority
            .register(
                "account",
                TokenSet::access_only("replacement", None, 10).unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        persistence.release.notify_one();
        assert!(matches!(
            old.await.unwrap(),
            Err(TokenAuthorityError::AccountNotFound)
        ));
        assert_eq!(persistence.capture.token_calls.load(Ordering::SeqCst), 1);
        assert!(persistence.capture.auth_states.lock().unwrap().is_empty());
        assert_eq!(
            authority.tokens("account").await.unwrap().access_token(),
            "replacement"
        );
    }

    #[tokio::test]
    async fn waiting_registrations_cannot_report_success_on_a_removed_slot() {
        use futures_util::poll;
        use std::task::Poll;

        let authority = TokenAuthority::new(1).unwrap();
        for case in 0..3 {
            authority
                .register(
                    "account",
                    TokenSet::new("original", None, None, None, 1, 1).unwrap(),
                    AccountAuthState::Active,
                )
                .await
                .unwrap();
            let old = lock(&authority.slots).get("account").cloned().unwrap();
            let held = old.lock().await;
            let mut waiting = Box::pin(async {
                let candidate = TokenSet::new("stale", None, None, None, 2, 2).unwrap();
                match case {
                    0 => authority
                        .register("account", candidate, AccountAuthState::Active)
                        .await
                        .map(|_| true),
                    1 => {
                        authority
                            .register_if_newer("account", candidate, AccountAuthState::Active)
                            .await
                    }
                    _ => {
                        authority
                            .register_if_not_stale("account", candidate, AccountAuthState::Active)
                            .await
                    }
                }
            });
            assert!(matches!(poll!(waiting.as_mut()), Poll::Pending));
            assert!(authority.remove("account"));
            authority
                .register(
                    "account",
                    TokenSet::new("replacement", None, None, None, 1, 1).unwrap(),
                    AccountAuthState::Active,
                )
                .await
                .unwrap();
            drop(held);
            assert_eq!(waiting.await, Err(TokenAuthorityError::AccountNotFound));
            assert_eq!(
                authority.tokens("account").await.unwrap().access_token(),
                "replacement"
            );
            authority.remove("account");
        }
    }

    #[tokio::test]
    async fn waiting_reads_cannot_return_credentials_from_a_removed_slot() {
        use futures_util::poll;
        use std::task::Poll;

        let authority = TokenAuthority::new(1).unwrap();
        authority
            .register(
                "account",
                TokenSet::access_only("old-access", None, 1).unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        let old = lock(&authority.slots).get("account").cloned().unwrap();
        let held = old.lock().await;
        let mut tokens = Box::pin(authority.tokens("account"));
        let mut auth_state = Box::pin(authority.auth_state("account"));
        assert!(matches!(poll!(tokens.as_mut()), Poll::Pending));
        assert!(matches!(poll!(auth_state.as_mut()), Poll::Pending));

        assert!(authority.remove("account"));
        authority
            .register(
                "account",
                TokenSet::access_only("new-access", None, 2).unwrap(),
                AccountAuthState::DegradedAccessOnly,
            )
            .await
            .unwrap();
        drop(held);

        assert!(tokens.await.is_none());
        assert!(auth_state.await.is_none());
        assert_eq!(
            authority.tokens("account").await.unwrap().access_token(),
            "new-access"
        );
        assert_eq!(
            authority.auth_state("account").await,
            Some(AccountAuthState::DegradedAccessOnly)
        );
    }

    #[tokio::test]
    async fn register_if_absent_never_overwrites_newer_tokens() {
        let authority = TokenAuthority::new(1).unwrap();
        let first =
            TokenSet::new("new-access", Some("new-refresh".into()), None, None, 2, 2).unwrap();
        let stale =
            TokenSet::new("old-access", Some("old-refresh".into()), None, None, 1, 1).unwrap();

        assert!(authority
            .register_if_absent("account", first, AccountAuthState::Active)
            .unwrap());
        assert!(!authority
            .register_if_absent("account", stale, AccountAuthState::Active)
            .unwrap());

        let stored = authority.tokens("account").await.unwrap();
        assert_eq!(stored.generation(), 2);
        assert_eq!(stored.access_token(), "new-access");
    }

    #[tokio::test]
    async fn conditional_registration_never_replaces_a_newer_refresh_generation() {
        let authority = TokenAuthority::new(1).unwrap();
        authority
            .register(
                "account",
                TokenSet::new("new-access", Some("new-refresh".into()), None, None, 2, 2).unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();

        let stale =
            TokenSet::new("old-access", Some("old-refresh".into()), None, None, 1, 1).unwrap();
        assert!(!authority
            .register_if_not_stale("account", stale, AccountAuthState::Active)
            .await
            .unwrap());
        assert_eq!(
            authority.tokens("account").await.unwrap().access_token(),
            "new-access"
        );

        let newest = TokenSet::new(
            "newest-access",
            Some("newest-refresh".into()),
            None,
            None,
            3,
            3,
        )
        .unwrap();
        assert!(authority
            .register_if_not_stale("account", newest, AccountAuthState::Active)
            .await
            .unwrap());
        assert_eq!(
            authority.tokens("account").await.unwrap().access_token(),
            "newest-access"
        );
    }

    #[tokio::test]
    async fn conditional_rollback_never_replaces_a_newer_token_generation() {
        let authority = TokenAuthority::new(1).unwrap();
        let attempted = TokenSet::new(
            "attempted-access",
            Some("attempted-refresh".into()),
            None,
            Some(20_000),
            20,
            2,
        )
        .unwrap();
        let newer = TokenSet::new(
            "newer-access",
            Some("newer-refresh".into()),
            None,
            Some(30_000),
            30,
            3,
        )
        .unwrap();
        let previous = TokenSet::new(
            "previous-access",
            Some("previous-refresh".into()),
            None,
            Some(10_000),
            10,
            1,
        )
        .unwrap();
        authority
            .register("account", newer, AccountAuthState::Active)
            .await
            .unwrap();

        assert!(!authority
            .replace_if_current(
                "account",
                &attempted,
                AccountAuthState::Active,
                previous,
                AccountAuthState::Active,
            )
            .await
            .unwrap());
        assert_eq!(
            authority.tokens("account").await.unwrap().access_token(),
            "newer-access"
        );
    }

    #[tokio::test]
    async fn conditional_remove_never_evicts_a_reused_account_slot() {
        let authority = TokenAuthority::new(1).unwrap();
        let attempted = TokenSet::new(
            "attempted-access",
            Some("attempted-refresh".into()),
            None,
            Some(20_000),
            20,
            2,
        )
        .unwrap();
        let newer = TokenSet::new(
            "newer-access",
            Some("newer-refresh".into()),
            None,
            Some(30_000),
            30,
            3,
        )
        .unwrap();
        authority
            .register("account", newer, AccountAuthState::Active)
            .await
            .unwrap();

        assert!(!authority
            .remove_if_current("account", &attempted, AccountAuthState::Active)
            .await
            .unwrap());
        assert_eq!(authority.len(), 1);
        assert_eq!(
            authority.tokens("account").await.unwrap().access_token(),
            "newer-access"
        );
    }

    #[tokio::test]
    async fn newer_only_registration_preserves_equal_generation_auth_state() {
        let authority = TokenAuthority::new(1).unwrap();
        let current = TokenSet::new(
            "access",
            Some("refresh".into()),
            Some("identity".into()),
            Some(10_000),
            7,
            3,
        )
        .unwrap();
        authority
            .register(
                "account",
                current.clone(),
                AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant),
            )
            .await
            .unwrap();

        assert!(!authority
            .register_if_newer("account", current, AccountAuthState::Active)
            .await
            .unwrap());
        assert_eq!(
            authority.auth_state("account").await,
            Some(AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant))
        );

        let newer = TokenSet::new(
            "new-access",
            Some("new-refresh".into()),
            Some("new-identity".into()),
            Some(20_000),
            8,
            3,
        )
        .unwrap();
        assert!(authority
            .register_if_newer("account", newer, AccountAuthState::Active)
            .await
            .unwrap());
        assert_eq!(
            authority.auth_state("account").await,
            Some(AccountAuthState::Active)
        );
    }

    struct InvalidGrant;

    impl TokenRefreshAdapter for InvalidGrant {
        fn refresh<'a>(
            &'a self,
            _account_id: &'a str,
            _refresh_token: &'a str,
            _now_ms: u64,
        ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
            Box::pin(async {
                Err(TokenRefreshFailure::new(
                    TokenRefreshFailureKind::InvalidGrant,
                    "invalid_grant",
                ))
            })
        }
    }

    #[tokio::test]
    async fn invalid_grant_marks_account_requires_reauth() {
        let authority = TokenAuthority::new(1).unwrap();
        authority
            .register(
                "account",
                TokenSet::new("access", Some("refresh".into()), None, Some(1), 0, 0).unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();

        assert!(matches!(
            authority.prepare("account", 2, 0, &InvalidGrant).await,
            Err(TokenAuthorityError::RequiresReauth(
                ReauthReason::InvalidGrant
            ))
        ));
        assert_eq!(
            authority.auth_state("account").await,
            Some(AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant))
        );
    }

    #[derive(Default)]
    struct CapturePersistence {
        token_calls: AtomicUsize,
        auth_states: std::sync::Mutex<Vec<(String, AccountAuthState)>>,
    }

    impl TokenPersistenceAdapter for CapturePersistence {
        fn persist<'a>(
            &'a self,
            _account_id: &'a str,
            _tokens: &'a TokenSet,
        ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
            Box::pin(async move {
                self.token_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }

        fn persist_auth_state<'a>(
            &'a self,
            account_id: &'a str,
            auth_state: AccountAuthState,
        ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
            Box::pin(async move {
                self.auth_states
                    .lock()
                    .unwrap()
                    .push((account_id.to_string(), auth_state));
                Ok(())
            })
        }

        fn persist_agent_task_id<'a>(
            &'a self,
            _account_id: &'a str,
            _expected_task_id: Option<&'a str>,
            _task_id: &'a str,
        ) -> BoxFuture<'a, Result<String, TokenPersistenceFailure>> {
            Box::pin(async move { Ok(_task_id.to_string()) })
        }
    }

    #[tokio::test]
    async fn terminal_auth_state_is_persisted_without_token_material() {
        let authority = TokenAuthority::new(1).unwrap();
        authority
            .register(
                "local-account",
                TokenSet::new("access", Some("refresh".into()), None, Some(1), 0, 0).unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        let persistence = CapturePersistence::default();

        assert!(matches!(
            authority
                .prepare_and_persist("local-account", 2, 0, &InvalidGrant, &persistence)
                .await,
            Err(TokenAuthorityError::RequiresReauth(
                ReauthReason::InvalidGrant
            ))
        ));
        assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            *persistence.auth_states.lock().unwrap(),
            vec![(
                "local-account".to_string(),
                AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant)
            )]
        );
    }

    struct TransientRefreshFailure;

    impl TokenRefreshAdapter for TransientRefreshFailure {
        fn refresh<'a>(
            &'a self,
            _account_id: &'a str,
            _refresh_token: &'a str,
            _now_ms: u64,
        ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
            Box::pin(async {
                Err(TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "transport",
                ))
            })
        }
    }

    struct ReusedRefreshFailure;

    impl TokenRefreshAdapter for ReusedRefreshFailure {
        fn refresh<'a>(
            &'a self,
            _account_id: &'a str,
            _refresh_token: &'a str,
            _now_ms: u64,
        ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
            Box::pin(async {
                Err(TokenRefreshFailure::new(
                    TokenRefreshFailureKind::ReusedRefreshToken,
                    "refresh_token_reused",
                ))
            })
        }
    }

    async fn active_authority_with_refreshable_expired_token() -> TokenAuthority {
        let authority = TokenAuthority::new(1).unwrap();
        authority
            .register(
                "local-account",
                TokenSet::new(
                    "access",
                    Some("refresh".into()),
                    Some("identity".into()),
                    Some(1),
                    0,
                    7,
                )
                .unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        authority
    }

    #[tokio::test]
    async fn transient_refresh_failure_preserves_auth_state_and_tokens() {
        let authority = active_authority_with_refreshable_expired_token().await;
        let persistence = CapturePersistence::default();

        assert_eq!(
            authority
                .prepare_and_persist(
                    "local-account",
                    2,
                    0,
                    &TransientRefreshFailure,
                    &persistence,
                )
                .await
                .unwrap_err(),
            TokenAuthorityError::RefreshFailed("transport".into())
        );
        assert_eq!(
            authority.auth_state("local-account").await,
            Some(AccountAuthState::Active)
        );
        assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 0);
        assert!(persistence.auth_states.lock().unwrap().is_empty());

        let tokens = authority.tokens("local-account").await.unwrap();
        assert_eq!(tokens.access_token(), "access");
        assert_eq!(tokens.refresh_token(), Some("refresh"));
        assert_eq!(tokens.id_token(), Some("identity"));
        assert_eq!(tokens.expires_at_ms(), Some(1));
        assert_eq!(tokens.issued_at_ms(), 0);
        assert_eq!(tokens.generation(), 7);
    }

    #[tokio::test]
    async fn reused_refresh_token_preserves_auth_state_and_tokens() {
        let authority = active_authority_with_refreshable_expired_token().await;
        let persistence = CapturePersistence::default();

        assert_eq!(
            authority
                .prepare_and_persist("local-account", 2, 0, &ReusedRefreshFailure, &persistence,)
                .await
                .unwrap_err(),
            TokenAuthorityError::RefreshFailed("refresh_token_reused".into())
        );
        assert_eq!(
            authority.auth_state("local-account").await,
            Some(AccountAuthState::Active)
        );
        assert!(persistence.auth_states.lock().unwrap().is_empty());
        assert_eq!(
            authority
                .tokens("local-account")
                .await
                .unwrap()
                .generation(),
            7
        );
    }

    #[tokio::test]
    async fn legacy_reused_refresh_token_reauth_state_is_healed_before_prepare() {
        let authority = TokenAuthority::new(1).unwrap();
        authority
            .register(
                "local-account",
                TokenSet::new(
                    "access",
                    Some("refresh".into()),
                    Some("identity".into()),
                    Some(10_000),
                    0,
                    7,
                )
                .unwrap(),
                AccountAuthState::RequiresReauth(ReauthReason::ReusedRefreshToken),
            )
            .await
            .unwrap();

        let prepared = authority
            .prepare("local-account", 1, 0, &TransientRefreshFailure)
            .await
            .expect("a legacy transient state must not force login");

        assert_eq!(prepared.status, PrepareStatus::Ready);
        assert_eq!(
            authority.auth_state("local-account").await,
            Some(AccountAuthState::Active)
        );
    }

    #[tokio::test]
    async fn invalidated_access_is_expired_and_persisted_once() {
        let authority = TokenAuthority::new(1).unwrap();
        authority
            .register(
                "local-account",
                TokenSet::new("access", Some("refresh".into()), None, Some(60_000), 1, 7).unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        let persistence = CapturePersistence::default();

        authority
            .invalidate_access_and_persist("local-account", 10, &persistence)
            .await
            .unwrap();

        let tokens = authority.tokens("local-account").await.unwrap();
        assert_eq!(tokens.expires_at_ms(), Some(10));
        assert_eq!(tokens.generation(), 8);
        assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stale_management_unauthorized_does_not_invalidate_a_newer_login() {
        let authority = TokenAuthority::new(1).unwrap();
        let current = TokenSet::new(
            "synthetic-new-access",
            Some("synthetic-new-refresh".into()),
            None,
            Some(60_000),
            2,
            8,
        )
        .unwrap();
        authority
            .register("local-account", current.clone(), AccountAuthState::Active)
            .await
            .unwrap();
        let persistence = CapturePersistence::default();
        assert!(!authority
            .invalidate_access_generation_and_persist("local-account", Some(7), 10, &persistence,)
            .await
            .unwrap());
        assert_eq!(
            authority.tokens("local-account").await,
            Some(current.clone())
        );
        assert_eq!(
            authority.auth_state("local-account").await,
            Some(AccountAuthState::Active)
        );
        assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 0);

        let same_generation_new_login = TokenSet::new(
            "another-login",
            Some("another-refresh".into()),
            None,
            Some(60_000),
            2,
            8,
        )
        .unwrap();
        assert!(!authority
            .invalidate_access_if_current_and_persist(
                "local-account",
                &same_generation_new_login,
                10,
                &persistence
            )
            .await
            .unwrap());
        assert_eq!(
            authority.tokens("local-account").await,
            Some(current.clone())
        );
        assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 0);

        assert!(authority
            .invalidate_access_generation_and_persist("local-account", Some(8), 20, &persistence,)
            .await
            .unwrap());
        assert!(!authority
            .invalidate_access_generation_and_persist("local-account", Some(8), 30, &persistence,)
            .await
            .unwrap());
        assert_eq!(
            authority
                .tokens("local-account")
                .await
                .unwrap()
                .generation(),
            9
        );
        assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 1);
    }

    struct MustNotRefresh;

    impl TokenRefreshAdapter for MustNotRefresh {
        fn refresh<'a>(
            &'a self,
            _account_id: &'a str,
            _refresh_token: &'a str,
            _now_ms: u64,
        ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
            Box::pin(async { panic!("access-only token must not refresh") })
        }
    }

    #[tokio::test]
    async fn expired_access_only_token_never_attempts_refresh() {
        let authority = TokenAuthority::new(1).unwrap();
        authority
            .register(
                "account",
                TokenSet::access_only("access", Some(1), 0).unwrap(),
                AccountAuthState::DegradedAccessOnly,
            )
            .await
            .unwrap();

        assert!(matches!(
            authority.prepare("account", 2, 0, &MustNotRefresh).await,
            Err(TokenAuthorityError::AccessTokenExpired)
        ));
    }
}
