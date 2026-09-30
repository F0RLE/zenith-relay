use super::*;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio::sync::{mpsc, Notify};

mod cache_lifecycle;
mod scheduling;

fn registration(revision: u64, automatic: bool) -> RefreshRegistration {
    RefreshRegistration {
        identity: RefreshIdentity::new("account:test", revision, 1),
        kind: RefreshKind::Quota,
        origin: "https://provider.example.test".into(),
        active: false,
        automatic,
        due_now: false,
    }
}
