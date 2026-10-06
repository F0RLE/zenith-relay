mod account;
mod profile;

use super::{LocalPoolError, Result};
use serde::Serialize;

pub(super) fn serialize_pretty<T: Serialize + ?Sized>(value: &T) -> Result<String> {
    let content = serde_json::to_string_pretty(value).map_err(LocalPoolError::invalid_state)?;
    Ok(format!("{content}\n"))
}

pub(super) use account::{
    account_auth_content, account_auth_matches_snapshot, account_auth_matches_tokens,
    account_backup_for_profile, account_backup_path, account_backup_secret_ref,
    account_managed_config_matches, attach_account_config, auth_credential_kind,
    auth_snapshot_json, binding_from_backup, canonical_profile_dir, cleanup_account_attach_secrets,
    credential_kind_locked, ensure_single_profile_backup, fill_missing_account_config,
    parse_account_backup, parse_account_backup_snapshot, restore_account_config,
    rollback_account_backup, serialize_account_backup,
};
pub(super) use profile::{
    cleanup_created_backup_secret, delete_backup_secrets, discard_backup,
    discard_managed_binding_locked, parse_backup_snapshot, previous_auth_snapshot,
    restore_secret_snapshot, rollback_backup, serialize_backup,
};
