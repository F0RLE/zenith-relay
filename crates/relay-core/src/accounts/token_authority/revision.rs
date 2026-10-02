use std::fmt;
use std::sync::{Arc, RwLock, RwLockReadGuard};

/// An in-memory incarnation, independent of persisted token generations and
/// visible token fields. Its read lock spans the final scheduler debit.
#[derive(Clone)]
pub struct TokenDispatchRevision {
    pub(in crate::accounts::token_authority) state: Arc<RwLock<DispatchRevisionState>>,
    pub(in crate::accounts::token_authority) expected: u64,
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

pub(in crate::accounts::token_authority) struct DispatchRevisionState {
    pub(in crate::accounts::token_authority) value: u64,
    pub(in crate::accounts::token_authority) active: bool,
}

#[must_use = "the guard must be held across the credential write"]
pub struct TokenDispatchRevisionGuard<'a> {
    _guard: RwLockReadGuard<'a, DispatchRevisionState>,
}

impl TokenDispatchRevision {
    /// Retains the slot's exact in-memory incarnation until the guard is
    /// dropped. Do not hold this synchronous guard across an async wait.
    pub fn guard(&self) -> Option<TokenDispatchRevisionGuard<'_>> {
        let guard = crate::poison::read(&self.state);
        (guard.active && guard.value == self.expected)
            .then_some(TokenDispatchRevisionGuard { _guard: guard })
    }
}
