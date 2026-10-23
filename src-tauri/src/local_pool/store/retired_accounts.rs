use super::{persistence, LocalPoolStore, TelemetryDb, STATE_ACCOUNTS};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::LocalAccountRecord,
};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

pub(super) const LEGACY_TARGET_FILE: &str = "remote-target.json";
pub(super) const STATE_REMOTE_TARGET: &str = "remote_target";
pub(super) const STATE_OWNERSHIP_OPERATION: &str = "ownership_operation";

#[derive(Deserialize)]
struct LegacyTarget {
    active: Option<Value>,
}

pub(super) fn legacy_entries(root: &Path) -> Result<Vec<(&'static str, String)>> {
    let path = root.join(LEGACY_TARGET_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let target: LegacyTarget = persistence::read_legacy_json(&path)?;
    Ok(vec![(
        STATE_REMOTE_TARGET,
        persistence::serialize_state(&target.active)?,
    )])
}

fn saved_state(database: &TelemetryDb, key: &str) -> Result<Option<Value>> {
    let Some(saved) = database.state_json(key)? else {
        return Ok(None);
    };
    serde_json::from_str(&saved).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "saved retired-server state is invalid",
        )
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingAccounts {
    server_id: String,
    local_account_ids: Vec<String>,
    #[serde(default)]
    remote_account_ids: Vec<String>,
}

pub(super) fn migrate(database: &TelemetryDb, accounts: &mut [LocalAccountRecord]) -> Result<()> {
    let target = saved_state(database, STATE_REMOTE_TARGET)?;
    let operation = saved_state(database, STATE_OWNERSHIP_OPERATION)?;
    let pending = operation
        .clone()
        .map(serde_json::from_value::<PendingAccounts>)
        .transpose()
        .map_err(|_| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "saved account ownership is invalid",
            )
        })?;
    if pending.as_ref().is_some_and(|pending| {
        !zenith_relay_core::is_ascii_token(&pending.server_id, 128)
            || pending.local_account_ids.is_empty()
            || pending.local_account_ids.len() > 256
            || pending
                .local_account_ids
                .iter()
                .any(|id| !zenith_relay_core::is_ascii_token(id, 128))
            || pending.remote_account_ids.len() > pending.local_account_ids.len()
            || pending
                .remote_account_ids
                .iter()
                .any(|id| !zenith_relay_core::is_ascii_token(id, 128))
    }) {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "saved account ownership is invalid",
        ));
    }
    let saved_accounts = database
        .state_json(STATE_ACCOUNTS)?
        .map(|saved| serde_json::from_str::<Value>(&saved))
        .transpose()
        .map_err(|_| {
            LocalPoolError::new(ErrorCode::RecoveryRequired, "saved accounts are invalid")
        })?;
    let mut changed = saved_accounts
        .as_ref()
        .and_then(Value::as_array)
        .is_some_and(|saved| {
            saved
                .iter()
                .any(|account| account.get("remoteLocation").is_some())
        });
    for account in accounts.iter_mut() {
        if pending
            .as_ref()
            .is_some_and(|pending| pending.local_account_ids.contains(&account.account.id))
            && account.recovery.is_none()
        {
            account.recovery = Some(serde_json::json!({ "reason": "retired_server" }));
            changed = true;
        }
        if account.recovery.is_some() && (account.account.enabled || account.account.in_pool) {
            account.account.enabled = false;
            account.account.in_pool = false;
            changed = true;
        }
    }
    let mut entries = Vec::new();
    if changed {
        entries.push((STATE_ACCOUNTS, persistence::serialize_state(&accounts)?));
    }
    if target.is_some() || operation.is_some() {
        entries.extend([
            (
                "retired_server_state",
                persistence::serialize_state(
                    &serde_json::json!({ "target": target, "operation": operation }),
                )?,
            ),
            (STATE_REMOTE_TARGET, "null".into()),
            (STATE_OWNERSHIP_OPERATION, "null".into()),
        ]);
    }
    if !entries.is_empty() {
        database.replace_state_json(&entries)?;
    }
    Ok(())
}

impl LocalPoolStore {
    pub fn recover_account_locally(&mut self, account_id: &str, confirmed: bool) -> Result<()> {
        if !confirmed {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "confirm the external account owner is stopped before recovery",
            ));
        }
        let mut accounts = self.accounts.clone();
        let account = accounts
            .iter_mut()
            .find(|account| account.account.id == account_id)
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
        if account.recovery.is_none() {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "account does not require local recovery",
            ));
        }
        account.recovery = None;
        account.account.enabled = false;
        account.account.in_pool = false;
        if account.account.last_error_code.as_deref() == Some("remote_missing") {
            account.account.last_error_code = None;
        }
        // Recovery never enables or dispatches the account.
        self.replace_accounts_and_keys(accounts, self.keys.clone())
    }
}
