use super::{export_ops::*, import_orchestrator::*, mutations::*, quota_refresh::*};
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::import_session::{
    ImportSessionError, ImportSessionErrorCode, ImportSessionStore,
};
use crate::local_pool::accounts::{records, NativeSecretBackend};
use crate::local_pool::commands::current_time_ms;
use crate::local_pool::error::{ErrorCode, LocalPoolError};
use crate::local_pool::models::{AutomationRecords, LocalAccountRecord, ProviderSourceRecord};
use crate::local_pool::profiles::codex;
use crate::local_pool::state::DesktopState;
use crate::local_pool::store::secret_store;
use axum::http::StatusCode;
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
use zenith_relay_core::providers::chatgpt::{
    CodexSubscriptionMetadata, ModelDiscoveryFailure, ModelDiscoveryFailureCode,
    QuotaRefreshOutcome,
};
use zenith_relay_core::quota::{QuotaRefreshFailure, QuotaWindowKind, Subscription};
use zenith_relay_core::{
    MessagesReasoningMode, ProviderSource, SourceAdapter, SourceProtocolBinding, WireApi,
};
mod identity;
mod importing;
mod inventory;
mod refresh;

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

async fn spawn_import_account_check_server(account_id: &str) -> (Url, tokio::task::JoinHandle<()>) {
    spawn_import_account_check_server_for_accounts(&[account_id]).await
}

async fn spawn_import_account_check_server_for_accounts(
    account_ids: &[&str],
) -> (Url, tokio::task::JoinHandle<()>) {
    spawn_import_account_check_payload(serde_json::json!({
        "accounts": account_ids
            .iter()
            .map(|account_id| serde_json::json!({"account": {"id": account_id}}))
            .collect::<Vec<_>>()
    }))
    .await
}

async fn spawn_import_account_check_payload(
    payload: serde_json::Value,
) -> (Url, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/accounts/check",
        get(move || {
            let payload = payload.clone();
            async move { Json(payload) }
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        Url::parse(&format!("http://{address}/accounts/check")).unwrap(),
        server,
    )
}

async fn spawn_rejected_import_account_check() -> (Url, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/accounts/check",
        get(|| async { StatusCode::UNAUTHORIZED }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        Url::parse(&format!("http://{address}/accounts/check")).unwrap(),
        server,
    )
}
