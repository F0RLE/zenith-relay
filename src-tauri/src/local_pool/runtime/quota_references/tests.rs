use super::*;
use crate::local_pool::{
    accounts::{credentials::StoredCodexCredentials, records::new_account_record},
    models::AccountRouteGroup,
};
use zenith_relay_core::{
    accounts::{AccountAuthMode, AccountAuthState, ReauthReason},
    providers::chatgpt::SiwcRegistration,
    quota::{QuotaErrorState, QuotaSnapshot, QuotaWindow, QuotaWindowInput, QuotaWindowKind},
};

fn connection(id: &str, kind: OAuthClientKind) -> (LocalAccountRecord, StoredCodexCredentials) {
    let registration = (kind == OAuthClientKind::Siwc).then(|| SiwcRegistration {
        client_id: "oaiapp_synthetic_reference".into(),
        host_id: "urn:uuid:synthetic-reference".into(),
        issuer: "https://auth.openai.com".into(),
        subject: "synthetic-reference-subject".into(),
        scopes: vec!["resource.invoke".into(), "chatgpt.tokens.use.direct".into()],
        earliest_refresh_at_ms: None,
        requires_reauthentication: false,
    });
    let credentials = StoredCodexCredentials::new(
        id,
        "synthetic-access".into(),
        Some("synthetic-refresh".into()),
        None,
        Some(current_time_ms() + 60_000),
        1,
        1,
        None,
        Some(registration.as_ref().map_or_else(
            || "synthetic-provider".into(),
            SiwcRegistration::identity_key,
        )),
        registration
            .as_ref()
            .map(|registration| registration.subject.clone()),
        None,
        Some("plus".into()),
        false,
    )
    .unwrap()
    .with_oauth_client_kind(kind)
    .with_siwc_registration(registration)
    .unwrap();
    let account = new_account_record(
        &credentials,
        AccountAuthMode::OAuth,
        vec!["gpt-test".into()],
        0,
        1,
    )
    .unwrap();
    (account, credentials)
}

fn fixture() -> (
    GatewaySettings,
    Vec<LocalAccountRecord>,
    HashMap<String, Option<AccountCredentialFacts>>,
) {
    let mut settings = GatewaySettings::default();
    let mut records = Vec::new();
    let mut credentials = HashMap::new();
    for (id, kind) in [
        ("siwc", OAuthClientKind::Siwc),
        ("bps", OAuthClientKind::ExcelBps),
        ("codex", OAuthClientKind::Codex),
    ] {
        let (record, secret) = connection(id, kind);
        credentials.insert(
            record.account.id.clone(),
            Some(AccountCredentialFacts::from_stored(&secret)),
        );
        records.push(record);
    }
    settings.account_routes = vec![AccountRouteGroup {
        account_ids: records.iter().map(|r| r.account.id.clone()).collect(),
        selected_account_id: records[0].account.id.clone(),
        notes_account_id: None,
    }];
    (settings, records, credentials)
}

fn quota(remaining: f64, observed: u64) -> QuotaSnapshot {
    QuotaSnapshot {
        primary: Some(
            QuotaWindow::normalize(
                QuotaWindowInput {
                    kind: QuotaWindowKind::Primary,
                    available_percent: Some(remaining),
                    explicitly_full: None,
                    reset: None,
                    window_minutes: Some(300),
                    provider_cycle_id: None,
                    observed_at_ms: observed,
                },
                None,
            )
            .unwrap(),
        ),
        updated_at_ms: Some(observed),
        ..Default::default()
    }
}

#[test]
fn reference_uses_newest_usable_codex_or_bps_without_mutating_own_quota() {
    let (settings, mut records, credentials) = fixture();
    records[1].account.quota = quota(97.0, 90);
    records[2].account.quota = quota(80.0, 95);
    let target = records[0].account.id.clone();
    let sources = account_quota_references(&settings, &records, &credentials, 100);
    assert_eq!(sources[&target].source_account_id, records[2].account.id);
    assert_eq!(sources[&target].remaining_basis_points, 8_000);
    assert!(records[0].account.quota.primary.is_none());
    records[1].account.quota = quota(97.0, 95);
    assert_eq!(
        account_quota_references(&settings, &records, &credentials, 100)[&target].source_account_id,
        records[1].account.id
    );
    records[1].account.quota = quota(97.0, 90);
    records[2].account.quota.error = Some(QuotaErrorState::new("synthetic_failure", 100));
    assert_eq!(
        account_quota_references(&settings, &records, &credentials, 100)[&target].source_account_id,
        records[1].account.id
    );
}

#[test]
fn quota_refresh_uses_a_linked_reader_even_before_counters_are_known() {
    use crate::local_pool::accounts::quota_refresh::quota_refresh_source;
    let (mut settings, mut records, mut credentials) = fixture();
    let before = records.clone();
    assert_eq!(
        quota_refresh_source(&settings, &records, &credentials, "siwc"),
        "bps"
    );
    assert_eq!(records, before);
    records[2].account.quota = quota(80.0, 95);
    assert_eq!(
        quota_refresh_source(&settings, &records, &credentials, "siwc"),
        "codex"
    );
    assert_eq!(
        quota_refresh_source(&settings, &records, &credentials, "bps"),
        "bps"
    );
    records[2].account.auth_state = AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant);
    assert_eq!(
        quota_refresh_source(&settings, &records, &credentials, "siwc"),
        "bps"
    );
    credentials.insert("bps".into(), None);
    assert_eq!(
        quota_refresh_source(&settings, &records, &credentials, "siwc"),
        "siwc"
    );
    settings.account_routes.clear();
    records[2].account.auth_state = AccountAuthState::Active;
    assert_eq!(
        quota_refresh_source(&settings, &records, &credentials, "siwc"),
        "siwc"
    );
}

#[test]
fn reference_preserves_own_zero_limits_and_group_selection() {
    let (mut settings, mut records, credentials) = fixture();
    records[1].account.quota = quota(97.0, 90);
    records[0].account.quota = quota(0.0, 100);
    assert!(account_quota_references(&settings, &records, &credentials, 100).is_empty());
    records[0].account.quota = QuotaSnapshot {
        limit_reached: true,
        ..Default::default()
    };
    assert!(account_quota_references(&settings, &records, &credentials, 100).is_empty());
    records[0].account.quota = QuotaSnapshot::default();
    settings.account_routes[0].selected_account_id = records[1].account.id.clone();
    assert!(account_quota_references(&settings, &records, &credentials, 100).is_empty());
    settings.account_routes.clear();
    assert!(account_quota_references(&settings, &records, &credentials, 100).is_empty());
}

#[test]
fn reference_requires_fresh_complete_windows_and_usable_source() {
    let (settings, records, credentials) = fixture();
    for case in 0..9 {
        let mut records = records.clone();
        let mut credentials = credentials.clone();
        records[1].account.quota = quota(97.0, 90);
        let donor = &mut records[1];
        match case {
            0 => donor.account.quota.primary.as_mut().unwrap().observed_at_ms = 101,
            1 => donor.account.quota.primary.as_mut().unwrap().reset_at_ms = Some(100),
            2 => donor.account.subscription.active_until_ms = Some(100),
            3 => {
                donor.account.auth_state =
                    AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant)
            }
            4 => donor.account.enabled = false,
            5 => {
                credentials.insert(donor.account.id.clone(), None);
            }
            6 => {
                donor
                    .account
                    .quota
                    .primary
                    .as_mut()
                    .unwrap()
                    .available_basis_points = None
            }
            7 => donor.account.quota.primary.as_mut().unwrap().observed_at_ms = 0,
            _ => donor.account.quota.error = Some(QuotaErrorState::new("synthetic_failure", 100)),
        }
        assert!(
            account_quota_references(&settings, &records, &credentials, 100).is_empty(),
            "case {case}"
        );
    }
    let mut records = records;
    records[1].account.quota = quota(97.0, 90);
    assert!(
        account_quota_references(&settings, &records, &credentials, 90 + QUOTA_STALE_AFTER_MS)
            .is_empty()
    );
}

#[tokio::test]
async fn inactive_reference_refresh_updates_siwc_without_replacing_runtime() {
    use crate::local_pool::{
        accounts::{
            NativeSecretBackend,
            credentials::CredentialStore,
            refresh_observations::{AccountRefreshScope, RefreshReadKind, record_read_error},
        },
        error::{ErrorCode, LocalPoolError},
        runtime::{runtime_from_store, sync_refreshed_account_or_rollback},
        store::secret_store,
    };
    use std::sync::Arc;

    for kind in [OAuthClientKind::Codex, OAuthClientKind::ExcelBps] {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("zenith-relay-quota-reference-{id}"));
        let state = DesktopState::open(root.clone()).unwrap();
        let credentials = CredentialStore::from_backend(NativeSecretBackend);
        let (mut selected, selected_secret) =
            connection(&format!("siwc-{id}"), OAuthClientKind::Siwc);
        let (mut donor, donor_secret) = connection(&format!("donor-{id}"), kind);
        selected.account.in_pool = true;
        donor.account.quota = quota(97.0, current_time_ms());
        credentials.save(&selected_secret).unwrap();
        credentials.save(&donor_secret).unwrap();
        {
            let mut store = state.store().unwrap();
            store.upsert_account(selected.clone()).unwrap();
            store.upsert_account(donor.clone()).unwrap();
            let mut settings = store.gateway().clone();
            settings.account_routes = vec![AccountRouteGroup {
                account_ids: vec![selected.account.id.clone(), donor.account.id.clone()],
                selected_account_id: selected.account.id.clone(),
                notes_account_id: None,
            }];
            store.replace_gateway(settings).unwrap();
        }
        let runtime = runtime_from_store(&state).await.unwrap();
        let headroom = || {
            let candidate = runtime
                .candidate_runtime_order()
                .into_iter()
                .find(|candidate| candidate.candidate_id == selected.account.id)
                .unwrap();
            assert!(candidate.available);
            candidate
                .resource_headroom
                .unwrap()
                .quota_remaining_basis_points
        };
        assert_eq!(headroom(), Some(9_700));
        state.gateway.start(runtime.clone(), 0).await.unwrap();
        let scope = AccountRefreshScope::capture(&state, &donor.account.id)
            .await
            .unwrap();
        record_read_error(
            &state,
            &scope,
            RefreshReadKind::Quota,
            &LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "synthetic quota read failure",
            ),
        )
        .await;
        assert_eq!(headroom(), None);
        for remaining in [20.0, 0.0, 80.0] {
            let previous = donor.clone();
            donor.account.quota = quota(remaining, current_time_ms());
            state
                .store()
                .unwrap()
                .upsert_account(donor.clone())
                .unwrap();
            sync_refreshed_account_or_rollback(&state, previous, donor.clone(), false)
                .await
                .unwrap();
            assert!(Arc::ptr_eq(
                &runtime,
                &state.gateway.runtime().await.unwrap()
            ));
            assert_eq!(
                headroom(),
                (remaining > 0.0).then_some((remaining * 100.0) as u64)
            );
        }
        assert!(!runtime.remove_candidate(&donor.account.id));
        assert_eq!(headroom(), None);
        assert!(
            state
                .store()
                .unwrap()
                .account(&selected.account.id)
                .unwrap()
                .account
                .quota
                .primary
                .is_none()
        );
        let key = state
            .store()
            .unwrap()
            .keys()
            .iter()
            .find(|key| key.system)
            .unwrap()
            .secret_ref
            .clone();
        state.gateway.stop().await;
        credentials.delete(&selected.account.id).unwrap();
        credentials.delete(&donor.account.id).unwrap();
        secret_store::delete(&key).unwrap();
        drop(runtime);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
