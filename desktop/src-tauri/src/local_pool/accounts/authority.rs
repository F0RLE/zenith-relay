mod diagnostics;
mod lock;

use super::credentials::CredentialStore;
pub use lock::{ProcessAccountGuard, ProcessAccountLocks, ProcessLockConfig, ProcessLockError};
use std::{path::PathBuf, sync::Arc};
pub use zenith_relay_core::providers::chatgpt::connector::authority::{
    AccountMetadataSink, CodexRefreshClient, CredentialPersistence, MetadataSinkError,
    StoredRefreshAdapter,
};

pub fn default_refresh_adapter<B, C>(
    root: PathBuf,
    credentials: CredentialStore<B>,
    client: Arc<C>,
    skew_ms: u64,
) -> Result<StoredRefreshAdapter<B, C>, ProcessLockError> {
    refresh_adapter_with_config(
        root,
        credentials,
        client,
        skew_ms,
        ProcessLockConfig::default(),
    )
}

pub fn refresh_adapter_with_config<B, C>(
    root: PathBuf,
    credentials: CredentialStore<B>,
    client: Arc<C>,
    skew_ms: u64,
    config: ProcessLockConfig,
) -> Result<StoredRefreshAdapter<B, C>, ProcessLockError> {
    let locks = Arc::new(ProcessAccountLocks::with_config(root, config)?);
    Ok(StoredRefreshAdapter::new(
        credentials,
        client,
        locks,
        skew_ms,
    ))
}

pub fn credential_persistence<B, M>(
    credentials: CredentialStore<B>,
    metadata: Arc<M>,
    root: PathBuf,
) -> CredentialPersistence<B, M> {
    let locks = Arc::new(
        ProcessAccountLocks::with_config(root, ProcessLockConfig::default())
            .expect("default credential lock config is valid"),
    );
    CredentialPersistence::new(credentials, metadata, locks)
}

#[cfg(test)]
mod tests;
