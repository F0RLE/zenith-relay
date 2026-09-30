use crate::local_pool::models::CURRENT_SCHEMA_VERSION;
use serde::Serialize;
use std::fs;
use std::path::Path;

mod refresh;
mod rotation_upgrade;
mod source_refresh;
use super::*;
use crate::local_pool::models::{OwnershipOperationKind, OwnershipOperationPhase};
use std::collections::{BTreeMap, BTreeSet};
use std::{
    env,
    sync::atomic::{AtomicU64, Ordering},
};
use zenith_relay_core::{
    accounts::{
        AccountAuthMode, AccountAuthState, AccountHealthState, AccountIdentity, AccountRecord,
    },
    quota::{QuotaSnapshot, Subscription},
    WireApi,
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

fn temp_root() -> PathBuf {
    let root = env::temp_dir().join(format!(
        "zenith-relay-store-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    root
}

#[test]
fn fresh_store_is_versioned_and_restart_safe() {
    let root = temp_root();
    let store = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(store.gateway().port, 14998);
    assert_eq!(store.database().state_count().unwrap(), 9);
    assert!(root.join("data/database/relay.sqlite").exists());
    assert!(!root.join("data/metadata.json").exists());
    drop(store);
    assert_eq!(
        LocalPoolStore::open(root.clone()).unwrap().gateway().port,
        14998
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quota_and_routing_policy_survive_restart() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let mut gateway = store.gateway().clone();
    gateway.tool_policy = zenith_relay_core::ToolPolicy {
        mode: zenith_relay_core::ToolPolicyMode::Automatic,
    };
    gateway.quota_request_timeout_seconds = 10;
    gateway.chatgpt_interface_quota_reserve_basis_points = 700;
    gateway.image_base_model = Some("gpt-5.4-mini".into());
    gateway.model_price_overrides.insert(
        "GPT-5.4".into(),
        zenith_relay_core::ApiModelPriceOverride {
            input_micro_usd_per_million: 1_250_000,
            cached_input_micro_usd_per_million: Some(125_000),
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: 7_500_000,
        },
    );
    store.replace_gateway(gateway).unwrap();
    drop(store);

    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(
        reopened.gateway().tool_policy.mode,
        zenith_relay_core::ToolPolicyMode::Automatic
    );
    assert_eq!(
        reopened.gateway().tool_policy.mode,
        zenith_relay_core::ToolPolicyMode::Automatic
    );
    assert_eq!(reopened.gateway().quota_request_timeout_seconds, 10);
    assert_eq!(
        reopened
            .gateway()
            .chatgpt_interface_quota_reserve_basis_points,
        700
    );
    assert_eq!(
        reopened.gateway().image_base_model.as_deref(),
        Some("gpt-5.4-mini")
    );
    assert_eq!(
        reopened.gateway().model_price_overrides.get("gpt-5.4"),
        Some(&zenith_relay_core::ApiModelPriceOverride {
            input_micro_usd_per_million: 1_250_000,
            cached_input_micro_usd_per_million: Some(125_000),
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: 7_500_000,
        })
    );
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn remote_target_survives_restart() {
    let root = temp_root();
    let target = RemoteTargetRecord {
        origin: "https://relay.example.test".into(),
        server_id: "server_1".into(),
        identity_fingerprint: "sha256:test".into(),
        server_version: "1.1.0".into(),
        protocol_version: 1,
        allow_insecure_http: false,
        secret_ref: "remote:server_1".into(),
        connected_at_ms: 123,
    };
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    store.replace_remote_target(Some(target.clone())).unwrap();
    drop(store);

    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(reopened.remote_target(), Some(&target));
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ownership_operation_survives_restart_without_secret_material() {
    let root = temp_root();
    let operation = OwnershipOperationRecord {
        id: "ownership_0123456789abcdef0123456789abcdef".into(),
        kind: OwnershipOperationKind::MoveToRemote,
        phase: OwnershipOperationPhase::MoveRemoteCommitted,
        server_id: "server_1".into(),
        local_account_ids: vec!["account_local".into()],
        remote_account_ids: vec!["account_remote".into()],
        created_remote_account_ids: vec!["account_remote".into()],
        created_at_ms: 100,
        updated_at_ms: 200,
    };
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    store
        .replace_ownership_operation(Some(operation.clone()))
        .unwrap();
    drop(store);

    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(reopened.ownership_operation(), Some(&operation));
    let stored = reopened
        .database()
        .state_json(STATE_OWNERSHIP_OPERATION)
        .unwrap()
        .unwrap();
    assert!(!stored.contains("access_token"));
    assert!(!stored.contains("refresh_token"));
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupt_database_state_is_rejected_without_deleting_the_database() {
    let root = temp_root();
    let store = LocalPoolStore::open(root.clone()).unwrap();
    store
        .database()
        .replace_state_json(&[(STATE_GATEWAY, "not-json".to_string())])
        .unwrap();
    drop(store);
    let error = LocalPoolStore::open(root.clone()).err().unwrap();
    assert!(matches!(error.code, ErrorCode::RecoveryRequired));
    assert!(root.join("data/database/relay.sqlite").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_json_store_is_imported_once_and_removed() {
    let root = temp_root();
    let data = root.join("data");
    fs::create_dir_all(&data).unwrap();
    write_json(
        &data.join("metadata.json"),
        &serde_json::json!({"schemaVersion": CURRENT_SCHEMA_VERSION}),
    );
    write_json(&data.join("settings.json"), &GatewaySettings::default());
    write_json(
        &data.join("connections.json"),
        &Vec::<ProviderSourceRecord>::new(),
    );
    write_json(
        &data.join("accounts.json"),
        &vec![account_record("imported")],
    );
    write_json(
        &data.join("pool-keys.json"),
        &Vec::<LocalGatewayKeyRecord>::new(),
    );
    write_json(
        &data.join("automations.json"),
        &AutomationRecords::default(),
    );
    rusqlite::Connection::open(data.join("usage.sqlite")).unwrap();

    let store = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(store.accounts()[0].account.id, "imported");
    assert!(data.join("database/relay.sqlite").exists());
    assert!(!data.join("usage.sqlite").exists());
    for name in LEGACY_STATE_FILES {
        assert!(!data.join(name).exists());
    }
    drop(store);
    assert_eq!(
        LocalPoolStore::open(root.clone()).unwrap().accounts()[0]
            .account
            .id,
        "imported"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn flat_relay_database_and_sidecars_move_together() {
    let root = temp_root();
    let data = root.join("data");
    let database = data.join("database");
    fs::create_dir_all(&database).unwrap();
    fs::write(data.join("relay.sqlite"), "database").unwrap();
    fs::write(data.join("relay.sqlite-wal"), "wal").unwrap();
    fs::write(data.join("relay.sqlite-shm"), "shm").unwrap();
    fs::write(data.join("relay.sqlite-journal"), "journal").unwrap();

    persistence::migrate_database_file(&data, &database).unwrap();

    assert_eq!(
        fs::read_to_string(database.join("relay.sqlite")).unwrap(),
        "database"
    );
    assert_eq!(
        fs::read_to_string(database.join("relay.sqlite-wal")).unwrap(),
        "wal"
    );
    assert_eq!(
        fs::read_to_string(database.join("relay.sqlite-shm")).unwrap(),
        "shm"
    );
    assert_eq!(
        fs::read_to_string(database.join("relay.sqlite-journal")).unwrap(),
        "journal"
    );
    assert!(!data.join("relay.sqlite").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn interrupted_database_move_finishes_the_remaining_sidecars() {
    let root = temp_root();
    let data = root.join("data");
    let database = data.join("database");
    fs::create_dir_all(&database).unwrap();
    fs::write(database.join("relay.sqlite"), "database").unwrap();
    fs::write(data.join("relay.sqlite-wal"), "wal").unwrap();
    fs::write(data.join("relay.sqlite-shm"), "shm").unwrap();
    fs::write(data.join("relay.sqlite-journal"), "journal").unwrap();

    persistence::migrate_database_file(&data, &database).unwrap();

    assert_eq!(
        fs::read_to_string(database.join("relay.sqlite-wal")).unwrap(),
        "wal"
    );
    assert_eq!(
        fs::read_to_string(database.join("relay.sqlite-shm")).unwrap(),
        "shm"
    );
    assert_eq!(
        fs::read_to_string(database.join("relay.sqlite-journal")).unwrap(),
        "journal"
    );
    assert!(!data.join("relay.sqlite-wal").exists());
    assert!(!data.join("relay.sqlite-shm").exists());
    assert!(!data.join("relay.sqlite-journal").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn competing_flat_and_categorized_databases_are_not_merged() {
    let root = temp_root();
    let data = root.join("data");
    let database = data.join("database");
    fs::create_dir_all(&database).unwrap();
    fs::write(data.join("relay.sqlite"), "legacy").unwrap();
    fs::write(database.join("relay.sqlite"), "current").unwrap();

    assert!(persistence::migrate_database_file(&data, &database).is_err());
    assert_eq!(
        fs::read_to_string(data.join("relay.sqlite")).unwrap(),
        "legacy"
    );
    assert_eq!(
        fs::read_to_string(database.join("relay.sqlite")).unwrap(),
        "current"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_and_key_records_survive_restart_without_secret_values() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    store
        .upsert_source(ProviderSourceRecord {
            id: "source_1".into(),
            name: "Synthetic".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "https://example.test/v1".into(),
            secret_ref: "source:source_1".into(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_config: Default::default(),
            protocol_bindings: Vec::new(),
            models: vec!["gpt-test".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: Default::default(),
            detected_model_prices: std::collections::BTreeMap::from([(
                "gpt-test".into(),
                zenith_relay_core::ApiModelPriceOverride {
                    input_micro_usd_per_million: 1_000_000,
                    cached_input_micro_usd_per_million: Some(100_000),
                    cache_write_5m_micro_usd_per_million: None,
                    cache_write_1h_micro_usd_per_million: None,
                    output_micro_usd_per_million: 2_000_000,
                },
            )]),
            last_used_at: None,
            last_test_at: None,
            last_test_status: None,
            last_error: None,
        })
        .unwrap();
    store
        .upsert_key(LocalGatewayKeyRecord {
            id: "key_1".into(),
            label: "Default".into(),
            enabled: true,
            system: false,
            secret_ref: "key:key_1".into(),
            created_at: "2026-07-10T00:00:00Z".into(),
            last_used_at: None,
        })
        .unwrap();
    drop(store);

    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(reopened.sources()[0].models, ["gpt-test"]);
    assert_eq!(
        reopened.sources()[0].detected_model_prices.get("gpt-test"),
        Some(&zenith_relay_core::ApiModelPriceOverride {
            input_micro_usd_per_million: 1_000_000,
            cached_input_micro_usd_per_million: Some(100_000),
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: 2_000_000,
        })
    );
    assert_eq!(reopened.keys()[0].secret_ref, "key:key_1");
    let records = reopened
        .database()
        .state_json(STATE_SOURCES)
        .unwrap()
        .unwrap();
    assert!(!records.contains("upstream-secret"));
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_transaction_preserves_all_state_and_memory() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let source = ProviderSourceRecord {
        id: "source_1".into(),
        name: "Before".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        base_url: "https://example.test/v1".into(),
        secret_ref: "source:source_1".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_config: Default::default(),
        protocol_bindings: Vec::new(),
        models: vec!["gpt-test".into()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: Default::default(),
        detected_model_prices: Default::default(),
        last_used_at: None,
        last_test_at: None,
        last_test_status: None,
        last_error: None,
    };
    let key = LocalGatewayKeyRecord {
        id: "key_1".into(),
        label: "Before".into(),
        enabled: true,
        system: false,
        secret_ref: "key:key_1".into(),
        created_at: "2026-07-10T00:00:00Z".into(),
        last_used_at: None,
    };
    store
        .replace_records(vec![source.clone()], vec![key.clone()])
        .unwrap();
    let mut changed_source = source;
    changed_source.name = "After".into();
    let mut changed_key = key;
    changed_key.label = "x".repeat(17 * 1024 * 1024);

    assert!(store
        .replace_records(vec![changed_source], vec![changed_key])
        .is_err());
    assert_eq!(store.sources()[0].name, "Before");
    drop(store);
    assert_eq!(
        LocalPoolStore::open(root.clone()).unwrap().sources()[0].name,
        "Before"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn store_rejects_account_overflow_without_changing_persisted_state() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let accounts = (0..MAX_LOCAL_ACCOUNTS)
        .map(|index| account_record(&format!("account-{index}")))
        .collect::<Vec<_>>();
    store
        .replace_accounts_and_keys(accounts.clone(), Vec::new())
        .unwrap();

    let mut overflow = accounts;
    overflow.push(account_record("account-overflow"));
    let error = store
        .replace_accounts_and_keys(overflow, Vec::new())
        .unwrap_err();
    assert!(matches!(error.code, ErrorCode::InvalidState));
    assert_eq!(store.accounts().len(), MAX_LOCAL_ACCOUNTS);
    drop(store);
    assert_eq!(
        LocalPoolStore::open(root.clone()).unwrap().accounts().len(),
        MAX_LOCAL_ACCOUNTS
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reset_clears_runtime_records_but_keeps_profile_backups() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    store
        .replace_accounts_and_keys(vec![account_record("account-reset")], Vec::new())
        .unwrap();
    let backup = root.join("recovery/profiles/config.toml");
    fs::create_dir_all(backup.parent().unwrap()).unwrap();
    fs::write(&backup, "preserved").unwrap();

    store.reset_local_records().unwrap();
    drop(store);

    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    assert!(reopened.accounts().is_empty());
    assert!(reopened.sources().is_empty());
    assert!(reopened.keys().is_empty());
    assert!(reopened.automations().tasks.is_empty());
    assert!(!reopened.gateway().enabled);
    assert_eq!(fs::read_to_string(backup).unwrap(), "preserved");
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

fn account_record(id: &str) -> LocalAccountRecord {
    LocalAccountRecord {
        account: AccountRecord {
            id: id.into(),
            label: id.into(),
            identity: AccountIdentity::from_hashed_parts(
                "openai",
                "chatgpt.com/backend-api/codex",
                &format!("identity-{id}"),
                &format!("secret-{id}"),
                "default",
                None,
            )
            .unwrap(),
            auth_mode: AccountAuthMode::OAuth,
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            source_id: "openai_codex".into(),
            secret_refs: vec![format!("account:{id}")],
            subscription: Subscription::default(),
            quota: QuotaSnapshot::default(),
            token_generation: 1,
            token_updated_at_ms: Some(1),
            tags: BTreeSet::new(),
            enabled: true,
            in_pool: true,
            draining: false,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
        },
        provider_family: Some("openai".into()),
        purchase_cost_micro_usd: None,
        remote_location: None,
        wire_api: WireApi::Responses,
        models: vec!["gpt-test".into()],
        discovered_models: None,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
        client_auth_status: None,
        last_client_login_redirect_at_ms: None,
    }
}

#[test]
fn client_auth_observation_is_durable_and_does_not_change_account_policy() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let mut account = account_record("observed-account");
    account.account.in_pool = true;
    store.upsert_account(account).unwrap();

    assert!(store
        .update_client_auth_observation(
            "observed-account",
            Some("login_required".into()),
            Some(123),
        )
        .unwrap());
    assert!(!store
        .update_client_auth_observation(
            "observed-account",
            Some("login_required".into()),
            Some(123),
        )
        .unwrap());
    let observed = store.account("observed-account").unwrap();
    assert_eq!(
        observed.client_auth_status.as_deref(),
        Some("login_required")
    );
    assert_eq!(observed.last_client_login_redirect_at_ms, Some(123));
    assert!(observed.account.enabled);
    assert!(observed.account.in_pool);

    drop(store);
    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    let observed = reopened.account("observed-account").unwrap();
    assert_eq!(
        observed.client_auth_status.as_deref(),
        Some("login_required")
    );
    assert_eq!(observed.last_client_login_redirect_at_ms, Some(123));
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn conditional_account_rollback_keeps_other_account_changes_and_watchdog_state() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let previous = account_record("rollback-target");
    let mut attempted = previous.clone();
    attempted.account.label = "Attempted account edit".into();
    let mut unrelated = account_record("rollback-unrelated");
    unrelated.account.last_error_code = Some("newer_quota_observation".into());
    store
        .replace_accounts_and_keys(vec![attempted.clone(), unrelated.clone()], Vec::new())
        .unwrap();
    store
        .update_client_auth_observation("rollback-target", Some("login_required".into()), Some(123))
        .unwrap();

    assert!(store
        .restore_account_if_current(&previous, &attempted)
        .unwrap());
    let restored = store.account("rollback-target").unwrap();
    assert_eq!(restored.account.label, previous.account.label);
    assert_eq!(
        restored.client_auth_status.as_deref(),
        Some("login_required")
    );
    assert_eq!(restored.last_client_login_redirect_at_ms, Some(123));
    assert_eq!(store.account("rollback-unrelated"), Some(&unrelated));

    let mut newer = attempted.clone();
    newer.account.token_generation = newer.account.token_generation.saturating_add(1);
    store.upsert_account(newer.clone()).unwrap();
    assert!(!store
        .restore_account_if_current(&previous, &attempted)
        .unwrap());
    assert_eq!(store.account("rollback-target"), Some(&newer));

    drop(store);
    fs::remove_dir_all(root).unwrap();
}

fn write_json(path: &Path, value: &impl Serialize) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
