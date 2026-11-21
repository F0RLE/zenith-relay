use super::*;
use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
};

struct Host {
    mutation: Mutex<()>,
    snapshot: Mutex<Option<PendingSnapshot>>,
    secrets: Mutex<HashMap<String, String>>,
    events: Mutex<Vec<OAuthFlowEvent>>,
    fail_write: Mutex<bool>,
    fail_delete: Mutex<bool>,
}

impl Host {
    fn new() -> Self {
        let snapshot = new_attempt(
            &CodexOAuthClient::new().unwrap(),
            "synthetic-login".into(),
            1455,
            10_000,
            None,
            None,
            None,
        )
        .unwrap();
        Self {
            mutation: Mutex::new(()),
            snapshot: Mutex::new(Some(snapshot)),
            secrets: Mutex::new(HashMap::new()),
            events: Mutex::new(Vec::new()),
            fail_write: Mutex::new(false),
            fail_delete: Mutex::new(false),
        }
    }
    fn callback(&self) -> String {
        let snapshot = self.snapshot.lock().unwrap();
        let snapshot = snapshot.as_ref().unwrap();
        let url: url::Url = snapshot.authorization_url.parse().unwrap();
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let mut callback: url::Url = snapshot.pending.redirect_uri().parse().unwrap();
        callback
            .query_pairs_mut()
            .append_pair("state", &state)
            .append_pair("code", "synthetic-code");
        callback.into()
    }
}

fn storage_error() -> OAuthFlowError {
    OAuthFlowError::new(OAuthFlowErrorCode::SnapshotIo, "synthetic storage failure")
}

impl AttemptHost for Host {
    type Guard<'a> = MutexGuard<'a, ()>;
    fn lock(&self) -> Self::Guard<'_> {
        self.mutation.lock().unwrap()
    }
    fn read(&self, _: &str) -> Result<PendingSnapshot, OAuthFlowError> {
        self.snapshot
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(storage_error)
    }
    fn write(&self, snapshot: &PendingSnapshot) -> Result<(), OAuthFlowError> {
        if *self.fail_write.lock().unwrap() {
            return Err(storage_error());
        }
        *self.snapshot.lock().unwrap() = Some(snapshot.clone());
        Ok(())
    }
    fn remove(&self, _: &str) -> Result<(), OAuthFlowError> {
        *self.snapshot.lock().unwrap() = None;
        Ok(())
    }
    fn load_secret(&self, key: &str) -> Result<Option<String>, OAuthFlowError> {
        Ok(self.secrets.lock().unwrap().get(key).cloned())
    }
    fn save_secret(&self, key: &str, value: &str) -> Result<(), OAuthFlowError> {
        self.secrets
            .lock()
            .unwrap()
            .insert(key.into(), value.into());
        Ok(())
    }
    fn delete_secret(&self, key: &str) -> Result<(), OAuthFlowError> {
        if *self.fail_delete.lock().unwrap() {
            return Err(storage_error());
        }
        self.secrets.lock().unwrap().remove(key);
        Ok(())
    }
    fn emit(&self, event: OAuthFlowEvent) {
        self.events.lock().unwrap().push(event);
    }
}

#[test]
fn callback_and_code_exchange_are_consumed_once() {
    let host = Host::new();
    let callback = host.callback();
    accept_callback(&host, "synthetic-login", &callback, 11_000).unwrap();
    assert_eq!(
        accept_callback(&host, "synthetic-login", &callback, 11_000)
            .unwrap_err()
            .code,
        OAuthFlowErrorCode::CallbackAlreadyReceived
    );
    exchange_material(&host, "synthetic-login", 11_000).unwrap();
    assert_eq!(
        exchange_material(&host, "synthetic-login", 11_000)
            .unwrap_err()
            .code,
        OAuthFlowErrorCode::RecoveryRequired
    );
    store_exchange_checkpoint(&host, "synthetic-login", "synthetic-checkpoint", 12_000).unwrap();
}

#[test]
fn cancellation_fences_late_token_reply_and_diagnostics() {
    let host = Host::new();
    accept_callback(&host, "synthetic-login", &host.callback(), 11_000).unwrap();
    exchange_material(&host, "synthetic-login", 11_000).unwrap();
    finish(&host, "synthetic-login", OAuthFlowStatus::Canceled).unwrap();
    assert!(store_exchange_checkpoint(&host, "synthetic-login", "late-result", 12_000).is_err());
    report_diagnostic(
        &host,
        "synthetic-login",
        OAuthFlowProgress::new().diagnostic(None),
        12_000,
    );
    assert!(host.secrets.lock().unwrap().is_empty());
    assert_eq!(host.events.lock().unwrap().len(), 2);
}

#[test]
fn expired_attempt_cannot_accept_callback_or_exchange() {
    let host = Host::new();
    let expiry = host
        .snapshot
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .pending
        .expires_at_ms();
    assert_eq!(
        accept_callback(&host, "synthetic-login", &host.callback(), expiry)
            .unwrap_err()
            .code,
        OAuthFlowErrorCode::Expired
    );
    accept_callback(&host, "synthetic-login", &host.callback(), 11_000).unwrap();
    assert_eq!(
        exchange_material(&host, "synthetic-login", expiry)
            .unwrap_err()
            .code,
        OAuthFlowErrorCode::Expired
    );
}

#[test]
fn failed_callback_commit_removes_uncommitted_secret() {
    let host = Host::new();
    *host.fail_write.lock().unwrap() = true;
    assert!(accept_callback(&host, "synthetic-login", &host.callback(), 11_000).is_err());
    assert!(host.secrets.lock().unwrap().is_empty());
    assert!(host.events.lock().unwrap().is_empty());
    assert_eq!(
        host.snapshot.lock().unwrap().as_ref().unwrap().status,
        OAuthFlowStatus::Pending
    );
}

#[test]
fn failed_exchange_claim_does_not_release_code() {
    let host = Host::new();
    accept_callback(&host, "synthetic-login", &host.callback(), 11_000).unwrap();
    *host.fail_write.lock().unwrap() = true;
    assert!(exchange_material(&host, "synthetic-login", 11_000).is_err());
    assert!(
        !host
            .snapshot
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .exchange_started
    );
}

#[test]
fn failed_cleanup_retains_recovery_record_and_does_not_emit_completion() {
    let host = Host::new();
    *host.fail_delete.lock().unwrap() = true;
    assert!(finish(&host, "synthetic-login", OAuthFlowStatus::Completed).is_err());
    assert!(host.snapshot.lock().unwrap().is_some());
    assert!(host.events.lock().unwrap().is_empty());
}
