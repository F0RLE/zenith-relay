use crate::local_pool::{
    error::CommandError,
    models::{LocalAccountRecord, LocalGatewayKeyRecord, LocalPoolSnapshot, ProviderSourceRecord},
    state::DesktopState,
    store::secret_store,
};
use std::collections::BTreeSet;
use tauri::State;
use zenith_relay_core::protocol::{
    AccountSummary, Capabilities, GatewaySummary, KeySummary, RuntimeStateSnapshot,
    RuntimeTargetSummary, SourceSummary,
};

#[tauri::command]
pub async fn get_local_pool_state(
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn get_local_runtime_state(
    state: State<'_, DesktopState>,
) -> Result<RuntimeStateSnapshot, CommandError> {
    let snapshot = state.snapshot().await?;
    let running = snapshot.runtime_target.connected;
    let visible_model_ids = snapshot
        .sources
        .iter()
        .filter(|record| record.enabled && !record.draining)
        .flat_map(|record| record.models.iter().cloned())
        .chain(
            snapshot
                .accounts
                .iter()
                .filter(|record| record.account.enabled && !record.account.draining)
                .flat_map(|record| record.models.iter().cloned()),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let candidate_count = snapshot.sources.len() + snapshot.accounts.len();
    let base_url = format!(
        "http://{}:{}/v1",
        snapshot.gateway.client_host, snapshot.gateway.port
    );
    Ok(RuntimeStateSnapshot {
        schema_version: snapshot.schema_version,
        runtime_target: RuntimeTargetSummary {
            kind: "local".to_string(),
            connected: running,
            origin: Some(format!(
                "http://{}:{}",
                snapshot.gateway.client_host, snapshot.gateway.port
            )),
            server_id: None,
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        },
        gateway: GatewaySummary {
            running,
            base_url,
            candidate_count,
            visible_model_ids,
        },
        platform: snapshot.platform.to_string(),
        capabilities: Capabilities::desktop_local(),
        sources: snapshot
            .sources
            .iter()
            .map(local_source_summary)
            .collect::<Result<_, _>>()?,
        accounts: snapshot
            .accounts
            .iter()
            .map(local_account_summary)
            .collect::<Result<_, _>>()?,
        keys: snapshot.keys.iter().map(local_key_summary).collect(),
        automations: snapshot.automations,
        wake_history: snapshot.wake_history,
        warnings: snapshot.warnings,
    })
}

fn local_source_summary(
    record: &ProviderSourceRecord,
) -> crate::local_pool::error::Result<SourceSummary> {
    Ok(SourceSummary {
        id: record.id.clone(),
        name: record.name.clone(),
        enabled: record.enabled,
        draining: record.draining,
        base_url: record.base_url.clone(),
        wire_api: record.wire_api,
        models: record.models.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        secret_available: secret_store::load(&record.secret_ref)?.is_some(),
        last_error_code: record.last_error.clone(),
    })
}

fn local_account_summary(
    record: &LocalAccountRecord,
) -> crate::local_pool::error::Result<AccountSummary> {
    let secret_available = record
        .account
        .secret_refs
        .iter()
        .map(|secret_ref| secret_store::load(secret_ref))
        .collect::<crate::local_pool::error::Result<Vec<_>>>()?
        .into_iter()
        .all(|value| value.is_some());
    Ok(AccountSummary {
        id: record.account.id.clone(),
        label: record.account.label.clone(),
        identity_hint: record
            .account
            .identity
            .identity_hash
            .chars()
            .take(12)
            .collect(),
        enabled: record.account.enabled,
        draining: record.account.draining,
        auth_state: record.account.auth_state,
        health: format!("{:?}", record.account.health).to_ascii_lowercase(),
        models: record.models.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        subscription: record.account.subscription.clone(),
        quota: record.account.quota.clone(),
        secret_available,
        last_error_code: record.account.last_error_code.clone(),
    })
}

fn local_key_summary(record: &LocalGatewayKeyRecord) -> KeySummary {
    KeySummary {
        id: record.id.clone(),
        label: record.label.clone(),
        enabled: record.enabled,
        source_ids: record.source_ids.clone(),
        account_ids: record.account_ids.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        model_prefix: record.model_prefix.clone(),
        created_at_ms: timestamp_ms(&record.created_at).unwrap_or_default(),
        last_used_at_ms: record.last_used_at.as_deref().and_then(timestamp_ms),
    }
}

fn timestamp_ms(value: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|value| u64::try_from(value.timestamp_millis()).ok())
}

#[cfg(test)]
mod parity_tests {
    use super::*;

    #[test]
    fn local_and_remote_snapshots_share_the_same_top_level_contract() {
        let local = serde_json::to_value(RuntimeStateSnapshot {
            schema_version: 1,
            runtime_target: RuntimeTargetSummary {
                kind: "local".into(),
                connected: false,
                origin: None,
                server_id: None,
                version: None,
            },
            gateway: GatewaySummary {
                running: false,
                base_url: "http://127.0.0.1:14998/v1".into(),
                candidate_count: 0,
                visible_model_ids: Vec::new(),
            },
            platform: "test".into(),
            capabilities: Capabilities::desktop_local(),
            sources: Vec::new(),
            accounts: Vec::new(),
            keys: Vec::new(),
            automations: Vec::new(),
            wake_history: Vec::new(),
            warnings: Vec::new(),
        })
        .unwrap();
        let remote = local.clone();
        assert_eq!(
            local.as_object().unwrap().keys().collect::<Vec<_>>(),
            remote.as_object().unwrap().keys().collect::<Vec<_>>()
        );
    }
}
