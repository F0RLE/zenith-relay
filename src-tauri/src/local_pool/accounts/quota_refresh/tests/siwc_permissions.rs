use super::*;
use crate::local_pool::accounts::{
    credentials::StoredCodexCredentials,
    oauth::{OAuthClientKind, siwc::SiwcRegistration},
    records,
};
use std::path::PathBuf;
use zenith_relay_core::accounts::{AccountAuthMode, AccountAuthState};

mod recovery;

struct Fixture {
    state: DesktopState,
    store: CredentialStore<NativeSecretBackend>,
    saved: StoredCodexCredentials,
    root: PathBuf,
}

impl Fixture {
    fn new(scopes: &[&str], now_ms: u64) -> Self {
        Self::with_refresh_token(scopes, now_ms, Some("synthetic-refresh".into()))
    }

    fn access_only(scopes: &[&str], now_ms: u64) -> Self {
        // Recovery regressions cannot contact a provider without a refresh grant.
        Self::with_refresh_token(scopes, now_ms, None)
    }

    fn with_refresh_token(scopes: &[&str], now_ms: u64, refresh_token: Option<String>) -> Self {
        let root = crate::files::test_temp_root("siwc-prepare-permission");
        let state = DesktopState::open(root.clone()).unwrap();
        let id = format!("siwc_{}", uuid::Uuid::new_v4().simple());
        let registration = SiwcRegistration {
            client_id: "oaiapp_synthetic".into(),
            host_id: "urn:uuid:synthetic-host".into(),
            issuer: "https://auth.openai.com".into(),
            subject: "synthetic-subject".into(),
            scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
            earliest_refresh_at_ms: None,
            requires_reauthentication: false,
        };
        let saved = StoredCodexCredentials::new(
            &id,
            "synthetic-access".into(),
            refresh_token,
            None,
            Some(now_ms + 3_600_000),
            now_ms,
            2,
            None,
            Some(registration.identity_key()),
            Some(registration.subject.clone()),
            None,
            None,
            false,
        )
        .unwrap()
        .with_oauth_client_kind(OAuthClientKind::Siwc)
        .with_siwc_registration(Some(registration))
        .unwrap();
        let store = CredentialStore::from_backend(NativeSecretBackend);
        store.save(&saved).unwrap();
        let record = records::new_account_record(
            &saved,
            AccountAuthMode::OAuth,
            vec!["gpt-synthetic".into()],
            0,
            now_ms,
        )
        .unwrap();
        state.store().unwrap().upsert_account(record).unwrap();
        Self {
            state,
            store,
            saved,
            root,
        }
    }

    fn close(self) {
        self.store.delete(self.saved.local_account_id()).unwrap();
        drop(self.state);
        std::fs::remove_dir_all(self.root).unwrap();
    }
}

#[tokio::test]
async fn siwc_prepare_requires_both_plan_scopes_before_registering_valid_tokens() {
    for scopes in [
        vec!["openid"],
        vec!["openid", "resource.invoke"],
        vec!["openid", "chatgpt.tokens.use.direct"],
    ] {
        for cached in [false, true] {
            let now_ms = current_time_ms();
            let fixture = Fixture::new(&scopes, now_ms);
            let id = fixture.saved.local_account_id();
            let authority = fixture.state.token_authority();
            if cached {
                authority
                    .register(
                        id,
                        TokenSet::new(
                            "synthetic-previous-access",
                            Some("synthetic-previous-refresh".into()),
                            None,
                            Some(now_ms - 1),
                            now_ms - 60_000,
                            1,
                        )
                        .unwrap(),
                        AccountAuthState::Active,
                    )
                    .await
                    .unwrap();
            }
            let before = authority.tokens(id).await;
            let prepared = prepare_account_credentials(&fixture.state, id).await;
            let after = authority.tokens(id).await;
            let unchanged_credentials = fixture
                .store
                .require(id)
                .unwrap()
                .matches_snapshot(&fixture.saved);
            let auth_state = fixture
                .state
                .store()
                .unwrap()
                .account(id)
                .unwrap()
                .account
                .auth_state;
            fixture.close();

            let error = prepared.unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidState);
            assert!(error.message.contains("ChatGPT plan use is disabled"));
            assert_eq!(after, before);
            assert!(after.is_none_or(|tokens| !tokens.is_access_usable(now_ms, 0)));
            assert!(unchanged_credentials);
            assert_eq!(auth_state, AccountAuthState::Active);
        }
    }
}

#[tokio::test]
async fn siwc_prepare_accepts_a_valid_authorized_grant_without_refreshing() {
    let now_ms = current_time_ms();
    let fixture = Fixture::new(&["resource.invoke", "chatgpt.tokens.use.direct"], now_ms);
    let id = fixture.saved.local_account_id();
    let prepared = prepare_account_credentials(&fixture.state, id).await;
    let cached = fixture.state.token_authority().tokens(id).await;
    let unchanged = fixture
        .store
        .require(id)
        .unwrap()
        .matches_snapshot(&fixture.saved);
    fixture.close();

    let prepared = prepared.unwrap();
    assert_eq!(prepared.oauth_client_kind, OAuthClientKind::Siwc);
    assert_eq!(prepared.tokens.generation(), 2);
    assert_eq!(cached.as_ref(), Some(&prepared.tokens));
    assert!(unchanged);
}
