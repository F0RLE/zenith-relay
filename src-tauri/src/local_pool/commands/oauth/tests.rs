use crate::local_pool::{
    accounts::{
        credentials::{
            credential_local_error as credential_error, CredentialStore, StoredCodexCredentials,
        },
        oauth::CodexOAuthClient,
        oauth_flow::{OAuthFlowStart, OAuthFlowStatus},
        records::{self, new_account_record},
        NativeSecretBackend,
    },
    error::ErrorCode,
    models::LocalAccountRecord,
};

use std::collections::BTreeSet;

use url::Url;
use uuid::Uuid;

use zenith_relay_core::{
    accounts::{AccountAuthMode, AccountAuthState, AccountHealthState},
    providers::chatgpt::{AgentIdentityCredential, ModelDiscoveryFailure},
};

use super::account::{
    apply_initial_model_issue, find_existing_account, initial_model_issue,
    preserve_existing_settings,
};
use super::checkpoint::{
    decode_completion_checkpoint, encode_completion_checkpoint, OAuthCompletionCheckpoint,
    COMPLETION_CHECKPOINT_VERSION,
};
use super::completion::{
    completion_rollback_owns_state, next_completion_generation,
    restore_attempted_completion_credentials_if_current,
};
use super::flow::validated_authorization_url;

use zenith_relay_core::providers::chatgpt::ModelDiscoveryFailureCode;

// Synthetic PKCS#8 bytes used only to exercise Agent Identity formatting.
// This is not a credential and is never registered with a provider.
const TEST_ED25519_PKCS8_FIXTURE: &str =
    "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";

#[test]
fn authorization_url_validation_allows_generated_url_only() {
    let oauth = CodexOAuthClient::new()
        .unwrap()
        .begin(1455, 10_000)
        .unwrap();
    let valid = OAuthFlowStart {
        login_id: Uuid::new_v4().hyphenated().to_string(),
        authorization_url: oauth.authorization_url().to_string(),
        redirect_uri: oauth.pending().redirect_uri().to_string(),
        expires_at_ms: oauth.pending().expires_at_ms(),
        status: OAuthFlowStatus::Pending,
        target_account_id: None,
    };
    assert!(validated_authorization_url(&valid).is_ok());

    let mut wrong_host = valid.clone();
    wrong_host.authorization_url = wrong_host
        .authorization_url
        .replace("auth.openai.com", "attacker.invalid");
    assert!(validated_authorization_url(&wrong_host).is_err());

    let mut sensitive = valid.clone();
    let mut url = Url::parse(&sensitive.authorization_url).unwrap();
    url.query_pairs_mut().append_pair("access_token", "secret");
    sensitive.authorization_url = url.to_string();
    let error = validated_authorization_url(&sensitive).unwrap_err();
    assert!(!format!("{error:?} {error}").contains("secret"));

    let mut override_request = valid.clone();
    let mut url = Url::parse(&override_request.authorization_url).unwrap();
    url.query_pairs_mut()
        .append_pair("request_uri", "https://attacker.invalid/request");
    override_request.authorization_url = url.to_string();
    assert!(validated_authorization_url(&override_request).is_err());

    let mut duplicate_redirect = valid.clone();
    let mut url = Url::parse(&duplicate_redirect.authorization_url).unwrap();
    url.query_pairs_mut()
        .append_pair("redirect_uri", &duplicate_redirect.redirect_uri);
    duplicate_redirect.authorization_url = url.to_string();
    assert!(validated_authorization_url(&duplicate_redirect).is_err());

    let mut wrong_redirect = valid;
    wrong_redirect.redirect_uri = "http://localhost:9999/auth/callback".into();
    assert!(validated_authorization_url(&wrong_redirect).is_err());
}

#[test]
fn exchanged_token_checkpoint_is_recoverable_and_fully_redacted() {
    let login_id = Uuid::new_v4().hyphenated().to_string();
    let checkpoint = OAuthCompletionCheckpoint {
        version: COMPLETION_CHECKPOINT_VERSION,
        login_id: login_id.clone(),
        access_token: "checkpoint-access-secret".into(),
        refresh_token: Some("checkpoint-refresh-secret".into()),
        id_token: Some("checkpoint-id-secret".into()),
        expires_at_ms: Some(60_000),
        issued_at_ms: 1,
        email: Some("private@example.test".into()),
        provider_account_id: "provider-private-id".into(),
        provider_user_id: Some("provider-user-private-id".into()),
        plan_type: Some("plus".into()),
        subscription_active_until_ms: Some(1_788_998_400_000),
        account_is_fedramp: false,
    };
    let encoded = encode_completion_checkpoint(&checkpoint).unwrap();
    let recovered = decode_completion_checkpoint(&encoded, &login_id)
        .unwrap()
        .unwrap();
    assert_eq!(recovered.access_token, "checkpoint-access-secret");
    assert!(decode_completion_checkpoint(
        "http://localhost:1455/auth/callback?code=callback-secret",
        &login_id
    )
    .unwrap()
    .is_none());
    let rendered = format!("{recovered:?}");
    for secret in [
        "checkpoint-access-secret",
        "checkpoint-refresh-secret",
        "checkpoint-id-secret",
        "private@example.test",
        "provider-private-id",
        "provider-user-private-id",
    ] {
        assert!(!rendered.contains(secret));
    }
}

#[test]
fn failed_initial_probe_keeps_account_with_typed_error() {
    let mut record = account("account_saved", "provider-account", "refresh-token");
    record.models.clear();
    apply_initial_model_issue(
        &mut record,
        initial_model_issue(&ModelDiscoveryFailure {
            code: ModelDiscoveryFailureCode::Unauthorized,
            retryable: false,
            retry_after_ms: None,
            http_status: Some(401),
        }),
    );

    assert_eq!(record.account.id, "account_saved");
    assert!(record.models.is_empty());
    assert_eq!(record.account.auth_state, AccountAuthState::Error);
    assert_eq!(record.account.health, AccountHealthState::Unhealthy);
    assert_eq!(
        record.account.last_error_code.as_deref(),
        Some("models_unauthorized")
    );
}

#[test]
fn initial_model_probe_uses_the_registered_agent_identity() {
    let oauth = StoredCodexCredentials::new(
        "account_models",
        "oauth-access-secret".into(),
        Some("oauth-refresh-secret".into()),
        None,
        Some(1_785_000_060_000),
        1_785_000_000_000,
        1,
        None,
        Some("provider-account".into()),
        None,
        None,
        Some("business".into()),
        false,
    )
    .unwrap();
    assert_eq!(
        oauth
            .authorization(1_785_000_000_000)
            .map_err(credential_error)
            .unwrap()
            .to_str()
            .unwrap(),
        "Bearer oauth-access-secret"
    );

    let registered = oauth.with_agent_identity(
        AgentIdentityCredential::new(
            TEST_ED25519_PKCS8_FIXTURE.into(),
            "runtime-models".into(),
            "task-models".into(),
        )
        .unwrap(),
    );
    let authorization = registered.authorization(1_785_000_000_000).unwrap();
    assert!(authorization
        .to_str()
        .unwrap()
        .starts_with("AgentAssertion "));
    assert_ne!(
        authorization.to_str().unwrap(),
        "Bearer oauth-access-secret"
    );
}

#[test]
fn duplicate_identity_preserves_local_id_and_user_settings() {
    let mut current = account("account_existing", "provider-account", "old-refresh");
    current.account.label = "My Codex".into();
    current.account.tags = BTreeSet::from(["work".into()]);
    current.account.enabled = false;
    current.account.in_pool = true;
    current.account.draining = true;
    current.account.created_at_ms = 7;
    current.account.last_used_at_ms = Some(8);
    current.allowed_models = vec!["allowed".into()];
    current.excluded_models = vec!["excluded".into()];
    current.priority = -10;
    current.weight = 4;
    current.purchase_cost_micro_usd = Some(42_000_000);
    current.cooldowns.insert("gpt-test".into(), 900);
    current.consecutive_failures = 3;

    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let identity_hash =
        records::identity_hash("provider-account", None, Some("private@example.test"));
    let existing =
        find_existing_account(std::slice::from_ref(&current), &credentials, &identity_hash)
            .unwrap()
            .unwrap();
    let mut updated_account = account("account_existing", "provider-account", "new-refresh");
    updated_account.models = vec!["new-model".into()];
    preserve_existing_settings(&mut updated_account, existing);

    assert_eq!(updated_account.account.id, "account_existing");
    assert_eq!(updated_account.account.label, "My Codex");
    assert_eq!(
        updated_account.account.identity.identity_hash,
        current.account.identity.identity_hash
    );
    assert_ne!(
        updated_account.account.identity.stable_index,
        current.account.identity.stable_index
    );
    assert_eq!(updated_account.account.tags, current.account.tags);
    assert!(!updated_account.account.enabled);
    assert!(updated_account.account.in_pool);
    assert!(updated_account.account.draining);
    assert_eq!(updated_account.account.created_at_ms, 7);
    assert_eq!(updated_account.account.last_used_at_ms, Some(8));
    assert_eq!(updated_account.allowed_models, vec!["allowed"]);
    assert_eq!(updated_account.excluded_models, vec!["excluded"]);
    assert_eq!(updated_account.priority, -10);
    assert_eq!(updated_account.weight, 4);
    assert_eq!(updated_account.purchase_cost_micro_usd, Some(42_000_000));
    assert!(updated_account.cooldowns.is_empty());
    assert_eq!(updated_account.consecutive_failures, 0);
    assert_eq!(updated_account.models, vec!["gpt-test"]);
    assert_eq!(
        updated_account.discovered_models,
        Some(vec!["new-model".into()])
    );
    assert_eq!(updated_account.effective_models(), ["new-model"]);
}

#[test]
fn duplicate_identity_keeps_models_when_the_new_snapshot_is_empty() {
    let current = account("account_empty_models", "provider-account", "old-refresh");
    let mut updated_account = account("account_empty_models", "provider-account", "new-refresh");
    updated_account.models.clear();
    updated_account.discovered_models = Some(Vec::new());

    preserve_existing_settings(&mut updated_account, &current);

    assert_eq!(updated_account.models, current.models);
    assert!(updated_account.discovered_models.is_none());
    assert_eq!(updated_account.effective_models(), ["gpt-test"]);
}

#[test]
fn reauth_does_not_restore_an_expired_subscription_date_without_new_metadata() {
    let mut current = account("account_expired", "provider-account", "old-refresh");
    current.account.subscription = zenith_relay_core::quota::Subscription::normalize(
        zenith_relay_core::quota::SubscriptionInput {
            plan_type: Some("plus".into()),
            active_until_ms: Some(1_000),
            forbidden: false,
            observed_at_ms: 2_000,
        },
    );
    let mut updated_account = account("account_expired", "provider-account", "new-refresh");
    preserve_existing_settings(&mut updated_account, &current);

    assert_eq!(updated_account.account.subscription.active_until_ms, None);
    assert_eq!(
        updated_account.account.subscription.status,
        zenith_relay_core::quota::SubscriptionStatus::Active
    );
}

#[test]
fn duplicate_identity_conflict_is_redacted() {
    let provider_account_id = "provider-private-id";
    let accounts = vec![
        account("account_one", provider_account_id, "refresh-one"),
        account("account_two", provider_account_id, "refresh-two"),
    ];
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let identity_hash =
        records::identity_hash(provider_account_id, None, Some("private@example.test"));
    let error = find_existing_account(&accounts, &credentials, &identity_hash).unwrap_err();
    assert!(matches!(error.code, ErrorCode::RecoveryRequired));
    assert!(!format!("{error:?} {error}").contains(provider_account_id));
}

#[test]
fn oauth_commit_uses_the_freshest_durable_token_generation() {
    let mut account = account("account_commit", "provider-account", "old-refresh");
    account.account.token_generation = 7;
    let credentials = StoredCodexCredentials::new(
        "account_commit",
        "newer-access-secret".into(),
        Some("newer-refresh-secret".into()),
        Some("newer-id-secret".into()),
        Some(80_000),
        80,
        9,
        Some("private@example.test".into()),
        Some("provider-account".into()),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .unwrap();

    assert_eq!(
        next_completion_generation(Some(&account), Some(&credentials)),
        10
    );
}

#[test]
fn stale_oauth_rollback_never_claims_a_newer_account_snapshot() {
    let attempted = StoredCodexCredentials::new(
        "account_rollback",
        "attempted-access-secret".into(),
        Some("attempted-refresh-secret".into()),
        Some("attempted-id-secret".into()),
        Some(20_000),
        20,
        2,
        Some("private@example.test".into()),
        Some("provider-account".into()),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .unwrap();
    let newer = StoredCodexCredentials::new(
        "account_rollback",
        "newer-access-secret".into(),
        Some("newer-refresh-secret".into()),
        Some("newer-id-secret".into()),
        Some(30_000),
        30,
        3,
        Some("private@example.test".into()),
        Some("provider-account".into()),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .unwrap();
    let attempted_record = new_account_record(
        &attempted,
        AccountAuthMode::OAuth,
        vec!["gpt-test".into()],
        0,
        20,
    )
    .unwrap();
    let newer_record = new_account_record(
        &newer,
        AccountAuthMode::OAuth,
        vec!["gpt-test".into()],
        0,
        30,
    )
    .unwrap();

    assert!(!completion_rollback_owns_state(
        Some(&newer),
        &attempted,
        newer_record.matches_rollback_snapshot(&attempted_record),
        false,
    ));
}

#[test]
fn completion_ownership_allows_a_watchdog_observation_but_not_token_change() {
    let attempted = account("account_observation", "provider-account", "refresh-token");
    let mut observed = attempted.clone();
    observed.client_auth_status = Some("login_required".into());
    observed.last_client_login_redirect_at_ms = Some(99);
    assert!(observed.matches_rollback_snapshot(&attempted));

    observed.account.token_generation = attempted.account.token_generation.saturating_add(1);
    assert!(!observed.matches_rollback_snapshot(&attempted));
}

#[test]
fn completion_credential_compensation_never_overwrites_a_newer_snapshot() {
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let account_id = format!("account_{}", Uuid::new_v4().simple());
    let previous = StoredCodexCredentials::new(
        &account_id,
        "previous-access-secret".into(),
        Some("previous-refresh-secret".into()),
        Some("previous-id-secret".into()),
        Some(10_000),
        10,
        1,
        Some("private@example.test".into()),
        Some("provider-account".into()),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .unwrap();
    let attempted = StoredCodexCredentials::new(
        &account_id,
        "attempted-access-secret".into(),
        Some("attempted-refresh-secret".into()),
        Some("attempted-id-secret".into()),
        Some(20_000),
        20,
        2,
        Some("private@example.test".into()),
        Some("provider-account".into()),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .unwrap();
    let newer = StoredCodexCredentials::new(
        &account_id,
        "newer-access-secret".into(),
        Some("newer-refresh-secret".into()),
        Some("newer-id-secret".into()),
        Some(30_000),
        30,
        3,
        Some("private@example.test".into()),
        Some("provider-account".into()),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .unwrap();

    credential_store.save(&previous).unwrap();
    assert!(restore_attempted_completion_credentials_if_current(
        &credential_store,
        &account_id,
        Some(&previous),
        &attempted,
    )
    .unwrap());
    assert!(credential_store
        .require(&account_id)
        .unwrap()
        .matches_snapshot(&attempted));

    credential_store.save(&newer).unwrap();
    assert!(!restore_attempted_completion_credentials_if_current(
        &credential_store,
        &account_id,
        Some(&previous),
        &attempted,
    )
    .unwrap());
    assert!(credential_store
        .require(&account_id)
        .unwrap()
        .matches_snapshot(&newer));

    credential_store.delete(&account_id).unwrap();
}

fn account(id: &str, provider_account_id: &str, refresh_token: &str) -> LocalAccountRecord {
    let credentials = StoredCodexCredentials::new(
        id,
        "access-secret".into(),
        Some(refresh_token.into()),
        Some("id-secret".into()),
        Some(60_000),
        1,
        1,
        Some("private@example.test".into()),
        Some(provider_account_id.into()),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .unwrap();
    new_account_record(
        &credentials,
        AccountAuthMode::OAuth,
        vec!["gpt-test".into()],
        0,
        1,
    )
    .unwrap()
}
