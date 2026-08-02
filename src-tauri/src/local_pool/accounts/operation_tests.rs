use super::{export_ops::*, import_orchestrator::*, mutations::*, quota_refresh::*};
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::import_session::{
    ImportSessionError, ImportSessionErrorCode, ImportSessionStore,
};
use crate::local_pool::accounts::models::{ModelDiscoveryFailure, ModelDiscoveryFailureCode};
use crate::local_pool::accounts::{records, NativeSecretBackend};
use crate::local_pool::commands::current_time_ms;
use crate::local_pool::error::{ErrorCode, LocalPoolError};
use crate::local_pool::models::{
    AutomationRecords, LocalAccountRecord, LocalGatewayKeyRecord, ProviderSourceRecord,
};
use crate::local_pool::profiles::codex;
use crate::local_pool::state::DesktopState;
use crate::local_pool::store::secret_store;
use axum::routing::get;
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;
use tokio::net::TcpListener;
use url::Url;
use uuid::Uuid;
use zenith_relay_core::accounts::{combine_import_documents, parse_import};
use zenith_relay_core::accounts::{
    AccountAuthMode, AccountAuthState, AccountHealthState, TokenSet,
};
use zenith_relay_core::automations::{
    AccountSelector, WakeExecutionPolicy, WakeModelPolicy, WakeTask, WakeTrigger,
};
use zenith_relay_core::protocol::RemoteAccountLocation;
use zenith_relay_core::providers::chatgpt::{CodexSubscriptionMetadata, QuotaRefreshOutcome};
use zenith_relay_core::quota::{QuotaRefreshFailure, QuotaWindow, QuotaWindowKind, Subscription};
use zenith_relay_core::{ProviderSource, WireApi};

fn account_record(account_id: &str) -> LocalAccountRecord {
    let credentials = StoredCodexCredentials::new(
        account_id,
        "access-private".into(),
        Some("refresh-private".into()),
        None,
        None,
        1,
        0,
        None,
        Some("provider-private".into()),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    records::new_account_record(
        &credentials,
        AccountAuthMode::OAuth,
        vec!["gpt-test".into()],
        0,
        1,
    )
    .unwrap()
}
fn wake_task(id: &str, account_ids: &[&str]) -> WakeTask {
    WakeTask {
        id: id.into(),
        name: id.into(),
        enabled: true,
        account_selector: AccountSelector::AccountIds(
            account_ids.iter().map(|id| (*id).to_string()).collect(),
        ),
        window_kinds: BTreeSet::from([QuotaWindowKind::Primary]),
        model_policy: WakeModelPolicy::LightestSupported,
        trigger: WakeTrigger::QuotaFull,
        fallback_schedule: None,
        execution_policy: WakeExecutionPolicy::Automatic,
        jitter_seconds: 0,
        max_attempts_per_cycle: 1,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}
#[test]
fn moved_accounts_remain_stored_but_leave_local_routing() {
    let mut moved = account_record("account-moved");
    moved.account.in_pool = true;
    let untouched = account_record("account-untouched");
    let mut accounts = vec![moved, untouched.clone()];

    let location = RemoteAccountLocation {
        server_id: "server-one".into(),
        remote_account_id: "account-remote".into(),
    };
    mark_local_accounts_moved(
        &mut accounts,
        &HashMap::from([("account-moved".to_string(), location.clone())]),
    )
    .unwrap();

    assert!(!accounts[0].account.enabled);
    assert!(!accounts[0].account.in_pool);
    assert_eq!(accounts[0].remote_location, Some(location));
    assert_eq!(accounts[1], untouched);
    assert!(mark_local_accounts_moved(
        &mut accounts,
        &HashMap::from([(
            "account-missing".to_string(),
            RemoteAccountLocation {
                server_id: "server-one".into(),
                remote_account_id: "account-missing".into(),
            },
        )]),
    )
    .is_err());
}
#[test]
fn revealable_identity_prefers_email_and_falls_back_to_provider_account() {
    let with_email = StoredCodexCredentials::new(
        "account_email",
        "access-private".into(),
        None,
        None,
        None,
        1,
        0,
        Some("private@example.test".into()),
        Some("provider-account".into()),
        Some("provider-user".into()),
        None,
        None,
        false,
    )
    .unwrap();
    assert_eq!(
        revealable_account_identity(&with_email),
        Some("private@example.test")
    );

    let without_email = StoredCodexCredentials::new(
        "account_provider",
        "access-private".into(),
        None,
        None,
        None,
        1,
        0,
        None,
        Some("provider-account".into()),
        Some("provider-user".into()),
        None,
        None,
        false,
    )
    .unwrap();
    assert_eq!(
        revealable_account_identity(&without_email),
        Some("provider-account")
    );
}
#[test]
fn export_restores_generated_identity_but_preserves_custom_labels() {
    let credentials = StoredCodexCredentials::new(
        "account_export",
        "access-private".into(),
        None,
        None,
        None,
        1,
        0,
        Some("private@example.test".into()),
        Some("provider-account".into()),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let masked = credentials.snapshot().identity.unwrap();

    assert_eq!(
        export_account_label(&masked, &credentials),
        "private@example.test"
    );
    assert_eq!(export_account_label("Work Plus", &credentials), "Work Plus");
}
#[test]
fn selected_ids_are_validated_and_deduplicated_in_order() {
    let selected = normalize_selected_item_ids(vec![
        "import_0123456789abcdef".into(),
        " import_0123456789abcdef ".into(),
        "import_fedcba9876543210".into(),
    ])
    .unwrap();
    assert_eq!(
        selected,
        [
            "import_0123456789abcdef".to_string(),
            "import_fedcba9876543210".to_string()
        ]
    );
    assert!(normalize_selected_item_ids(vec!["../secret".into()]).is_err());
}
#[test]
fn quota_refresh_preserves_a_failure_observed_while_it_was_in_flight() {
    let before_refresh = account_record("account-race");
    let mut current = before_refresh.clone();
    current.account.health = AccountHealthState::Degraded;
    current.account.last_error_code = Some("upstream_rate_limited".into());
    current.cooldowns.insert("*".into(), 500);
    current.consecutive_failures = 2;
    let mut refreshed = before_refresh.clone();

    preserve_newer_account_state(&mut refreshed, &before_refresh, &current);

    assert!(refreshed.cooldowns.is_empty());
    assert_eq!(refreshed.consecutive_failures, 0);
    assert_eq!(refreshed.account.health, AccountHealthState::Degraded);
    assert_eq!(
        refreshed.account.last_error_code.as_deref(),
        Some("upstream_rate_limited")
    );
}
#[test]
fn quota_refresh_merges_auth_and_probe_state_independently() {
    let before_refresh = account_record("account-auth-race");
    let mut current = before_refresh.clone();
    current.account.auth_state = AccountAuthState::RequiresReauth(
        zenith_relay_core::accounts::ReauthReason::ReusedRefreshToken,
    );
    let mut refreshed = before_refresh.clone();
    refreshed.account.health = AccountHealthState::Unhealthy;
    refreshed.account.last_error_code = Some("token_invalidated".into());

    preserve_newer_account_state(&mut refreshed, &before_refresh, &current);

    assert!(matches!(
        refreshed.account.auth_state,
        AccountAuthState::RequiresReauth(
            zenith_relay_core::accounts::ReauthReason::ReusedRefreshToken
        )
    ));
    assert_eq!(refreshed.account.health, AccountHealthState::Unhealthy);
    assert_eq!(
        refreshed.account.last_error_code.as_deref(),
        Some("token_invalidated")
    );
}
#[test]
fn quota_recovery_uses_http_401_instead_of_a_fixed_error_list() {
    for body in [
        br#"{"detail":{"code":"token_invalidated"}}"#.as_slice(),
        br#"{"detail":{"code":"future_auth_error"}}"#.as_slice(),
        b"".as_slice(),
    ] {
        let result = Ok(QuotaRefreshOutcome::Failed {
            failure: zenith_relay_core::quota::classify_quota_http_failure(401, body),
            subscription: Subscription::default(),
        });
        assert!(quota_refresh_was_unauthorized(&result));
    }

    let payment = Ok(QuotaRefreshOutcome::Failed {
        failure: zenith_relay_core::quota::classify_quota_http_failure(
            402,
            br#"{"detail":{"code":"future_billing_error"}}"#,
        ),
        subscription: Subscription::default(),
    });
    assert!(!quota_refresh_was_unauthorized(&payment));
}
#[test]
fn failed_quota_refresh_still_applies_fetched_subscription() {
    let mut account = account_record("account-subscription-on-failure");
    let subscription = Subscription::normalize(zenith_relay_core::quota::SubscriptionInput {
        plan_type: Some("plus".into()),
        active_until_ms: Some(1_787_544_851_000),
        forbidden: false,
        observed_at_ms: 123,
    });

    let outcome = apply_quota_outcome(
        &mut account,
        QuotaRefreshOutcome::Failed {
            failure: QuotaRefreshFailure::new("quota_transport", true),
            subscription,
        },
        124,
    );

    assert!(matches!(outcome, AccountQuotaOutcome::Failed { .. }));
    assert_eq!(
        account.account.subscription.active_until_ms,
        Some(1_787_544_851_000)
    );
}
#[tokio::test]
async fn hybrid_agent_import_preserves_oauth_for_subscription_metadata() {
    const PRIVATE_KEY: &str = "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";
    let mut parsed = parse_import(
        &serde_json::json!({
            "email": "hybrid@example.test",
            "account_id": "account-hybrid",
            "access_token": "access-hybrid",
            "refresh_token": "refresh-hybrid",
            "agent_private_key": PRIVATE_KEY,
            "agent_runtime_id": "runtime-hybrid",
            "task_id": "task-hybrid"
        })
        .to_string(),
        None,
        &[],
    )
    .unwrap();
    let material =
        build_import_credential_material(parsed.items.remove(0), 1, None, None, None, 30)
            .await
            .unwrap();

    assert!(material
        .authorization(1_700_000_000_000)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("AgentAssertion "));
    assert_eq!(
        material
            .subscription_authorization()
            .unwrap()
            .unwrap()
            .to_str()
            .unwrap(),
        "Bearer access-hybrid"
    );
    let stored = material.into_stored("account_local_hybrid", 1, 0).unwrap();
    assert!(stored.is_agent_identity());
    assert!(stored.has_oauth());
    assert_eq!(stored.refresh_token(), Some("refresh-hybrid"));
}
#[test]
fn large_import_preview_defers_quota_network_calls() {
    assert!(should_probe_import_quota(true, QUOTA_REFRESH_BATCH_SIZE));
    assert!(!should_probe_import_quota(
        true,
        QUOTA_REFRESH_BATCH_SIZE + 1
    ));
    assert!(!should_probe_import_quota(false, 1));
}
#[test]
fn model_refresh_accepts_unknown_slugs_and_preserves_last_good_list() {
    let mut account = account_record("account_models");
    apply_model_discovery(&mut account, Ok(vec!["gpt-future-codex".into()]));
    assert_eq!(account.models, ["gpt-future-codex"]);

    apply_model_discovery(
        &mut account,
        Err(ModelDiscoveryFailure {
            code: ModelDiscoveryFailureCode::Transport,
            retryable: true,
            http_status: None,
        }),
    );
    assert_eq!(account.models, ["gpt-future-codex"]);
    assert!(account.account.last_error_code.is_none());

    account.models.clear();
    apply_model_discovery(
        &mut account,
        Err(ModelDiscoveryFailure {
            code: ModelDiscoveryFailureCode::Transport,
            retryable: true,
            http_status: None,
        }),
    );
    assert_eq!(
        account.account.last_error_code.as_deref(),
        Some("models_transport")
    );
    assert_eq!(account.account.health, AccountHealthState::Degraded);

    apply_model_discovery(&mut account, Ok(vec!["gpt-recovered".into()]));
    assert_eq!(account.models, ["gpt-recovered"]);
    assert_eq!(account.account.health, AccountHealthState::Healthy);
    assert!(account.account.last_error_code.is_none());
}
#[test]
fn model_unauthorized_removes_an_account_with_cached_models_from_routing() {
    let mut account = account_record("account_models_unauthorized");
    let failure = ModelDiscoveryFailure {
        code: ModelDiscoveryFailureCode::Unauthorized,
        retryable: false,
        http_status: Some(401),
    };
    assert!(model_discovery_was_unauthorized(&Some(
        Err(failure.clone())
    )));

    apply_model_discovery(&mut account, Err(failure));

    assert!(!account.models.is_empty());
    assert_eq!(account.account.auth_state, AccountAuthState::Error);
    assert_eq!(account.account.health, AccountHealthState::Unhealthy);
    assert_eq!(
        account.account.last_error_code.as_deref(),
        Some("models_unauthorized")
    );
}
#[test]
fn selected_import_files_are_read_and_combined_only_in_rust() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-files-{}",
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let paths = (1..=3)
        .map(|index| {
            let path = root.join(format!("account-{index}.json"));
            std::fs::write(
                &path,
                serde_json::json!({
                    "account_id": format!("provider-{index}"),
                    "access_token": format!("synthetic-access-{index}")
                })
                .to_string(),
            )
            .unwrap();
            path
        })
        .collect::<Vec<_>>();

    let documents = read_import_documents(paths).unwrap();
    let combined = combine_import_documents(&documents).unwrap();
    let parsed = parse_import(&combined, None, &[]).unwrap();

    assert_eq!(parsed.items.len(), 3);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn dropped_import_accepts_txt_tokens_and_rejects_other_extensions() {
    let txt_path = std::env::temp_dir().join(format!(
        "zenith-relay-import-{}.txt",
        Uuid::new_v4().simple()
    ));
    std::fs::write(&txt_path, "at-synthetic-token").unwrap();
    assert_eq!(
        read_import_documents(vec![txt_path.clone()]).unwrap(),
        ["at-synthetic-token"]
    );
    std::fs::remove_file(txt_path).unwrap();

    let unsupported_path = std::env::temp_dir().join(format!(
        "zenith-relay-import-{}.md",
        Uuid::new_v4().simple()
    ));
    std::fs::write(&unsupported_path, "at-synthetic-token").unwrap();
    assert!(read_import_documents(vec![unsupported_path.clone()]).is_err());
    std::fs::remove_file(unsupported_path).unwrap();
}
#[test]
fn current_codex_profile_reads_only_its_auth_document() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-current-codex-import-{}",
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let content = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-current-key"}"#;
    std::fs::write(root.join("auth.json"), content).unwrap();

    let documents = current_codex_import_documents(&root, &[]).unwrap();

    assert_eq!(documents, [content]);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn current_codex_profile_rejects_an_active_local_gateway_projection() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-managed-codex-import-{}",
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("auth.json"),
        r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-local-key"}"#,
    )
    .unwrap();
    let binding = codex::ProfileBinding {
        profile_dir: root.to_string_lossy().into_owned(),
        credential_kind: codex::ProfileCredentialKind::LocalGateway,
        credential_id: "local_gateway".into(),
        bound_oauth_account_id: None,
        active: true,
    };

    let error = current_codex_import_documents(&root, &[binding]).unwrap_err();

    assert!(matches!(error.code, ErrorCode::Conflict));
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn current_chatgpt_profile_visibility_requires_refreshable_oauth_identity() {
    let oauth = parse_import(
            r#"{"auth_mode":"chatgpt","account_id":"provider-current","access_token":"access-current","refresh_token":"refresh-current"}"#,
            Some("auth.json"),
            &[],
        )
        .unwrap();
    let api_key = parse_import(
        r#"{"auth_mode":"apikey","account_id":"provider-key","OPENAI_API_KEY":"key-current"}"#,
        Some("auth.json"),
        &[],
    )
    .unwrap();

    assert!(is_usable_current_chatgpt_profile(&oauth, current_time_ms()));
    assert!(!is_usable_current_chatgpt_profile(
        &api_key,
        current_time_ms()
    ));
}
#[tokio::test]
async fn batch_confirm_persists_every_selected_account_and_credential() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-batch-import-{}",
        Uuid::new_v4().simple()
    ));
    let state = DesktopState::open(root.clone()).unwrap();
    let documents = (1..=3)
        .map(|index| {
            serde_json::json!({
                "name": format!("Imported {index}"),
                "credentials": {
                    "access_token": format!("synthetic-access-{index}"),
                    "refresh_token": format!("synthetic-refresh-{index}"),
                    "chatgpt_account_id": format!("synthetic-provider-{index}"),
                    "email": format!("member-{index}@example.test"),
                    "subscription_expires_at": format!("2026-08-0{index}T00:00:00Z")
                }
            })
            .to_string()
        })
        .collect::<Vec<_>>();
    let (content, _) = normalize_import_input(StartAccountImportInput {
        content: None,
        documents,
        source_file: None,
    })
    .unwrap();
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions.start(&content, None, &[]).unwrap();
    let selected_item_ids = session
        .preview
        .rows
        .iter()
        .map(|row| row.item_id.clone())
        .collect::<Vec<_>>();

    let response = confirm_local_account_import_inner(
        ConfirmAccountImportInput {
            session_id: session.session_id,
            selected_item_ids,
            add_to_pool: true,
            discover_models: false,
            probe_quota: false,
            models: vec!["gpt-test".into()],
        },
        &state,
        None,
    )
    .await
    .unwrap();

    assert_eq!(response.results.len(), 3);
    assert!(response
        .results
        .iter()
        .all(|result| result.status == ImportItemStatus::Succeeded));
    let accounts = state.store().unwrap().accounts().to_vec();
    assert_eq!(accounts.len(), 3);
    assert_eq!(
        accounts
            .iter()
            .map(|account| account.account.id.as_str())
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let mut provider_ids = HashSet::new();
    for account in &accounts {
        let credentials = credential_store.require(&account.account.id).unwrap();
        provider_ids.insert(credentials.provider_account_id().unwrap().to_string());
        credential_store.delete(&account.account.id).unwrap();
    }
    assert_eq!(provider_ids.len(), 3);
    assert!(accounts
        .iter()
        .all(|account| account.account.subscription.active_until_ms.is_some()));
    assert!(accounts.iter().all(|account| account.account.in_pool));
    assert!(state.next_quota_refresh_due().unwrap().is_some());

    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn access_only_reimport_preserves_existing_refresh_token() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-refresh-preserve-{}",
        Uuid::new_v4().simple()
    ));
    let state = DesktopState::open(root.clone()).unwrap();
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let first = sessions
            .start(
                r#"{"auth_mode":"chatgpt","account_id":"provider-preserve","access_token":"access-original","refresh_token":"refresh-original"}"#,
                None,
                &[],
            )
            .unwrap();
    let first_item_id = first.preview.rows[0].item_id.clone();
    let response = confirm_local_account_import_inner(
        ConfirmAccountImportInput {
            session_id: first.session_id,
            selected_item_ids: vec![first_item_id],
            add_to_pool: false,
            discover_models: false,
            probe_quota: false,
            models: vec!["gpt-test".into()],
        },
        &state,
        None,
    )
    .await
    .unwrap();
    let account_id = response.results[0]
        .account
        .as_ref()
        .unwrap()
        .account
        .id
        .clone();

    let second = sessions
        .start(
            r#"{"account_id":"provider-preserve","access_token":"access-replacement"}"#,
            None,
            &[],
        )
        .unwrap();
    let second_item_id = second.preview.rows[0].item_id.clone();
    let response = confirm_local_account_import_inner(
        ConfirmAccountImportInput {
            session_id: second.session_id,
            selected_item_ids: vec![second_item_id],
            add_to_pool: false,
            discover_models: false,
            probe_quota: false,
            models: vec!["gpt-test".into()],
        },
        &state,
        None,
    )
    .await
    .unwrap();

    assert_eq!(response.results[0].status, ImportItemStatus::Succeeded);
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let credentials = credential_store.require(&account_id).unwrap();
    assert_eq!(credentials.access_token(), "access-replacement");
    assert_eq!(credentials.refresh_token(), Some("refresh-original"));
    assert_eq!(
        state
            .store()
            .unwrap()
            .account(&account_id)
            .unwrap()
            .account
            .auth_mode,
        AccountAuthMode::OAuth
    );
    credential_store.delete(&account_id).unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn import_outside_pool_is_scheduled_for_quota_monitoring() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-retry-{}",
        Uuid::new_v4().simple()
    ));
    let state = DesktopState::open(root.clone()).unwrap();
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions
        .start(
            r#"{"account_id":"provider-retry","access_token":"access-retry"}"#,
            None,
            &[],
        )
        .unwrap();
    let item_id = session.preview.rows[0].item_id.clone();
    let session_id = session.session_id.clone();

    let response = confirm_local_account_import_inner(
        ConfirmAccountImportInput {
            session_id: session_id.clone(),
            selected_item_ids: vec![item_id],
            add_to_pool: false,
            discover_models: false,
            probe_quota: false,
            models: Vec::new(),
        },
        &state,
        None,
    )
    .await
    .unwrap();

    assert_eq!(response.results[0].status, ImportItemStatus::Succeeded);
    let account = response.results[0].account.as_ref().unwrap();
    assert!(account.models.is_empty());
    assert!(state.next_quota_refresh_due().unwrap().is_some());
    assert!(sessions.resume(&session_id, &[]).is_err());
    CredentialStore::from_backend(NativeSecretBackend)
        .delete(&account.account.id)
        .unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn cockpit_api_keys_do_not_require_oauth_quota_preview() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-cockpit-source-import-{}",
        Uuid::new_v4().simple()
    ));
    let state = DesktopState::open(root.clone()).unwrap();
    let content = r#"[
            {"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-key-one","api_base_url":"https://one.example.test/v1","api_provider_name":"One API"},
            {"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-key-two","api_base_url":"https://two.example.test/v1","api_provider_name":"Two API"}
        ]"#;
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions.start(content, None, &[]).unwrap();
    let selected_item_ids = session
        .preview
        .rows
        .iter()
        .map(|row| row.item_id.clone())
        .collect();

    let response = confirm_local_account_import_inner(
        ConfirmAccountImportInput {
            session_id: session.session_id,
            selected_item_ids,
            add_to_pool: true,
            discover_models: false,
            probe_quota: true,
            models: vec!["gpt-test".into()],
        },
        &state,
        None,
    )
    .await
    .unwrap();

    assert_eq!(response.results.len(), 2);
    assert!(response
        .results
        .iter()
        .all(|result| result.status == ImportItemStatus::Succeeded));
    let sources = state.store().unwrap().sources().to_vec();
    assert_eq!(sources.len(), 2);
    assert!(sources.iter().all(|source| source.in_pool));
    assert_eq!(
        sources
            .iter()
            .map(|source| source.name.as_str())
            .collect::<HashSet<_>>(),
        HashSet::from(["One API", "Two API"])
    );
    for source in sources {
        secret_store::delete(&source.secret_ref).unwrap();
    }

    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn api_key_auth_json_builds_a_safe_default_responses_source() {
    let mut parsed = parse_import(
        r#"{"auth_mode":"api_key","OPENAI_API_KEY":"sk-private"}"#,
        None,
        &[],
    )
    .unwrap();
    let item = parsed.items.remove(0);
    let base_url = imported_source_base_url(&item).unwrap();
    let wire_api = imported_source_wire_api(&item, None).unwrap();
    let runtime = ProviderSource {
        id: "source_test".into(),
        name: item.label.clone(),
        base_url: base_url.clone(),
        api_key: item.secrets().api_key().unwrap().to_string(),
        wire_api,
        models: vec!["gpt-test".into()],
    };
    runtime.validate().unwrap();
    let source = imported_source_record(&item, runtime, "source:source_test".into(), None, None);
    let serialized = serde_json::to_string(&ImportItemResult::source_success(
        item.item_id,
        source.clone(),
    ))
    .unwrap();

    assert_eq!(source.base_url, DEFAULT_OPENAI_SOURCE_URL);
    assert_eq!(source.wire_api, WireApi::Responses);
    assert_eq!(source.models, ["gpt-test"]);
    assert!(!serialized.contains("sk-private"));
    assert!(serialized.contains("source"));
}
#[test]
fn source_duplicate_identity_updates_the_existing_local_record() {
    let mut parsed = parse_import(
        r#"{"api_key":"sk-private","base_url":"https://api.example.test/v1/"}"#,
        None,
        &[],
    )
    .unwrap();
    let item = parsed.items.remove(0);
    let existing = ProviderSourceRecord {
        id: "source_existing".into(),
        name: "Custom name".into(),
        enabled: false,
        in_pool: true,
        draining: true,
        base_url: "https://api.example.test/v1".into(),
        secret_ref: "source:source_existing".into(),
        wire_api: WireApi::ChatCompletions,
        protocol_bindings: Vec::new(),
        models: vec!["old-model".into()],
        allowed_models: vec!["gpt-*".into()],
        excluded_models: vec!["gpt-old".into()],
        priority: 7,
        weight: 3,
        recovery_delay_seconds: 60,
        model_price_overrides: Default::default(),
        last_used_at: Some("2026-07-10T00:00:00Z".into()),
        last_test_at: None,
        last_test_status: None,
        last_error: None,
    };
    assert_eq!(
        source_identity_key(&existing.base_url, "sk-private").unwrap(),
        source_identity_key(item.base_url.as_deref().unwrap(), "sk-private").unwrap()
    );
    let wire_api = imported_source_wire_api(&item, Some(&existing)).unwrap();
    let runtime = ProviderSource {
        id: existing.id.clone(),
        name: existing.name.clone(),
        base_url: imported_source_base_url(&item).unwrap(),
        api_key: "sk-private".into(),
        wire_api,
        models: vec!["new-model".into()],
    };
    let updated = imported_source_record(
        &item,
        runtime,
        existing.secret_ref.clone(),
        Some(&existing),
        None,
    );

    assert_eq!(updated.id, existing.id);
    assert_eq!(updated.name, existing.name);
    assert_eq!(updated.wire_api, WireApi::ChatCompletions);
    assert_eq!(updated.models, ["new-model"]);
    assert_eq!(updated.allowed_models, existing.allowed_models);
    assert_eq!(updated.excluded_models, existing.excluded_models);
    assert_eq!(updated.priority, 7);
    assert_eq!(updated.weight, 3);
    assert_eq!(updated.recovery_delay_seconds, 60);
    assert!(!updated.enabled);
    assert!(updated.draining);
}
#[test]
fn refresh_only_without_explicit_account_id_updates_after_exchange_identity() {
    let parsed = parse_import(r#"{"refresh_token":"refresh-rotated"}"#, None, &[]).unwrap();
    assert!(parsed.items[0].account_id.is_none());
    let mut existing = account_record("account_existing");
    existing.account.label = "My account".into();
    existing.account.token_generation = 7;
    existing.account.enabled = false;
    existing.account.in_pool = false;
    existing.priority = 9;
    existing.remote_location = Some(RemoteAccountLocation {
        server_id: "server-one".into(),
        remote_account_id: "account-remote".into(),
    });
    existing.cooldowns.insert("gpt-test".into(), 900);
    existing.consecutive_failures = 2;
    let resolved = existing.clone();
    let credentials = ImportedCredentialMaterial {
        access_token: "access-rotated".into(),
        agent_identity: None,
        refresh_token: Some("refresh-rotated".into()),
        id_token: None,
        expires_at_ms: Some(60_000),
        email: None,
        provider_account_id: Some("provider-private".into()),
        provider_user_id: None,
        organization_id: None,
        plan_type: None,
        subscription_active_until_ms: None,
        account_is_fedramp: false,
    }
    .into_stored(&resolved.account.id, 2, 8)
    .unwrap();
    let mut updated = records::new_account_record(
        &credentials,
        AccountAuthMode::ImportedToken,
        vec!["gpt-test".into()],
        0,
        2,
    )
    .unwrap();
    merge_existing_account(&mut updated, Some(&resolved));

    assert_eq!(credentials.local_account_id(), "account_existing");
    assert_eq!(credentials.generation(), 8);
    assert_eq!(updated.account.id, "account_existing");
    assert_eq!(updated.account.label, "My account");
    assert_eq!(updated.account.token_generation, 8);
    assert!(!updated.account.enabled);
    assert!(!updated.account.in_pool);
    assert_eq!(updated.priority, 9);
    assert_eq!(updated.remote_location, existing.remote_location);
    assert!(updated.cooldowns.is_empty());
    assert_eq!(updated.consecutive_failures, 0);
    assert_ne!(
        updated.account.identity.stable_index,
        existing.account.identity.stable_index
    );
}
#[test]
fn provider_identity_hash_matches_import_parser_without_exposing_id() {
    let parsed = parse_import(
            r#"{"account_id":"Provider-Private","chatgpt_user_id":"User-Private","email":"private@example.test","access_token":"access-private"}"#,
            None,
            &[],
        )
        .unwrap();
    let key = provider_identity_key(
        "Provider-Private",
        Some("User-Private"),
        Some("private@example.test"),
    );
    assert_eq!(parsed.items[0].identity_key, key);
    assert!(!key.contains("provider"));
}
#[test]
fn account_patch_normalizes_metadata_and_rejects_zero_weight() {
    let credentials = StoredCodexCredentials::new(
        "account_local",
        "access-private".into(),
        Some("refresh-private".into()),
        None,
        None,
        1,
        0,
        None,
        Some("provider-private".into()),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let mut account = records::new_account_record(
        &credentials,
        AccountAuthMode::OAuth,
        vec!["gpt-test".into()],
        0,
        1,
    )
    .unwrap();
    apply_account_patch(
        &mut account,
        UpdateAccountInput {
            account_id: "account_local".into(),
            label: Some("  Personal  ".into()),
            priority: Some(7),
            weight: Some(2),
            allowed_models: Some(vec![" gpt-test ".into(), "gpt-test".into()]),
            excluded_models: Some(vec![" gpt-old ".into()]),
            in_pool: Some(true),
            draining: Some(true),
            purchase_cost_micro_usd: Some(12_500_000),
        },
    )
    .unwrap();
    assert_eq!(account.account.label, "Personal");
    assert_eq!(account.priority, 7);
    assert_eq!(account.weight, 2);
    assert_eq!(account.allowed_models, ["gpt-test"]);
    assert_eq!(account.excluded_models, ["gpt-old"]);
    assert_eq!(account.economics.purchase_cost_micro_usd, Some(12_500_000));
    assert!(account.account.draining);
    assert!(apply_account_patch(
        &mut account,
        UpdateAccountInput {
            account_id: "account_local".into(),
            label: None,
            priority: None,
            weight: Some(0),
            allowed_models: None,
            excluded_models: None,
            in_pool: None,
            draining: None,
            purchase_cost_micro_usd: None,
        },
    )
    .is_err());
}
#[test]
fn failed_account_without_models_remains_manageable() {
    let mut account = account_record("account_failed");
    account.models.clear();
    account.account.health = zenith_relay_core::accounts::AccountHealthState::Unhealthy;
    account.account.last_error_code = Some("models_unauthorized".into());
    assert!(account_model_state_is_valid(&account));

    account.account.health = zenith_relay_core::accounts::AccountHealthState::Healthy;
    assert!(!account_model_state_is_valid(&account));
}
#[test]
fn deleting_account_preserves_explicit_empty_key_scope() {
    let mut keys = [LocalGatewayKeyRecord {
        id: "key_1".into(),
        label: "Scoped".into(),
        enabled: true,
        system: false,
        secret_ref: "key:key_1".into(),
        source_ids: None,
        account_ids: Some(vec!["account_1".into()]),
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        model_prefix: None,
        wire_apis: None,
        created_at: "2026-07-10T00:00:00Z".into(),
        last_used_at: None,
    }];
    prune_key_account_scopes(&mut keys, &[]);
    assert_eq!(keys[0].account_ids, Some(Vec::new()));
}
#[test]
fn deleting_account_prunes_explicit_selectors_without_rewriting_wake_state() {
    let mut automations = AutomationRecords::default();
    let original_state = automations.state.clone();
    automations.tasks = vec![
        wake_task("only-deleted", &["account_1"]),
        wake_task("shared", &["account_1", "account_2"]),
    ];

    let pruned = prune_account_task_selectors(automations, "account_1");

    assert_eq!(pruned.tasks.len(), 1);
    assert_eq!(pruned.tasks[0].id, "shared");
    assert_eq!(
        pruned.tasks[0].account_selector,
        AccountSelector::AccountIds(BTreeSet::from(["account_2".to_string()]))
    );
    assert_eq!(pruned.state, original_state);
}
#[test]
fn failed_delete_restores_credentials_quota_and_profile_binding() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-delete-rollback-{}",
        Uuid::new_v4().simple()
    ));
    let profile = root.join("profile");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("config.toml"), "model_provider = \"custom\"\n").unwrap();
    let state = DesktopState::open(root.clone()).unwrap();
    let account_id = format!("account_{}", Uuid::new_v4().simple());
    let stored = StoredCodexCredentials::new(
        &account_id,
        "access-private".into(),
        Some("refresh-private".into()),
        None,
        Some(60_000),
        1,
        1,
        None,
        Some("provider-private".into()),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    credentials.save(&stored).unwrap();
    state
        .store()
        .unwrap()
        .upsert_account(account_record(&account_id))
        .unwrap();
    state.mark_quota_refresh(&account_id, 123_456).unwrap();
    codex::attach_account(
        &profile,
        &state.profile_backup_root(),
        &account_id,
        &stored.to_token_set().unwrap(),
        "provider-private",
    )
    .unwrap();

    let previous_quota = state.quota_refresh_snapshot().unwrap();
    let previous_wake = state.wake_snapshot().unwrap();
    let old_automations = state.store().unwrap().automations().clone();
    let bindings = codex::account_bindings(&state.profile_backup_root()).unwrap();
    let restored = restore_bound_account_profiles(&state, &bindings, Some(&stored)).unwrap();
    credentials.delete(&account_id).unwrap();
    state.remove_quota_refresh(&account_id).unwrap();

    rollback_deleted_account_side_effects(
        &state,
        &credentials,
        &account_id,
        Some(&stored),
        previous_quota,
        previous_wake,
        old_automations,
        &restored,
        None,
        &LocalPoolError::new(ErrorCode::Io, "injected delete failure"),
    )
    .unwrap();

    assert!(credentials.require(&account_id).is_ok());
    assert_eq!(state.next_quota_refresh_due().unwrap(), Some(123_456));
    assert_eq!(
        codex::account_bindings(&state.profile_backup_root())
            .unwrap()
            .len(),
        1
    );
    assert!(std::fs::read_to_string(profile.join("auth.json"))
        .unwrap()
        .contains("access-private"));

    codex::restore_account_profile(&profile, &state.profile_backup_root()).unwrap();
    credentials.delete(&account_id).unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn quota_refresh_schedule_uses_reset_lead_and_failure_backoff() {
    let now_ms = 100_000;
    let mut account = account_record("account_local");
    account.account.quota.primary = Some(QuotaWindow {
        kind: QuotaWindowKind::Primary,
        available_basis_points: Some(10_000),
        explicitly_full: Some(true),
        reset_at_ms: Some(now_ms + 300_000),
        window_minutes: Some(300),
        observed_at_ms: now_ms,
        full_transition_fingerprint: None,
    });
    let updated = AccountQuotaRefreshResponse {
        account: account.clone(),
        quota: AccountQuotaOutcome::Updated {
            transitions: Vec::new(),
        },
    };
    assert_eq!(
        next_quota_refresh_at(&updated, now_ms),
        Some(now_ms + 300_000 + quota_reset_refresh_delay(&account.account.id))
    );
    account.account.quota.primary.as_mut().unwrap().reset_at_ms = Some(now_ms + 10_000);
    let short_reset = AccountQuotaRefreshResponse {
        account: account.clone(),
        quota: AccountQuotaOutcome::Updated {
            transitions: Vec::new(),
        },
    };
    assert_eq!(
        next_quota_refresh_at(&short_reset, now_ms),
        Some(now_ms + 10_000 + quota_reset_refresh_delay(&account.account.id))
    );
    account.account.quota.primary.as_mut().unwrap().reset_at_ms = Some(now_ms + 5 * 60 * 60_000);
    let long_window = AccountQuotaRefreshResponse {
        account: account.clone(),
        quota: AccountQuotaOutcome::Updated {
            transitions: Vec::new(),
        },
    };
    assert_eq!(
        next_quota_refresh_at(&long_window, now_ms),
        Some(now_ms + QUOTA_IDLE_REFRESH_MS)
    );

    let retryable = AccountQuotaRefreshResponse {
        account: account.clone(),
        quota: AccountQuotaOutcome::Failed {
            code: "quota_transport".into(),
            retryable: true,
        },
    };
    assert_eq!(
        next_quota_refresh_at(&retryable, now_ms),
        Some(now_ms + QUOTA_REFRESH_RETRY_MS)
    );
    let parser_failure = AccountQuotaRefreshResponse {
        account: account.clone(),
        quota: AccountQuotaOutcome::Failed {
            code: "quota_invalid_response".into(),
            retryable: false,
        },
    };
    assert_eq!(
        next_quota_refresh_at(&parser_failure, now_ms),
        Some(now_ms + QUOTA_IDLE_REFRESH_MS)
    );
    account.account.auth_state =
        AccountAuthState::RequiresReauth(zenith_relay_core::accounts::ReauthReason::InvalidGrant);
    let terminal = AccountQuotaRefreshResponse {
        account,
        quota: AccountQuotaOutcome::Failed {
            code: "invalid_grant".into(),
            retryable: false,
        },
    };
    assert_eq!(next_quota_refresh_at(&terminal, now_ms), None);
    assert_eq!(QUOTA_REFRESH_BATCH_SIZE, 5);
}
#[test]
fn prepared_credentials_debug_output_is_redacted() {
    let prepared = PreparedAccountCredentials {
        tokens: TokenSet::new(
            "access-private",
            Some("refresh-private".into()),
            None,
            Some(60_000),
            1,
            0,
        )
        .unwrap(),
        provider_account_id: "provider-private".into(),
        proxy: None,
    };
    assert_eq!(prepared.tokens().access_token(), "access-private");
    let debug = format!("{prepared:?}");
    assert!(!debug.contains("access-private"));
    assert!(!debug.contains("refresh-private"));
    assert!(!debug.contains("provider-private"));
}
#[test]
fn session_and_item_responses_never_serialize_secret_material() {
    let parsed = parse_import(
            r#"{"account_id":"provider-private","access_token":"access-private","refresh_token":"refresh-private"}"#,
            None,
            &[],
        )
        .unwrap();
    let response = ImportSessionResponse {
        session_id: "session-safe".into(),
        created_at_ms: 1,
        prepared: false,
        preview: parsed.preview,
    };
    let serialized = serde_json::to_string(&response).unwrap();
    assert!(!serialized.contains("access-private"));
    assert!(!serialized.contains("refresh-private"));
    assert!(!serialized.contains("provider-private"));
    assert!(!serialized.contains("items"));

    let failed = ImportItemResult::failure(
        "import_0123456789abcdef".into(),
        ImportItemError::new("use_source_import", "use the source import flow"),
    );
    let serialized = serde_json::to_string(&failed).unwrap();
    assert!(!serialized.contains("access-private"));
}
#[test]
fn imported_jwt_claims_supply_account_identity_without_serializing_token() {
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "email": "private@example.test",
            "exp": 123,
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "pro",
                "chatgpt_subscription_active_until": 1_767_225_600,
                "chatgpt_user_id": "user-private",
                "account_id": "account-private"
            }
        })
        .to_string(),
    );
    let token = format!("header.{payload}.signature");
    let identity = imported_identity(Some(&token), Some(&token));
    assert_eq!(
        identity.provider_account_id.as_deref(),
        Some("account-private")
    );
    assert_eq!(identity.provider_user_id.as_deref(), Some("user-private"));
    assert_eq!(identity.plan_type.as_deref(), Some("pro"));
    assert_eq!(
        identity.subscription_active_until_ms,
        Some(1_767_225_600_000)
    );
    assert_eq!(identity.access_expires_at_ms, Some(123_000));
    assert!(!hex::encode(Sha256::digest(token.as_bytes())).contains("private"));
}
#[test]
fn subscription_metadata_adds_expiry_and_normalizes_the_plan_alias() {
    let mut subscription = Subscription {
        plan_type: Some("plus".into()),
        ..Default::default()
    };
    apply_subscription_metadata(
        &mut subscription,
        CodexSubscriptionMetadata {
            account_id: None,
            plan_type: Some("chatgptplusplan".into()),
            active_until_ms: Some(1_787_544_851_000),
        },
        123,
    );

    assert_eq!(subscription.plan_type.as_deref(), Some("plus"));
    assert_eq!(subscription.active_until_ms, Some(1_787_544_851_000));
    assert_eq!(subscription.updated_at_ms, Some(123));
}
#[test]
fn imported_identity_prefers_the_access_token_workspace() {
    let token = |account_id: &str| {
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "https://api.openai.com/auth": { "chatgpt_account_id": account_id }
            })
            .to_string(),
        );
        format!("header.{payload}.signature")
    };
    let identity = imported_identity(
        Some(&token("workspace-old")),
        Some(&token("workspace-live")),
    );
    assert_eq!(
        identity.provider_account_id.as_deref(),
        Some("workspace-live")
    );
}
#[tokio::test]
async fn account_check_recovers_an_id_from_an_access_only_session() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/accounts/check",
        get(|| async {
            Json(serde_json::json!({
                "account_ordering": ["workspace-private"],
                "accounts": {
                    "workspace-private": {
                        "account": { "workspace_id": "workspace-private" }
                    }
                }
            }))
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let endpoint = Url::parse(&format!("http://{address}/accounts/check")).unwrap();

    let account_id = lookup_import_account_id(
        endpoint,
        "synthetic-access-only-token",
        None,
        Duration::from_secs(2),
    )
    .await
    .unwrap();

    assert_eq!(account_id, "workspace-private");
    server.abort();
}
#[tokio::test]
async fn imported_explicit_email_wins_over_shared_token_email() {
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "email": "shared@example.test",
            "https://api.openai.com/auth": {
                "chatgpt_user_id": "shared-user",
                "chatgpt_account_id": "shared-team"
            }
        })
        .to_string(),
    );
    let token = format!("header.{payload}.signature");
    let mut parsed = parse_import(
        &serde_json::json!({
            "email": "member@example.test",
            "access_token": token
        })
        .to_string(),
        None,
        &[],
    )
    .unwrap();
    let material =
        build_import_credential_material(parsed.items.remove(0), 1, None, None, None, 20)
            .await
            .unwrap();
    assert_eq!(material.email.as_deref(), Some("member@example.test"));
}
#[test]
fn quota_response_types_are_safe_and_serializable() {
    let response = AccountQuotaOutcome::Failed {
        code: "quota_transport".into(),
        retryable: true,
    };
    let serialized = serde_json::to_string(&response).unwrap();
    assert!(serialized.contains("quota_transport"));
    assert!(!serialized.contains("Bearer"));
    let _ = WireApi::Responses;
}
#[test]
fn missing_import_secret_requires_recovery() {
    let error = import_session_error(ImportSessionError {
        code: ImportSessionErrorCode::SecretMissing,
        message: "import session secret is missing".into(),
        session_id: None,
        import_code: None,
    });
    assert!(serde_json::to_string(&error)
        .unwrap()
        .contains("recovery_required"));
}
