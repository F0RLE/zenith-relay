use crate::state::{
    is_internal_gateway_key, now_ms, AccountCredential, AppState, ServerAccountRecord, SourceRecord,
};
use crate::token_refresh::{find_account, CodexRefreshClient, ServerTokenPersistence};
use crate::usage_writer::UsageWriter;
use std::sync::Arc;
#[cfg(test)]
use zenith_relay_core::accounts::AccountHealthState;
use zenith_relay_core::{
    accounts::TokenSet, protocol::RuntimeStateSnapshot, CandidateScope, GatewayRuntime,
    UsageCallback,
};

mod account_runtime;
mod runtime_build;
mod runtime_records;
mod snapshot;

pub(crate) use account_runtime::{account_proxy_config, prepare_server_account_authorization};

/// Hold across a durable configuration edit and its hot apply, replacement or
/// rollback. Acquire configuration_lock first when both are needed. Builds
/// never acquire configuration_lock internally or perform provider HTTP.
pub(crate) struct RuntimeBuildGuard<'a> {
    _guard: tokio::sync::MutexGuard<'a, ()>,
}

impl RuntimeBuildGuard<'_> {
    fn retire_after_failed_restore(state: &AppState, error: String) -> String {
        match state.replace_runtime(None) {
            Ok(()) => error,
            Err(retire_error) => {
                format!("{error}; failed to retire previous runtime: {retire_error}")
            }
        }
    }

    pub(crate) async fn rebuild(&self, state: &Arc<AppState>) -> Result<(), String> {
        runtime_build::rebuild(state).await
    }

    pub(crate) async fn rebuild_or_rollback<F>(
        &self,
        state: &Arc<AppState>,
        rollback: F,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Result<(), String>,
    {
        let Err(error) = self.rebuild(state).await else {
            return Ok(());
        };
        if let Err(rollback_error) = rollback() {
            return Err(Self::retire_after_failed_restore(
                state,
                format!("{error}; failed to restore persisted state: {rollback_error}"),
            ));
        }
        if let Err(restore_error) = self.rebuild(state).await {
            return Err(Self::retire_after_failed_restore(
                state,
                format!("{error}; failed to rebuild previous runtime: {restore_error}"),
            ));
        }
        Err(error)
    }

    pub(crate) async fn rollback_and_rebuild<F>(
        &self,
        state: &Arc<AppState>,
        rollback: F,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Result<(), String>,
    {
        if let Err(error) = rollback() {
            return Err(Self::retire_after_failed_restore(
                state,
                format!("failed to restore persisted state: {error}"),
            ));
        }
        self.rebuild(state).await.map_err(|error| {
            Self::retire_after_failed_restore(
                state,
                format!("failed to rebuild previous runtime: {error}"),
            )
        })
    }
}

impl AppState {
    pub async fn prepare_account_tokens(
        self: &Arc<Self>,
        expected: &ServerAccountRecord,
    ) -> Result<TokenSet, String> {
        // Import holds this lock through the new record commit and slot
        // removal. A late preparation cannot register an old login afterward.
        let configuration = self.configuration_lock.lock().await;
        let record = find_account(self, &expected.id)?;
        if record.secret_ref != expected.secret_ref {
            return Err("account login changed during authorization".into());
        }
        let secret = self
            .vault
            .load(&record.secret_ref)?
            .ok_or_else(|| "stored account credential is missing".to_string())?;
        let credential: AccountCredential = serde_json::from_str(&secret)
            .map_err(|_| "stored account credential is invalid".to_string())?;
        self.token_authority
            .register_if_absent(&record.id, credential.tokens()?, record.auth_state)
            .map_err(|error| error.to_string())?;
        let proxy = account_proxy_config(self, &record, &credential)?;
        let refresh = CodexRefreshClient::new_with_proxy(proxy.as_ref())?;
        let persistence = ServerTokenPersistence::for_account(self.clone(), &record);
        drop(configuration);
        let tokens = self
            .token_authority
            .prepare_and_persist(
                &record.id,
                now_ms(),
                zenith_relay_core::accounts::TOKEN_REFRESH_SKEW_MS,
                &refresh,
                &persistence,
            )
            .await
            .map(|prepared| prepared.tokens)
            .map_err(|error| error.to_string())?;
        let _configuration = self.configuration_lock.lock().await;
        if find_account(self, &record.id)?.secret_ref != record.secret_ref {
            return Err("account login changed during authorization".into());
        }
        Ok(tokens)
    }

    pub async fn recover_account_tokens_after_unauthorized(
        self: &Arc<Self>,
        expected: &ServerAccountRecord,
        rejected_tokens: &TokenSet,
    ) -> Result<TokenSet, String> {
        let persistence = ServerTokenPersistence::for_account(self.clone(), expected);
        if !self
            .token_authority
            .invalidate_access_if_current_and_persist(
                &expected.id,
                rejected_tokens,
                now_ms(),
                &persistence,
            )
            .await
            .map_err(|error| error.to_string())?
        {
            return Err("rejected OAuth token is no longer current".into());
        }
        self.prepare_account_tokens(expected).await
    }

    pub async fn rebuild_runtime(self: &Arc<Self>) -> Result<(), String> {
        let guard = self.lock_runtime_rebuild().await;
        guard.rebuild(self).await
    }

    pub(crate) async fn lock_runtime_rebuild(&self) -> RuntimeBuildGuard<'_> {
        RuntimeBuildGuard {
            _guard: self.runtime_build_lock.lock().await,
        }
    }

    /// Rebuilds the runtime from persisted state and restores the previous
    /// configuration if activation fails. Callers use this for mutations that
    /// have already been committed to the store or vault.
    pub(crate) async fn rebuild_runtime_or_rollback<F>(
        self: &Arc<Self>,
        rollback: F,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Result<(), String>,
    {
        let build = self.lock_runtime_rebuild().await;
        build.rebuild_or_rollback(self, rollback).await
    }

    /// Updates the scopes of all active internal profile keys after a candidate
    /// policy changes in place. This keeps an enabled or un-drained pool member
    /// reachable without replacing the runtime that owns active streams.
    pub(crate) fn refresh_internal_gateway_key_scopes(
        &self,
        runtime: &GatewayRuntime,
    ) -> Result<bool, String> {
        let sources = self.store.sources()?;
        let accounts = self.store.accounts()?;
        let routing = self.store.routing_policy()?;
        let keys = self
            .store
            .keys()?
            .into_iter()
            .filter(|key| key.enabled && is_internal_gateway_key(key))
            .collect::<Vec<_>>();
        let scopes = if keys.is_empty() {
            Vec::new()
        } else {
            let scope = Self::active_internal_gateway_scope(&sources, &accounts, runtime);
            keys.into_iter()
                .map(|key| (key.id, scope.clone()))
                .collect()
        };
        runtime
            .set_pool_routing_policy_with_key_scopes(
                runtime_build::resolve_pool_routing(&routing, &sources, &accounts),
                routing.max_retry_candidates,
                &scopes,
            )
            .map_err(|error| error.to_string())
    }

    fn active_internal_gateway_scope(
        sources: &[SourceRecord],
        accounts: &[ServerAccountRecord],
        runtime: &GatewayRuntime,
    ) -> CandidateScope {
        let (source_ids, account_ids) = runtime_build::pool_member_ids(sources, accounts);
        runtime.active_responses_scope(
            &source_ids.into_iter().collect(),
            &account_ids.into_iter().collect(),
        )
    }

    fn usage_callback(self: &Arc<Self>) -> Result<UsageCallback, String> {
        let mut writer = self
            .usage_writer
            .lock()
            .map_err(|_| "usage writer lock poisoned".to_string())?;
        if writer.is_none() {
            *writer = Some(UsageWriter::start(self)?);
        }
        Ok(writer
            .as_ref()
            .expect("usage writer initialized")
            .callback())
    }

    pub async fn shutdown_runtime(self: &Arc<Self>) -> Result<(), String> {
        self.replace_runtime(None)?;
        let writer = self
            .usage_writer
            .lock()
            .map_err(|_| "usage writer lock poisoned".to_string())?
            .take();
        if let Some(writer) = writer {
            writer.shutdown().await?;
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Result<RuntimeStateSnapshot, String> {
        snapshot::build(self)
    }
}

#[cfg(test)]
mod tests;
