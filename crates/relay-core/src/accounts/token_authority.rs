use super::{AccountAuthState, ReauthReason};
use futures_util::future::BoxFuture;
use futures_util::lock::Mutex as AsyncMutex;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone)]
pub struct TokenSet {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_at_ms: Option<u64>,
    issued_at_ms: u64,
    generation: u64,
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
        self.expires_at_ms
            .is_none_or(|expires_at| expires_at > now_ms.saturating_add(refresh_skew_ms))
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
}

pub trait TokenPersistenceAdapter: Send + Sync {
    fn persist<'a>(
        &'a self,
        account_id: &'a str,
        tokens: &'a TokenSet,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>>;

    fn persist_auth_state<'a>(
        &'a self,
        account_id: &'a str,
        auth_state: AccountAuthState,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenPersistenceFailure {
    pub code: String,
}

impl TokenPersistenceFailure {
    pub fn new(code: &str) -> Self {
        Self {
            code: safe_code(code),
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
            code: safe_code(code),
        }
    }

    fn reauth_reason(&self) -> Option<ReauthReason> {
        match self.kind {
            TokenRefreshFailureKind::InvalidGrant => Some(ReauthReason::InvalidGrant),
            TokenRefreshFailureKind::ReusedRefreshToken => Some(ReauthReason::ReusedRefreshToken),
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

pub struct TokenAuthority {
    slots: Mutex<HashMap<String, Arc<AsyncMutex<TokenSlot>>>>,
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

    pub async fn register(
        &self,
        account_id: &str,
        tokens: TokenSet,
        auth_state: AccountAuthState,
    ) -> Result<(), TokenAuthorityError> {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return Err(TokenAuthorityError::InvalidAccountId);
        }
        let existing = {
            let mut slots = lock(&self.slots);
            if let Some(slot) = slots.get(account_id) {
                slot.clone()
            } else {
                if slots.len() >= self.max_accounts {
                    return Err(TokenAuthorityError::CapacityReached);
                }
                slots.insert(
                    account_id.to_string(),
                    Arc::new(AsyncMutex::new(TokenSlot {
                        tokens,
                        auth_state,
                        persistence_pending: false,
                        auth_state_persistence_pending: false,
                    })),
                );
                return Ok(());
            }
        };
        *existing.lock().await = TokenSlot {
            tokens,
            auth_state,
            persistence_pending: false,
            auth_state_persistence_pending: false,
        };
        Ok(())
    }

    pub fn remove(&self, account_id: &str) -> bool {
        lock(&self.slots).remove(account_id).is_some()
    }

    pub fn len(&self) -> usize {
        lock(&self.slots).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub async fn auth_state(&self, account_id: &str) -> Option<AccountAuthState> {
        let slot = lock(&self.slots).get(account_id).cloned()?;
        let state = slot.lock().await.auth_state;
        Some(state)
    }

    pub async fn tokens(&self, account_id: &str) -> Option<TokenSet> {
        let slot = lock(&self.slots).get(account_id).cloned()?;
        let tokens = slot.lock().await.tokens.clone();
        Some(tokens)
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
        let slot = lock(&self.slots)
            .get(account_id)
            .cloned()
            .ok_or(TokenAuthorityError::AccountNotFound)?;
        let mut slot = slot.lock().await;
        if slot.persistence_pending {
            let persistence = persistence.ok_or(TokenAuthorityError::PersistenceRequired)?;
            persistence
                .persist(account_id, &slot.tokens)
                .await
                .map_err(|failure| TokenAuthorityError::PersistenceFailed(failure.code))?;
            slot.persistence_pending = false;
            slot.auth_state_persistence_pending = true;
        }
        if slot.auth_state_persistence_pending {
            let persistence = persistence.ok_or(TokenAuthorityError::PersistenceRequired)?;
            persistence
                .persist_auth_state(account_id, slot.auth_state)
                .await
                .map_err(|failure| TokenAuthorityError::PersistenceFailed(failure.code))?;
            slot.auth_state_persistence_pending = false;
        }
        if let AccountAuthState::RequiresReauth(reason) = slot.auth_state {
            return Err(TokenAuthorityError::RequiresReauth(reason));
        }
        if slot.tokens.is_access_usable(now_ms, refresh_skew_ms) {
            return Ok(PreparedToken {
                status: PrepareStatus::Ready,
                tokens: slot.tokens.clone(),
            });
        }
        let Some(refresh_token) = slot.tokens.refresh_token.clone() else {
            slot.auth_state = AccountAuthState::DegradedAccessOnly;
            persist_auth_state(account_id, &mut slot, persistence).await?;
            return Err(TokenAuthorityError::AccessTokenExpired);
        };

        slot.auth_state = AccountAuthState::Refreshing;
        match adapter.refresh(account_id, &refresh_token, now_ms).await {
            Ok(refreshed) => {
                let tokens = TokenSet {
                    access_token: refreshed.access_token,
                    refresh_token: refreshed.refresh_token.or(Some(refresh_token)),
                    id_token: refreshed.id_token.or_else(|| slot.tokens.id_token.clone()),
                    expires_at_ms: refreshed.expires_at_ms,
                    issued_at_ms: now_ms,
                    generation: slot.tokens.generation.saturating_add(1),
                };
                slot.tokens = tokens.clone();
                slot.auth_state = AccountAuthState::Active;
                if let Some(persistence) = persistence {
                    slot.persistence_pending = true;
                    persistence
                        .persist(account_id, &slot.tokens)
                        .await
                        .map_err(|failure| TokenAuthorityError::PersistenceFailed(failure.code))?;
                    slot.persistence_pending = false;
                    persist_auth_state(account_id, &mut slot, Some(persistence)).await?;
                }
                Ok(PreparedToken {
                    status: PrepareStatus::Refreshed,
                    tokens,
                })
            }
            Err(failure) => {
                if let Some(reason) = failure.reauth_reason() {
                    slot.auth_state = AccountAuthState::RequiresReauth(reason);
                    persist_auth_state(account_id, &mut slot, persistence).await?;
                    Err(TokenAuthorityError::RequiresReauth(reason))
                } else {
                    slot.auth_state = AccountAuthState::Error;
                    persist_auth_state(account_id, &mut slot, persistence).await?;
                    Err(TokenAuthorityError::RefreshFailed(failure.code))
                }
            }
        }
    }
}

async fn persist_auth_state(
    account_id: &str,
    slot: &mut TokenSlot,
    persistence: Option<&dyn TokenPersistenceAdapter>,
) -> Result<(), TokenAuthorityError> {
    let Some(persistence) = persistence else {
        return Ok(());
    };
    slot.auth_state_persistence_pending = true;
    persistence
        .persist_auth_state(account_id, slot.auth_state)
        .await
        .map_err(|failure| TokenAuthorityError::PersistenceFailed(failure.code))?;
    slot.auth_state_persistence_pending = false;
    Ok(())
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn safe_code(value: &str) -> String {
    let value = value.trim();
    if !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        value.to_string()
    } else {
        "redacted".to_string()
    }
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
