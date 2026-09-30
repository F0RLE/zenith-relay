#[cfg(test)]
use crate::local_pool::accounts::authority::{ProcessAccountLocks, ProcessLockConfig};
#[cfg(test)]
use crate::local_pool::accounts::credentials::CredentialStore;
use crate::local_pool::accounts::credentials::StoredCodexCredentials;
#[cfg(test)]
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::models::LocalAccountRecord;
#[cfg(test)]
use crate::local_pool::state::DesktopState;
#[cfg(test)]
use zenith_relay_core::accounts::AccountAuthState;

mod rollback;
mod write;

#[cfg(test)]
use rollback::{reconcile_import_authority, restore_import_durable_state_if_current};
pub(in crate::local_pool::accounts) use write::persist_imported_account;

#[derive(Clone)]
pub(super) struct ImportedAccountCommit {
    pub(super) account_id: String,
    pub(super) previous_credentials: Option<StoredCodexCredentials>,
    pub(super) previous_account: Option<LocalAccountRecord>,
    pub(super) attempted_credentials: StoredCodexCredentials,
    pub(super) attempted_account: LocalAccountRecord,
    pub(super) runtime_sync_required: bool,
}

#[cfg(test)]
mod tests;
