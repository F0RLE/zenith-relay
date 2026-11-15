//! Shared management runs with in-memory host adapters, without Tauri or a vault.
use futures_util::future::BoxFuture;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use zenith_relay_core::{
    accounts::{
        AccountAuthMode, AccountAuthState, AccountHealthState, AccountIdentity, AccountRecord,
        AccountRouteGroup, SecretBackend, SecretBackendError, TokenPersistenceAdapter,
        TokenRefreshAdapter, TokenRefreshFailure, TokenRefreshFailureKind, TokenSet, management::*,
    },
    automations::reset_resources,
    providers::chatgpt::{
        OAuthClientKind,
        connector::{authority::*, credentials::*, observations},
    },
    quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind, Subscription},
};

#[derive(Clone, Debug, PartialEq)]
struct Record {
    record: AccountRecord,
    recovery: bool,
    images: bool,
    settings: AccountRouteSettings,
    models: Vec<String>,
    discovered: Option<Vec<String>>,
}
impl ManagedAccount for Record {
    fn account(&self) -> &AccountRecord {
        &self.record
    }
    fn account_mut(&mut self) -> &mut AccountRecord {
        &mut self.record
    }
    fn requires_recovery(&self) -> bool {
        self.recovery
    }
    fn route_settings(&self) -> AccountRouteSettings {
        self.settings
    }
    fn set_route_settings(&mut self, settings: AccountRouteSettings) {
        self.settings = settings;
    }
    fn images_enabled(&self) -> bool {
        self.images
    }
    fn set_images_enabled(&mut self, enabled: bool) {
        self.images = enabled;
    }
}

fn record(id: &str) -> Record {
    Record {
        record: AccountRecord {
            id: id.into(),
            label: id.into(),
            identity: AccountIdentity {
                stable_index: id.into(),
                identity_hash: id.into(),
                organization_hash: None,
            },
            auth_mode: AccountAuthMode::OAuth,
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            source_id: "synthetic-provider".into(),
            secret_refs: Vec::new(),
            subscription: Subscription::default(),
            quota: QuotaSnapshot::default(),
            token_generation: 0,
            token_updated_at_ms: None,
            tags: BTreeSet::new(),
            enabled: true,
            in_pool: true,
            draining: false,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
        },
        recovery: false,
        models: Vec::new(),
        discovered: None,
        images: false,
        settings: AccountRouteSettings {
            priority: 4,
            weight: 3,
            purchase_cost_micro_usd: Some(17),
        },
    }
}

#[path = "host_management/import_workflow.rs"]
mod import_workflow;
fn group(ids: &[&str], selected: &str) -> AccountRouteGroup {
    AccountRouteGroup {
        account_ids: ids.iter().map(|id| (*id).into()).collect(),
        selected_account_id: selected.into(),
        notes_account_id: None,
    }
}
fn window(remaining: u16) -> QuotaWindow {
    QuotaWindow {
        kind: QuotaWindowKind::Primary,
        provider_cycle_id: None,
        window_start_ms: None,
        available_basis_points: Some(remaining),
        explicitly_full: None,
        reset_at_ms: Some(5000),
        window_minutes: Some(300),
        observed_at_ms: 10,
        full_transition_fingerprint: None,
        exhaustion_transition_fingerprint: None,
    }
}

#[test]
fn route_selection_preserves_credentials_quota_and_requires_explicit_image_consent() {
    let mut records = vec![record("a"), record("b")];
    records[0].images = true;
    records[1].record.quota.primary = Some(window(1234));
    records[1].record.token_generation = 19;
    let target_identity = records[1].record.identity.clone();
    let mut groups = vec![group(&["a", "b"], "a")];
    select_route_records(
        &mut records,
        &mut groups,
        &mut None,
        "a",
        OAuthClientKind::ExcelBps,
        group(&["a", "b"], "b"),
    )
    .unwrap();
    assert!(!records[0].record.in_pool);
    assert!(records[1].record.in_pool);
    assert!(!records[0].images && !records[1].images);
    assert_eq!(records[1].record.identity, target_identity);
    assert_eq!(records[1].record.token_generation, 19);
    assert_eq!(
        records[1]
            .record
            .quota
            .primary
            .as_ref()
            .unwrap()
            .available_basis_points,
        Some(1234)
    );
    assert_eq!(records[1].record.label, "a");
}

#[test]
fn failed_selection_and_rollback_do_not_partially_modify_records() {
    let mut records = vec![record("a"), record("b")];
    records[1].recovery = true;
    let before = records.clone();
    let mut groups = vec![group(&["a", "b"], "a")];
    let original_groups = groups.clone();
    assert!(
        select_route_records(
            &mut records,
            &mut groups,
            &mut None,
            "a",
            OAuthClientKind::Siwc,
            group(&["a", "b"], "b")
        )
        .is_err()
    );
    assert_eq!(records, before);
    assert_eq!(groups, original_groups);
    let mut previous = before.clone();
    previous[0].record.label = "old label".into();
    previous[1].record.identity.identity_hash = "replaced".into();
    assert!(restore_route_records(&mut records, &previous).is_err());
    assert_eq!(records, before);
}

#[test]
fn quota_without_optional_reset_capability_keeps_pool_membership() {
    let mut account = record("a");
    account.record.quota.primary = Some(window(9000));
    let accounts = vec![account];
    let requested = BTreeSet::from(["a".into()]);
    validate_pool_membership(
        &accounts,
        &PoolMembershipEvidence {
            requested_accounts: &requested,
            requested_sources: &BTreeSet::new(),
            known_source_ids: &[],
            verified_source_ids: &[],
            routes: &[],
            in_pool: true,
        },
    )
    .unwrap();
    assert!(reset_resources::reset_member(&[], &accounts, &accounts[0], |_| false).is_none());
    assert!(reset_resources::reset_member(&[], &accounts, &accounts[0], |_| true).is_some());
}

#[test]
fn linked_reset_has_one_owner_and_does_not_block_an_unrelated_account() {
    let mut accounts = vec![
        record("selected"),
        record("native-a"),
        record("native-b"),
        record("independent"),
    ];
    accounts[1].record.in_pool = false;
    accounts[2].record.in_pool = false;
    let groups = vec![group(&["selected", "native-a", "native-b"], "selected")];
    let capable = |id: &str| id.starts_with("native");
    assert!(reset_resources::reset_member(&groups, &accounts, &accounts[1], capable).is_some());
    assert!(reset_resources::reset_member(&groups, &accounts, &accounts[2], capable).is_none());
    let markers = BTreeMap::from([("native-a".into(), "weekly-threshold:9000:pending".into())]);
    assert!(reset_resources::has_pending_confirmation(
        &groups, &markers, &accounts, "selected"
    ));
    assert!(!reset_resources::has_pending_confirmation(
        &groups,
        &markers,
        &accounts,
        "independent"
    ));
}

#[test]
fn timer_drift_is_not_new_usage_but_changed_quota_and_new_auth_win() {
    let mut initial = record("a").record;
    initial.quota.primary = Some(window(7000));
    let mut latest = initial.clone();
    latest.quota.primary.as_mut().unwrap().reset_at_ms = Some(8000);
    assert!(!observations::quota_is_newer(&latest.quota, &initial.quota));
    latest
        .quota
        .primary
        .as_mut()
        .unwrap()
        .available_basis_points = Some(6000);
    assert!(observations::quota_is_newer(&latest.quota, &initial.quota));
    latest.health = AccountHealthState::Blocked;
    latest.last_error_code = Some("new-auth-observation".into());
    let mut incoming = initial.clone();
    observations::preserve_newer_account_state(&mut incoming, &initial, &latest);
    assert_eq!(incoming.health, latest.health);
    assert_eq!(incoming.last_error_code, latest.last_error_code);
}

#[derive(Default)]
struct Secrets(Mutex<HashMap<String, String>>);
impl SecretBackend for Secrets {
    fn save(&self, key: &str, value: &str) -> Result<(), SecretBackendError> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn load(&self, key: &str) -> Result<Option<String>, SecretBackendError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn delete(&self, key: &str) -> Result<(), SecretBackendError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}
#[derive(Default)]
struct Locks(Arc<tokio::sync::Mutex<()>>);
impl CredentialLocks for Locks {
    fn acquire<'a>(
        &'a self,
        _id: &'a str,
    ) -> BoxFuture<'a, Result<Box<dyn Send>, CredentialLockError>> {
        Box::pin(async move { Ok(Box::new(self.0.clone().lock_owned().await) as Box<dyn Send>) })
    }
}
#[derive(Default)]
struct Refresh(AtomicUsize);
impl CodexRefreshClient for Refresh {
    fn refresh<'a>(
        &'a self,
        _id: &'a str,
        _provider: Option<&'a str>,
        _token: &'a str,
        now: u64,
        _kind: OAuthClientKind,
    ) -> BoxFuture<'a, Result<CredentialRefresh, TokenRefreshFailure>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            CredentialRefresh::new(
                "synthetic-new-access".into(),
                Some("synthetic-new-refresh".into()),
                None,
                Some(now + 60_000),
            )
            .map_err(|_| TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "fixture"))
        })
    }
}
#[derive(Default)]
struct Metadata;
impl AccountMetadataSink for Metadata {
    fn persist_generation<'a>(
        &'a self,
        _id: &'a str,
        _generation: u64,
        _at: u64,
    ) -> BoxFuture<'a, Result<(), MetadataSinkError>> {
        Box::pin(async { Ok(()) })
    }
    fn persist_auth_state<'a>(
        &'a self,
        _id: &'a str,
        _state: AccountAuthState,
    ) -> BoxFuture<'a, Result<(), MetadataSinkError>> {
        Box::pin(async { Ok(()) })
    }
}
fn credentials() -> StoredCodexCredentials {
    StoredCodexCredentials::new(
        "headless",
        "synthetic-old-access".into(),
        Some("synthetic-old-refresh".into()),
        None,
        Some(1),
        1,
        1,
        None,
        Some("synthetic-provider".into()),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .unwrap()
}

#[tokio::test]
async fn host_injected_locks_and_secrets_preserve_single_refresh() {
    let store = CredentialStore::from_backend(Secrets::default());
    store.save(&credentials()).unwrap();
    let client = Arc::new(Refresh::default());
    let adapter = Arc::new(StoredRefreshAdapter::new(
        store.clone(),
        client.clone(),
        Arc::new(Locks::default()),
        0,
    ));
    let mut jobs = Vec::new();
    for _ in 0..20 {
        let adapter = adapter.clone();
        jobs.push(tokio::spawn(async move {
            adapter.refresh("headless", "unused-stale-hint", 10).await
        }));
    }
    for job in jobs {
        job.await.unwrap().unwrap();
    }
    assert_eq!(client.0.load(Ordering::SeqCst), 1);
    assert_eq!(store.require("headless").unwrap().generation(), 2);
}

#[tokio::test]
async fn host_independent_persistence_never_replaces_a_newer_generation() {
    let store = CredentialStore::from_backend(Secrets::default());
    store.save(&credentials()).unwrap();
    let persistence = CredentialPersistence::new(
        store.clone(),
        Arc::new(Metadata),
        Arc::new(Locks::default()),
    );
    let newer = TokenSet::new(
        "synthetic-new-access",
        Some("synthetic-new-refresh".into()),
        None,
        Some(9000),
        20,
        2,
    )
    .unwrap();
    persistence.persist("headless", &newer).await.unwrap();
    persistence
        .persist("headless", &credentials().to_token_set().unwrap())
        .await
        .unwrap();
    assert_eq!(store.require("headless").unwrap().generation(), 2);
    assert_eq!(
        store.require("headless").unwrap().access_token(),
        "synthetic-new-access"
    );
}
