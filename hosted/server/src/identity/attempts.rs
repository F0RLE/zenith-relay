use super::{LoginContext, Nonce, PkceCodeVerifier};
use crate::auth::browser::Token;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Mutex,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::{Semaphore, SemaphorePermit};

pub(super) const LIFE: Duration = Duration::from_secs(180);
const BROWSER_WINDOW: Duration = Duration::from_secs(20);
const MAX_STARTS: usize = 64;
const MAX_BROWSER_STARTS: usize = 6;
const MAX_PREPARING: usize = 4;
const MAX_EXCHANGES: usize = 4;
const RETRY_SOON: Duration = Duration::from_secs(1);
type Browser = [u8; 32];

pub(super) struct Pending {
    state: Token,
    pub nonce: Nonce,
    pub verifier: PkceCodeVerifier,
    pub until: Instant,
    pub context: LoginContext,
}

#[derive(Default)]
struct Book {
    starts: VecDeque<(Instant, Browser)>,
    preparing: HashSet<Browser>,
    pending: HashMap<Browser, Pending>,
}

pub(super) struct Attempts {
    book: Mutex<Book>,
    exchanges: Semaphore,
}

impl Default for Attempts {
    fn default() -> Self {
        Self {
            book: Mutex::default(),
            exchanges: Semaphore::new(MAX_EXCHANGES),
        }
    }
}

pub(super) struct Start<'a> {
    book: &'a Mutex<Book>,
    pub cookie: Token,
    until: Instant,
}

impl Start<'_> {
    pub fn issue(
        &self,
        now: impl FnOnce() -> Instant,
        state: Token,
        nonce: Nonce,
        verifier: PkceCodeVerifier,
        context: LoginContext,
    ) -> bool {
        let mut book = self.book.lock().unwrap();
        if now() >= self.until {
            return false;
        }
        book.pending.insert(
            *self.cookie,
            Pending {
                state,
                nonce,
                verifier,
                until: self.until,
                context,
            },
        );
        true
    }
}

impl Drop for Start<'_> {
    fn drop(&mut self) {
        self.book.lock().unwrap().preparing.remove(&*self.cookie);
    }
}

pub(super) enum ClaimError {
    Denied,
    Busy,
}

impl Attempts {
    pub fn start(
        &self,
        previous: Option<Token>,
        fresh: Token,
        now: impl FnOnce() -> Instant,
    ) -> Result<Start<'_>, Duration> {
        let mut book = self.book.lock().unwrap();
        let now = now();
        while book.starts.front().is_some_and(|(at, _)| *at + LIFE <= now) {
            book.starts.pop_front();
        }
        book.pending.retain(|_, p| p.until > now);
        let cookie = previous
            .filter(|raw| book.starts.iter().any(|(_, key)| key == &**raw))
            .unwrap_or(fresh);
        let service_ready =
            (book.starts.len() >= MAX_STARTS).then(|| book.starts.front().unwrap().0 + LIFE);
        let browser_ready = book
            .starts
            .iter()
            .rev()
            .take_while(|(at, _)| *at + BROWSER_WINDOW > now)
            .filter(|(_, key)| key == &*cookie)
            .nth(MAX_BROWSER_STARTS - 1)
            .map(|(at, _)| *at + BROWSER_WINDOW);
        if let Some(ready) = service_ready.into_iter().chain(browser_ready).max() {
            return Err(ready - now);
        }
        if book.preparing.len() >= MAX_PREPARING || !book.preparing.insert(*cookie) {
            return Err(RETRY_SOON);
        }
        book.starts.push_back((now, *cookie));
        Ok(Start {
            book: &self.book,
            cookie,
            until: now + LIFE,
        })
    }

    pub fn claim(
        &self,
        cookie: &Token,
        state: &Token,
        exchange: bool,
        now: impl FnOnce() -> Instant,
    ) -> Result<(Pending, Option<SemaphorePermit<'_>>), ClaimError> {
        let mut book = self.book.lock().unwrap();
        let now = now();
        if !book
            .pending
            .get(&**cookie)
            .is_some_and(|p| p.until > now && bool::from(p.state.as_ref().ct_eq(state.as_ref())))
        {
            return Err(ClaimError::Denied);
        }
        // A busy exchange must not consume the matching attempt.
        let permit = exchange
            .then(|| self.exchanges.try_acquire())
            .transpose()
            .map_err(|_| ClaimError::Busy)?;
        Ok((book.pending.remove(&**cookie).unwrap(), permit))
    }

    pub fn cancel(&self, cookie: &Token) {
        self.book.lock().unwrap().pending.remove(&**cookie);
    }
}

#[cfg(test)]
mod tests;
